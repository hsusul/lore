//! Codex CLI adapter (see `docs/agents/CODEX.md`).
//!
//! Reads rollout JSONL from `~/.codex/sessions/**` and `~/.codex/archived_sessions/`.
//! Each line is `{ type, timestamp, payload }`. A session is a **linear** event
//! log (no message tree). This step handles `session_meta` (optional git),
//! `response_item` message/reasoning (with `encrypted_content` kept opaque and
//! never indexed/exported), turn-context segmentation, and compaction markers.
//! Tool-call pairing and `patch_apply_end` file events follow in later steps.
//! Read-only and tolerant: unknown/partial input degrades to `partial`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::common::{
    bounded, epoch_ms, fallback_title, non_negative_int_field, resolve_file_event_segments,
    sanitize_path, str_field, unified_diff_line_counts,
};
use super::{
    AgentAdapter, AgentId, AgentMetadata, Capabilities, Detection, DiscoveryRoots, SessionRef,
};
use crate::model::{
    EventKind, FileChangeKind, FileEventSource, ParsedFileEvent, ParsedMessage, ParsedPart,
    ParsedSegment, ParsedSession, ParsedToolCall, PartKind, Role, Tokens,
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
        // native call_id -> index into session.tool_calls, for result pairing.
        let mut id_map: HashMap<String, usize> = HashMap::new();

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
                        "function_call" | "custom_tool_call" => {
                            let part = tool_part(PartKind::ToolUse, None, p);
                            session.messages.push(tool_message(
                                Role::Assistant,
                                part,
                                seq,
                                ts,
                                line_start,
                            ));
                            contexts.push((cwd.clone(), model.clone()));
                            register_tool_call(p, seq, &mut session, &mut id_map);
                            seq += 1;
                        }
                        "function_call_output" | "custom_tool_call_output" => {
                            let output_text = json_text(p.get("output"));
                            let part = tool_part(PartKind::ToolResult, output_text.clone(), p);
                            session.messages.push(tool_message(
                                Role::Tool,
                                part,
                                seq,
                                ts,
                                line_start,
                            ));
                            contexts.push((cwd.clone(), model.clone()));
                            attach_tool_output(p, seq, output_text, &mut session, &id_map);
                            seq += 1;
                        }
                        // tool_search_* / web_search_call are recorded but not yet modeled.
                        "tool_search_call" | "tool_search_output" | "web_search_call" => {}
                        other => {
                            session
                                .note_partial(format!("unknown response_item: {}", bounded(other)));
                        }
                    }
                }
                "event_msg" => {
                    let Some(p) = payload else { continue };
                    // Conversation comes from response_item; most event_msg entries are
                    // telemetry. context_compacted leaves a marker; patch_apply_end yields
                    // file events attached to the matching tool call by call_id.
                    match p.get("type").and_then(Value::as_str).unwrap_or("") {
                        "context_compacted" => {
                            session
                                .messages
                                .push(compaction_marker(None, seq, ts, line_start));
                            contexts.push((cwd.clone(), model.clone()));
                            seq += 1;
                        }
                        "patch_apply_end" => extract_patch_changes(p, ts, &mut session),
                        "token_count" => {
                            if let Some(tokens) = token_totals(p) {
                                session.total_tokens = tokens;
                            }
                        }
                        // Known telemetry that carries no conversation content of
                        // its own (the conversation is reconstructed from
                        // response_item). Intentionally ignored, not flagged
                        // (CODEX.md §3).
                        "task_started"
                        | "user_message"
                        | "agent_message"
                        | "agent_reasoning"
                        | "thread_settings_applied"
                        | "mcp_tool_call_start"
                        | "mcp_tool_call_end"
                        | "web_search_start"
                        | "web_search_end"
                        | "task_complete"
                        | "turn_aborted" => {}
                        // A genuinely new event_msg subtype: degrade to a bounded,
                        // content-free note rather than dropping it silently.
                        other => {
                            session.note_partial(format!("unknown event_msg: {}", bounded(other)));
                        }
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
        resolve_file_event_segments(&mut session);
        // Codex has no native title event, so any derived title is synthetic.
        session.title = fallback_title(&session.messages);
        session.title_is_synthetic = session.title.is_some();
        session
    }
}

/// Derive file events from a `patch_apply_end.changes` object (keyed by path).
/// Each value is `{type, content}` (create/write) or `{type, unified_diff,
/// move_path?}` (update/move). Line counts come only from a valid unified diff.
fn extract_patch_changes(p: &Value, ts: Option<i64>, session: &mut ParsedSession) {
    let call_id = str_field(p, "call_id");
    let Some(changes) = p.get("changes").and_then(Value::as_object) else {
        // A patch_apply_end whose `changes` is absent or not an object is
        // malformed; note it rather than silently recording nothing.
        if p.get("changes").is_some() {
            session.note_partial("malformed patch_apply_end changes (not an object)");
        }
        return;
    };
    for (path, change) in changes {
        let typ = change.get("type").and_then(Value::as_str);
        let move_path = change.get("move_path").and_then(Value::as_str);
        let unified_diff = change.get("unified_diff").and_then(Value::as_str);
        let content = change.get("content").and_then(Value::as_str);

        // An unrecognized change value shape still yields a path-only file event
        // (no content is invented), but the parse is flagged partial so the loss
        // of the recorded payload is visible.
        if typ.is_none() && move_path.is_none() && unified_diff.is_none() && content.is_none() {
            session.note_partial("unrecognized patch_apply_end change shape");
        }

        let (change_kind, event_path, old_path) = if let Some(dest) = move_path {
            (
                FileChangeKind::Move,
                sanitize_path(dest),
                Some(sanitize_path(path)),
            )
        } else {
            let kind = match typ {
                Some("add" | "create") => FileChangeKind::Create,
                Some("delete" | "remove") => FileChangeKind::Delete,
                Some("update" | "modify" | "edit") => FileChangeKind::Edit,
                _ if unified_diff.is_some() => FileChangeKind::Edit,
                _ if content.is_some() => FileChangeKind::Write,
                _ => FileChangeKind::Patch,
            };
            (kind, sanitize_path(path), None)
        };

        let (lines_added, lines_removed) = unified_diff
            .and_then(unified_diff_line_counts)
            .map_or((None, None), |(a, r)| (Some(a), Some(r)));

        session.file_events.push(ParsedFileEvent {
            segment_ix: 0,
            tool_native_call_id: call_id.clone(),
            path: event_path,
            change_kind,
            old_path,
            lines_added,
            lines_removed,
            patch_text: unified_diff.or(content).map(str::to_string),
            source: FileEventSource::AgentPatch,
            event_ts: ts,
        });
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

/// Read the latest cumulative token totals. Current Codex nests these under
/// `info.total_token_usage`; the flat `info.{input,output}` form is retained as
/// a tolerant legacy fallback because the on-disk format is version-sensitive.
fn token_totals(payload: &Value) -> Option<Tokens> {
    let info = payload.get("info")?;
    let usage = info.get("total_token_usage").unwrap_or(info);
    let input = non_negative_int_field(usage, "input_tokens")
        .or_else(|| non_negative_int_field(usage, "input"));
    let output = non_negative_int_field(usage, "output_tokens")
        .or_else(|| non_negative_int_field(usage, "output"));
    let cache = optional_sum([
        non_negative_int_field(usage, "cached_input_tokens"),
        non_negative_int_field(usage, "cache_write_input_tokens"),
    ]);
    if input.is_none() && output.is_none() && cache.is_none() {
        None
    } else {
        Some(Tokens {
            input,
            output,
            cache,
        })
    }
}

fn optional_sum<const N: usize>(values: [Option<i64>; N]) -> Option<i64> {
    let mut seen = false;
    let total = values.into_iter().fold(0_i64, |sum, value| {
        if value.is_some() {
            seen = true;
        }
        sum.saturating_add(value.unwrap_or(0))
    });
    seen.then_some(total)
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
        let text = block
            .as_str()
            .or_else(|| block.get("text").and_then(Value::as_str));
        if let Some(t) = text {
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
            if let Some(s) = block.as_str() {
                push(s);
            } else if let Some(t) = block.get("text").and_then(Value::as_str) {
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

/// Build a single-part message wrapping a tool invocation or result.
fn tool_message(
    role: Role,
    part: ParsedPart,
    seq: i64,
    ts: Option<i64>,
    offset: i64,
) -> ParsedMessage {
    ParsedMessage {
        seq,
        segment_ix: 0,
        native_uuid: None,
        parent_native_uuid: None,
        role,
        event_kind: EventKind::Message,
        is_sidechain: false,
        ts,
        model: None,
        tokens: Tokens::default(),
        stop_reason: None,
        source_offset: Some(offset),
        metadata_json: None,
        parts: vec![part],
    }
}

/// A tool_use/tool_result part preserving the raw payload as JSON content.
fn tool_part(kind: PartKind, text: Option<String>, payload: &Value) -> ParsedPart {
    ParsedPart {
        ordinal: 0,
        kind,
        text,
        content_json: serde_json::to_string(payload).ok(),
        searchable: matches!(kind, PartKind::ToolResult),
        metadata_json: None,
    }
}

/// Register a function/custom tool call, keyed by `call_id`.
fn register_tool_call(
    p: &Value,
    seq: i64,
    session: &mut ParsedSession,
    id_map: &mut HashMap<String, usize>,
) {
    let Some(call_id) = str_field(p, "call_id") else {
        session.note_partial("tool call without call_id");
        return;
    };
    let name = str_field(p, "name").unwrap_or_default();
    // function_call uses "arguments" (a JSON string); custom_tool_call uses "input".
    let input_json = json_text(p.get("arguments").or_else(|| p.get("input")));
    let idx = session.tool_calls.len();
    session.tool_calls.push(ParsedToolCall {
        native_call_id: call_id.clone(),
        name,
        call_ref: (seq, 0),
        result_ref: None,
        input_json,
        output_text: None,
        is_error: None,
        duration_ms: None,
    });
    id_map.insert(call_id, idx);
}

/// Attach a tool output to its call by `call_id`.
fn attach_tool_output(
    p: &Value,
    seq: i64,
    output_text: Option<String>,
    session: &mut ParsedSession,
    id_map: &HashMap<String, usize>,
) {
    let Some(call_id) = str_field(p, "call_id") else {
        session.note_partial("tool output without call_id");
        return;
    };
    let Some(&idx) = id_map.get(&call_id) else {
        session.note_partial("tool output without matching call");
        return;
    };
    let is_error = str_field(p, "status")
        .as_deref()
        .map(|s| !matches!(s, "completed" | "success" | "ok"));
    let tc = &mut session.tool_calls[idx];
    tc.result_ref = Some((seq, 0));
    tc.output_text = output_text;
    tc.is_error = is_error;
}

/// Extract text from a JSON value that may be a string or a structured object.
fn json_text(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        Some(s.to_string())
    } else {
        serde_json::to_string(v).ok()
    }
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
            // Cost estimation (est_cost_usd) is not implemented for any adapter
            // yet; token usage is captured but no priced cost is produced.
            cost: false,
            model_name: true,
            summaries: true,
            git_context: true,
            message_tree: false, // linear
            // `task_complete.duration_ms` is turn latency; per-tool duration is
            // not normalized yet.
            durations: false,
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

    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf> {
        Self::effective_roots(overrides)
    }

    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef> {
        let mut out = Vec::new();
        for root in Self::effective_roots(roots) {
            collect_rollouts(&root, 5, &mut out);
        }
        out
    }

    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        self.parse_str(content, fallback_dedupe)
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
    fn capabilities_reflect_implemented_output() {
        let c = CodexAdapter::new().capabilities();
        // Implemented normalized output.
        assert!(c.messages && c.tool_calls && c.file_events && c.token_usage);
        assert!(c.thinking && c.model_name && c.git_context && c.encrypted_regions);
        // Codex rollouts are a linear log.
        assert!(!c.message_tree);
        // Not yet implemented anywhere.
        assert!(!c.cost, "cost estimation is not implemented");
        assert!(!c.durations, "per-tool durations are not normalized");
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
        assert_eq!(s.title.as_deref(), Some("add a retry to the client"));

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
    fn title_skips_injected_context_before_the_user_request() {
        let content = concat!(
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"<permissions instructions>\\n…\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"Fix repository discovery\"}}\n"
        );
        let session = CodexAdapter::new().parse_str(content, "fallback");
        assert_eq!(session.title.as_deref(), Some("Fix repository discovery"));
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

    #[test]
    fn pairs_function_call_with_output() {
        let s = CodexAdapter::new().parse_str(&fixture("function_call.jsonl"), "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Ok);
        assert_eq!(s.tool_calls.len(), 1);
        let tc = &s.tool_calls[0];
        assert_eq!(tc.native_call_id, "call_1");
        assert_eq!(tc.name, "shell");
        assert!(tc
            .input_json
            .as_deref()
            .is_some_and(|j| j.contains("migrate")));
        assert!(tc.result_ref.is_some());
        assert_eq!(tc.output_text.as_deref(), Some("migration complete"));
        assert_eq!(tc.is_error, Some(false));

        // Call and output are distinct linear records; the invocation part is a
        // tool_use and the output part is a tool_result.
        assert_eq!(s.tool_calls[0].call_ref.0, 1, "invoked at seq 1");
        assert_eq!(s.tool_calls[0].result_ref.map(|r| r.0), Some(2));
    }

    #[test]
    fn tool_output_without_matching_call_degrades_partial() {
        let content = "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T12:00:00.000Z\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"ghost\",\"output\":\"x\"}}\n";
        let s = CodexAdapter::new().parse_str(content, "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert!(s.tool_calls.is_empty());
    }

    #[test]
    fn patch_apply_end_yields_file_events() {
        let s = CodexAdapter::new().parse_str(&fixture("patch_apply.jsonl"), "fallback");
        assert_eq!(s.file_events.len(), 3);

        let by_path = |p: &str| s.file_events.iter().find(|f| f.path == p).unwrap().clone();

        // Create from content (no diff → no line counts).
        let created = by_path("src/new.ts");
        assert_eq!(created.change_kind, FileChangeKind::Create);
        assert_eq!(created.source, FileEventSource::AgentPatch);
        assert_eq!(created.lines_added, None);
        assert!(created
            .patch_text
            .as_deref()
            .is_some_and(|t| t.contains("export")));

        // Update from unified diff → line counts derived.
        let edited = by_path("src/edit.ts");
        assert_eq!(edited.change_kind, FileChangeKind::Edit);
        assert_eq!(edited.lines_added, Some(1));
        assert_eq!(edited.lines_removed, Some(1));

        // Move: destination is the path, source is old_path.
        let moved = by_path("src/renamed.ts");
        assert_eq!(moved.change_kind, FileChangeKind::Move);
        assert_eq!(moved.old_path.as_deref(), Some("src/old.ts"));

        // All attach to the apply_patch tool call and its segment.
        assert!(s
            .file_events
            .iter()
            .all(|f| f.tool_native_call_id.as_deref() == Some("call_9")));
    }

    #[test]
    fn malformed_patch_changes_degrade_without_loss_or_crash() {
        // `changes` present but not an object: flagged, no file events invented.
        let bad_shape = "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"patch_apply_end\",\"call_id\":\"c\",\"changes\":\"oops\"}}\n";
        let s = CodexAdapter::new().parse_str(bad_shape, "bad");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert!(s.file_events.is_empty());

        // A change value with an unrecognized shape: the path is still recorded
        // (no silent total loss) but no content is fabricated and the parse is
        // flagged partial.
        let unknown_value = "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"patch_apply_end\",\"call_id\":\"c\",\"changes\":{\"src/x.ts\":{\"unexpected\":true}}}}\n";
        let s = CodexAdapter::new().parse_str(unknown_value, "bad2");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert_eq!(s.file_events.len(), 1);
        assert_eq!(s.file_events[0].path, "src/x.ts");
        assert_eq!(s.file_events[0].change_kind, FileChangeKind::Patch);
        assert!(s.file_events[0].patch_text.is_none());
    }

    #[test]
    fn delete_patch_maps_kind_and_derives_removed_counts() {
        // A delete recorded with a unified diff keeps the delete kind (not
        // downgraded to edit) and derives removed-line counts from the diff.
        let content = concat!(
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{",
            "\"type\":\"patch_apply_end\",\"call_id\":\"c\",\"changes\":{\"src/gone.ts\":{",
            "\"type\":\"delete\",\"unified_diff\":\"--- a/src/gone.ts\\n+++ /dev/null\\n@@ -1,2 +0,0 @@\\n-line one\\n-line two\\n\"}}}}\n"
        );
        let s = CodexAdapter::new().parse_str(content, "del");
        assert_eq!(s.file_events.len(), 1);
        let fe = &s.file_events[0];
        assert_eq!(fe.change_kind, FileChangeKind::Delete);
        assert_eq!(fe.path, "src/gone.ts");
        assert_eq!(fe.lines_removed, Some(2));
        assert_eq!(fe.lines_added, Some(0));
        assert!(fe
            .patch_text
            .as_deref()
            .is_some_and(|t| t.contains("-line one")));
    }

    #[test]
    fn non_openai_provider_is_preserved() {
        let content = concat!(
            "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"z\",\"cli_version\":\"1\",\"cwd\":\"/p\",\"model_provider\":\"anthropic\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}\n"
        );
        let s = CodexAdapter::new().parse_str(content, "fallback");
        assert_eq!(s.segments[0].provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn token_count_uses_latest_cumulative_totals() {
        let content = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":20,\"cached_input_tokens\":30,\"cache_write_input_tokens\":4}}}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":140,\"output_tokens\":25,\"cached_input_tokens\":35,\"cache_write_input_tokens\":5}}}}\n"
        );
        let session = CodexAdapter::new().parse_str(content, "tokens");
        assert_eq!(session.total_tokens.input, Some(140));
        assert_eq!(session.total_tokens.output, Some(25));
        assert_eq!(session.total_tokens.cache, Some(40));
    }

    #[test]
    fn token_count_tolerates_flat_legacy_shape() {
        let content = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"input\":7,\"output\":3}}}\n";
        let session = CodexAdapter::new().parse_str(content, "legacy-tokens");
        assert_eq!(session.total_tokens.input, Some(7));
        assert_eq!(session.total_tokens.output, Some(3));
        assert_eq!(session.total_tokens.cache, None);
    }

    #[test]
    fn top_level_compacted_and_context_compacted_become_markers() {
        // A mid-session compaction (both the top-level `compacted` record and the
        // `event_msg.context_compacted` marker) must be represented, not dropped.
        let content = concat!(
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"before\"}}\n",
            "{\"type\":\"compacted\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"message\":\"summary of earlier turns\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:02.000Z\",\"payload\":{\"type\":\"context_compacted\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:03.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"after\"}}\n"
        );
        let s = CodexAdapter::new().parse_str(content, "compaction");
        assert_eq!(s.status, crate::model::ParseStatus::Ok);
        assert_eq!(
            s.messages.len(),
            4,
            "compaction markers preserve surrounding turns"
        );

        let markers: Vec<_> = s
            .messages
            .iter()
            .filter(|m| m.event_kind == EventKind::Compaction)
            .collect();
        assert_eq!(markers.len(), 2);
        assert!(markers.iter().all(|m| m.role == Role::Meta));
        // The top-level marker keeps its summary text; the context marker has none.
        assert_eq!(
            markers[0].parts[0].text.as_deref(),
            Some("summary of earlier turns")
        );
        assert!(markers[1].parts.is_empty());
    }

    #[test]
    fn session_meta_git_variants_map_only_present_fields() {
        let parse_git = |git: &str| {
            let content = format!(
                concat!(
                    "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"x\",\"cli_version\":\"1\",\"cwd\":\"/p\"{git}}}}}\n",
                    "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}}}\n"
                ),
                git = git
            );
            let s = CodexAdapter::new().parse_str(&content, "git");
            assert_eq!(
                s.status,
                crate::model::ParseStatus::Ok,
                "git shape never fails a parse"
            );
            let seg = s.segments[0].clone();
            (seg.git_branch, seg.git_commit_sha, seg.git_remote_url)
        };

        // Empty object: no fields.
        assert_eq!(parse_git(",\"git\":{}"), (None, None, None));
        // Branch only.
        assert_eq!(
            parse_git(",\"git\":{\"branch\":\"dev\"}"),
            (Some("dev".into()), None, None)
        );
        // Branch + commit.
        assert_eq!(
            parse_git(",\"git\":{\"branch\":\"dev\",\"commit_hash\":\"abc\"}"),
            (Some("dev".into()), Some("abc".into()), None)
        );
        // Full: branch + commit + repository url.
        assert_eq!(
            parse_git(",\"git\":{\"branch\":\"dev\",\"commit_hash\":\"abc\",\"repository_url\":\"github.com/x/y\"}"),
            (Some("dev".into()), Some("abc".into()), Some("github.com/x/y".into()))
        );
    }

    #[test]
    fn unknown_event_msg_is_flagged_but_known_telemetry_is_not() {
        // Documented telemetry keeps the session Ok...
        let known = concat!(
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"task_started\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.100Z\",\"payload\":{\"type\":\"mcp_tool_call_start\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.500Z\",\"payload\":{\"type\":\"web_search_start\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"task_complete\"}}\n"
        );
        assert_eq!(
            CodexAdapter::new().parse_str(known, "known").status,
            crate::model::ParseStatus::Ok
        );

        // ...but a genuinely new event_msg subtype degrades to partial.
        let unknown = "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"brand_new_event\"}}\n";
        assert_eq!(
            CodexAdapter::new().parse_str(unknown, "unknown").status,
            crate::model::ParseStatus::Partial
        );
    }

    #[test]
    fn truncated_final_line_degrades_to_partial() {
        // A partial final record (interrupted write) is noted, and complete
        // records before it still parse.
        let content = concat!(
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"complete\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11"
        );
        let s = CodexAdapter::new().parse_str(content, "truncated");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert_eq!(s.messages.len(), 1, "the complete record is kept");
    }

    #[test]
    fn parses_string_arrays_in_message_content_and_reasoning() {
        let content = concat!(
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[\"first line\",\"second line\"]}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"reasoning\",\"summary\":[\"thinking step 1\",\"thinking step 2\"],\"content\":[\"detailed step\"]}}\n"
        );
        let s = CodexAdapter::new().parse_str(content, "arrays");
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.messages[0].parts.len(), 2);
        assert_eq!(s.messages[0].parts[0].text.as_deref(), Some("first line"));
        assert_eq!(s.messages[0].parts[1].text.as_deref(), Some("second line"));

        assert_eq!(s.messages[1].parts.len(), 3);
        assert_eq!(
            s.messages[1].parts[0].text.as_deref(),
            Some("thinking step 1")
        );
        assert_eq!(
            s.messages[1].parts[1].text.as_deref(),
            Some("thinking step 2")
        );
        assert_eq!(
            s.messages[1].parts[2].text.as_deref(),
            Some("detailed step")
        );
    }
}
