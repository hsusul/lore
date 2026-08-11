//! Codex CLI adapter (see `docs/agents/CODEX.md`).
//!
//! Reads rollout JSONL from `~/.codex/sessions/**` and `~/.codex/archived_sessions/`.
//! Each line is `{ type, timestamp, payload }`. A session is a **linear** event
//! log (no message tree). This step handles `session_meta` (optional git),
//! `response_item` message/reasoning (with `encrypted_content` kept opaque and
//! never indexed/exported), turn-context segmentation, and compaction markers.
//! Tool-call pairing and `patch_apply_end` file events follow in later steps.
//! Read-only and tolerant: unknown/partial input degrades to `partial`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::common::{bounded, epoch_ms};
use super::{
    AdapterError, AgentAdapter, AgentId, AgentMetadata, Capabilities, Detection, DiscoveryRoots,
    SessionRef,
};
use crate::model::{
    EventKind, ParsedMessage, ParsedPart, ParsedSegment, ParsedSession, PartKind, Role, Tokens,
};

const AGENT_ID: AgentId = AgentId("codex");

/// Read-only adapter for Codex CLI rollout sessions.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexAdapter;

/// Git identity as recorded in `session_meta` (any field may be absent).
type RecordedGit = (Option<String>, Option<String>, Option<String>);

impl CodexAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn default_roots() -> Vec<PathBuf> {
        let Some(home) = std::env::var_os("HOME") else {
            return Vec::new();
        };
        let base = PathBuf::from(home).join(".codex");
        vec![base.join("sessions"), base.join("archived_sessions")]
    }

    fn effective_roots(roots: &DiscoveryRoots) -> Vec<PathBuf> {
        if roots.roots.is_empty() {
            Self::default_roots()
        } else {
            roots.roots.clone()
        }
    }

    /// Parse rollout content. Separated for deterministic testing.
    #[must_use]
    pub fn parse_str(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        let mut session = ParsedSession::new(fallback_dedupe);
        let mut cwd: Option<String> = None;
        let mut model: Option<String> = None;
        let mut provider: Option<String> = None;
        let mut git: RecordedGit = (None, None, None);
        // (cwd, model) at each emitted message, for segmentation.
        let mut contexts: Vec<(Option<String>, Option<String>)> = Vec::new();
        let mut offset: i64 = 0;
        let mut seq: i64 = 0;

        for line in content.split_inclusive('\n') {
            let line_start = offset;
            offset += line.len() as i64;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(obj) = serde_json::from_str::<Value>(trimmed) else {
                session.note_partial("unparseable line (possibly truncated)");
                continue;
            };
            let typ = obj.get("type").and_then(Value::as_str).unwrap_or("");
            let ts = obj
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(epoch_ms);
            if let Some(ts) = ts {
                session.started_at = Some(session.started_at.map_or(ts, |s| s.min(ts)));
                session.ended_at = Some(session.ended_at.map_or(ts, |e| e.max(ts)));
            }
            let payload = obj.get("payload");

            match typ {
                "session_meta" => {
                    if let Some(p) = payload {
                        session.native_session_id = str_field(p, "id");
                        session.agent_version = str_field(p, "cli_version");
                        provider = str_field(p, "model_provider");
                        if let Some(c) = str_field(p, "cwd") {
                            cwd = Some(c);
                        }
                        git = recorded_git(p);
                    }
                }
                "turn_context" => {
                    if let Some(p) = payload {
                        if let Some(c) = str_field(p, "cwd") {
                            cwd = Some(c);
                        }
                        if let Some(m) = str_field(p, "model") {
                            model = Some(m);
                        }
                    }
                }
                "response_item" => {
                    let Some(p) = payload else { continue };
                    match p.get("type").and_then(Value::as_str).unwrap_or("") {
                        "message" => {
                            let msg = message_item(p, seq, ts, model.clone(), line_start);
                            session.messages.push(msg);
                            contexts.push((cwd.clone(), model.clone()));
                            seq += 1;
                        }
                        "reasoning" => {
                            let msg = reasoning_item(p, seq, ts, model.clone(), line_start);
                            session.messages.push(msg);
                            contexts.push((cwd.clone(), model.clone()));
                            seq += 1;
                        }
                        // Tool families are paired in M2.b/c.
                        "function_call"
                        | "function_call_output"
                        | "custom_tool_call"
                        | "custom_tool_call_output"
                        | "tool_search_call"
                        | "tool_search_output"
                        | "web_search_call" => {}
                        other => {
                            session
                                .note_partial(format!("unknown response_item: {}", bounded(other)));
                        }
                    }
                }
                "event_msg" => {
                    let Some(p) = payload else { continue };
                    // Conversation comes from response_item; most event_msg entries are
                    // telemetry. context_compacted leaves a marker; patch_apply_end and
                    // token_count are handled in later steps.
                    if p.get("type").and_then(Value::as_str) == Some("context_compacted") {
                        session
                            .messages
                            .push(compaction_marker(None, seq, ts, line_start));
                        contexts.push((cwd.clone(), model.clone()));
                        seq += 1;
                    }
                }
                "compacted" => {
                    let text = payload.and_then(|p| str_field(p, "message"));
                    session
                        .messages
                        .push(compaction_marker(text, seq, ts, line_start));
                    contexts.push((cwd.clone(), model.clone()));
                    seq += 1;
                }
                // Known non-conversation record types.
                "world_state" | "inter_agent_communication_metadata" => {}
                other => {
                    session.note_partial(format!("unknown record type: {}", bounded(other)));
                }
            }
        }

        assign_segments(&mut session, &contexts, provider.as_ref(), &git);
        session
    }
}

/// Extract the recorded git fields that are actually present (absence is normal).
fn recorded_git(meta: &Value) -> RecordedGit {
    let Some(g) = meta.get("git").filter(|v| v.is_object()) else {
        return (None, None, None);
    };
    (
        str_field(g, "branch"),
        str_field(g, "commit_hash"),
        str_field(g, "repository_url"),
    )
}

fn message_item(
    p: &Value,
    seq: i64,
    ts: Option<i64>,
    model: Option<String>,
    offset: i64,
) -> ParsedMessage {
    let role = match p.get("role").and_then(Value::as_str).unwrap_or("") {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "tool" => Role::Tool,
        _ => Role::User,
    };
    ParsedMessage {
        seq,
        segment_ix: 0,
        native_uuid: None,
        parent_native_uuid: None,
        role,
        event_kind: EventKind::Message,
        is_sidechain: false,
        ts,
        model,
        tokens: Tokens::default(),
        stop_reason: None,
        source_offset: Some(offset),
        metadata_json: None,
        parts: text_parts(p.get("content")),
    }
}

fn reasoning_item(
    p: &Value,
    seq: i64,
    ts: Option<i64>,
    model: Option<String>,
    offset: i64,
) -> ParsedMessage {
    let mut parts = Vec::new();
    let mut ord = 0_i64;
    push_reasoning_text(p.get("summary"), &mut parts, &mut ord);
    push_reasoning_text(p.get("content"), &mut parts, &mut ord);
    // Opaque encrypted reasoning: preserved but never rendered/indexed/exported.
    if let Some(enc) = p.get("encrypted_content").and_then(Value::as_str) {
        parts.push(ParsedPart {
            ordinal: ord,
            kind: PartKind::Opaque,
            text: None,
            content_json: serde_json::to_string(&serde_json::json!({ "encrypted_content": enc }))
                .ok(),
            searchable: false,
            metadata_json: Some(r#"{"opaque":"encrypted_content"}"#.to_string()),
        });
    }
    ParsedMessage {
        seq,
        segment_ix: 0,
        native_uuid: None,
        parent_native_uuid: None,
        role: Role::Assistant,
        event_kind: EventKind::Message,
        is_sidechain: false,
        ts,
        model,
        tokens: Tokens::default(),
        stop_reason: None,
        source_offset: Some(offset),
        metadata_json: None,
        parts,
    }
}

fn compaction_marker(
    text: Option<String>,
    seq: i64,
    ts: Option<i64>,
    offset: i64,
) -> ParsedMessage {
    ParsedMessage {
        seq,
        segment_ix: 0,
        native_uuid: None,
        parent_native_uuid: None,
        role: Role::Meta,
        event_kind: EventKind::Compaction,
        is_sidechain: false,
        ts,
        model: None,
        tokens: Tokens::default(),
        stop_reason: None,
        source_offset: Some(offset),
        metadata_json: None,
        parts: text
            .map(|t| {
                vec![ParsedPart {
                    ordinal: 0,
                    kind: PartKind::Summary,
                    text: Some(t),
                    content_json: None,
                    searchable: true,
                    metadata_json: None,
                }]
            })
            .unwrap_or_default(),
    }
}

/// Convert a Codex message `content` (string or array of `{text}` blocks) into
/// ordered text parts.
fn text_parts(content: Option<&Value>) -> Vec<ParsedPart> {
    let Some(content) = content else {
        return Vec::new();
    };
    if let Some(s) = content.as_str() {
        return vec![ParsedPart {
            ordinal: 0,
            kind: PartKind::Text,
            text: Some(s.to_string()),
            content_json: None,
            searchable: true,
            metadata_json: None,
        }];
    }
    let Some(arr) = content.as_array() else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    for block in arr {
        if let Some(t) = block.get("text").and_then(Value::as_str) {
            parts.push(ParsedPart {
                ordinal: parts.len() as i64,
                kind: PartKind::Text,
                text: Some(t.to_string()),
                content_json: None,
                searchable: true,
                metadata_json: None,
            });
        }
    }
    parts
}

/// Reasoning `content`/`summary` may be a string or array of `{text}` blocks;
/// each becomes a non-searchable thinking part.
fn push_reasoning_text(value: Option<&Value>, parts: &mut Vec<ParsedPart>, ord: &mut i64) {
    let Some(value) = value else { return };
    let mut push = |text: &str| {
        parts.push(ParsedPart {
            ordinal: *ord,
            kind: PartKind::Thinking,
            text: Some(text.to_string()),
            content_json: None,
            searchable: false,
            metadata_json: None,
        });
        *ord += 1;
    };
    if let Some(s) = value.as_str() {
        push(s);
    } else if let Some(arr) = value.as_array() {
        for block in arr {
            if let Some(t) = block.get("text").and_then(Value::as_str) {
                push(t);
            }
        }
    }
}

/// Group consecutive messages by (cwd, model). Recorded git attaches to the
/// first segment only (it is stamped once at session start), and provider to all.
fn assign_segments(
    session: &mut ParsedSession,
    contexts: &[(Option<String>, Option<String>)],
    provider: Option<&String>,
    git: &RecordedGit,
) {
    session.segments.clear();
    let mut current: Option<(Option<String>, Option<String>)> = None;
    for (i, ctx) in contexts.iter().enumerate() {
        let seq = session.messages[i].seq;
        if current.as_ref() != Some(ctx) {
            if let Some(last) = session.segments.last_mut() {
                last.seq_end = session.messages[i.saturating_sub(1)].seq;
            }
            let first = session.segments.is_empty();
            session.segments.push(ParsedSegment {
                seq_start: seq,
                seq_end: seq,
                cwd: ctx.0.clone(),
                model: ctx.1.clone(),
                provider: provider.cloned(),
                git_branch: if first { git.0.clone() } else { None },
                git_commit_sha: if first { git.1.clone() } else { None },
                git_remote_url: if first { git.2.clone() } else { None },
            });
            current = Some(ctx.clone());
        }
        let ix = session.segments.len() - 1;
        session.messages[i].segment_ix = ix;
        if let Some(last) = session.segments.last_mut() {
            last.seq_end = seq;
        }
    }
}

fn str_field(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

impl AgentAdapter for CodexAdapter {
    fn id(&self) -> AgentId {
        AGENT_ID
    }

    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            display_name: "Codex",
            format_id: "codex/rollout-jsonl",
            doc_link: "docs/agents/CODEX.md",
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            messages: true,
            thinking: true,
            tool_calls: true,
            file_events: true,
            token_usage: true,
            cost: true,
            model_name: true,
            summaries: true,
            git_context: true,
            message_tree: false, // linear
            durations: true,
            encrypted_regions: true,
        }
    }

    fn detect_installation(&self, roots: &DiscoveryRoots) -> Detection {
        let found: Vec<PathBuf> = Self::effective_roots(roots)
            .into_iter()
            .filter(|p| p.is_dir())
            .collect();
        Detection {
            installed: !found.is_empty(),
            version: None,
            roots_found: found,
        }
    }

    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef> {
        let mut out = Vec::new();
        for root in Self::effective_roots(roots) {
            collect_rollouts(&root, 5, &mut out);
        }
        out
    }

    fn parse_session(&self, source: &SessionRef) -> Result<ParsedSession, AdapterError> {
        let content = std::fs::read_to_string(&source.path).map_err(|_| AdapterError::Io)?;
        let fallback = source
            .native_id
            .clone()
            .unwrap_or_else(|| source.path.to_string_lossy().into_owned());
        Ok(self.parse_str(&content, &fallback))
    }
}

/// Recursively collect `rollout-*.jsonl` files up to `depth` levels below `dir`.
fn collect_rollouts(dir: &Path, depth: usize, out: &mut Vec<SessionRef>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_rollouts(&path, depth - 1, out);
            }
        } else {
            let is_rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
            if is_rollout {
                let meta = entry.metadata().ok();
                out.push(SessionRef {
                    agent: AGENT_ID,
                    native_id: path.file_stem().map(|s| s.to_string_lossy().into_owned()),
                    size: meta.as_ref().map_or(0, std::fs::Metadata::len),
                    mtime: meta.and_then(|m| m.modified().ok()),
                    path,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/codex")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn parses_minimal_session_with_git_and_reasoning() {
        let s = CodexAdapter::new().parse_str(&fixture("minimal.jsonl"), "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Ok);
        assert_eq!(
            s.native_session_id.as_deref(),
            Some("019e0000-0000-7000-8000-000000000001")
        );
        assert_eq!(s.agent_version.as_deref(), Some("0.133.0"));

        // user message + reasoning + assistant message.
        assert_eq!(s.messages.len(), 3);
        assert_eq!(s.messages[0].role, Role::User);
        assert_eq!(s.messages[1].role, Role::Assistant);
        assert_eq!(s.messages[1].parts[0].kind, PartKind::Thinking);
        assert!(!s.messages[1].parts[0].searchable);
        assert_eq!(s.messages[2].role, Role::Assistant);

        // git recorded on the first segment; provider set.
        assert_eq!(s.segments.len(), 1);
        assert_eq!(s.segments[0].git_branch.as_deref(), Some("main"));
        assert_eq!(s.segments[0].git_commit_sha.as_deref(), Some("3ab9f1"));
        assert_eq!(s.segments[0].provider.as_deref(), Some("openai"));
        assert_eq!(s.segments[0].cwd.as_deref(), Some("/proj"));
    }

    #[test]
    fn encrypted_reasoning_is_opaque_and_not_searchable() {
        let content = "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"reasoning\",\"summary\":\"plan\",\"encrypted_content\":\"ENCRYPTED-BLOB\"}}\n";
        let s = CodexAdapter::new().parse_str(content, "fallback");
        let opaque = s.messages[0]
            .parts
            .iter()
            .find(|p| p.kind == PartKind::Opaque)
            .expect("opaque part present");
        assert!(
            !opaque.searchable,
            "encrypted content must not be searchable"
        );
        assert!(
            opaque.text.is_none(),
            "encrypted content is never rendered as text"
        );
    }

    #[test]
    fn missing_git_is_not_an_error() {
        let content = concat!(
            "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"x\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}\n"
        );
        let s = CodexAdapter::new().parse_str(content, "fallback");
        assert_eq!(
            s.status,
            crate::model::ParseStatus::Ok,
            "absent git is normal"
        );
        assert!(s.segments[0].git_branch.is_none());
    }

    #[test]
    fn unknown_record_type_degrades_partial() {
        let content = "{\"type\":\"brand_new_type\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{}}\n";
        let s = CodexAdapter::new().parse_str(content, "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
    }
}
