//! Claude Code adapter (see `docs/agents/CLAUDE_CODE.md`).
//!
//! Reads `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl` (honoring
//! `CLAUDE_CONFIG_DIR`). Each line is one event. This module parses the message
//! envelope and ordered content blocks (text/thinking now; tool pairing and
//! file events in later steps) into the normalized model. It is read-only and
//! tolerant: an unparseable or truncated line degrades the session to
//! `partial` with a bounded, content-free note — never a panic.

use std::path::PathBuf;

use serde_json::Value;

use super::{
    AdapterError, AgentAdapter, AgentId, AgentMetadata, Capabilities, Detection, DiscoveryRoots,
    SessionRef,
};
use crate::model::{
    EventKind, ParsedMessage, ParsedPart, ParsedSegment, ParsedSession, PartKind, Role, Tokens,
};

const AGENT_ID: AgentId = AgentId("claude-code");

/// Read-only adapter for Claude Code sessions.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The default session root: `$CLAUDE_CONFIG_DIR/projects` or
    /// `$HOME/.claude/projects`.
    fn default_root() -> Option<PathBuf> {
        let base = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")));
        base.map(|b| b.join("projects"))
    }

    fn effective_roots(roots: &DiscoveryRoots) -> Vec<PathBuf> {
        if roots.roots.is_empty() {
            Self::default_root().into_iter().collect()
        } else {
            roots.roots.clone()
        }
    }

    /// Parse already-read file content. Separated for deterministic testing.
    #[must_use]
    pub fn parse_str(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        let mut session = ParsedSession::new(fallback_dedupe);
        // Per-message context (cwd, branch) used to build segments afterward.
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
            let event_type = obj.get("type").and_then(Value::as_str).unwrap_or("");

            // Session-level metadata.
            if session.native_session_id.is_none() {
                session.native_session_id = str_field(&obj, "sessionId");
            }
            if session.agent_version.is_none() {
                session.agent_version = str_field(&obj, "version");
            }

            match event_type {
                "custom-title" => {
                    session.title = str_field(&obj, "customTitle").or(session.title.take());
                    continue;
                }
                "ai-title" => {
                    if session.title.is_none() {
                        session.title = str_field(&obj, "aiTitle");
                    }
                    continue;
                }
                // Internal bookkeeping events carry no conversation content.
                "queue-operation" | "last-prompt" => continue,
                "user" | "assistant" | "system" | "attachment" | "summary" | "mode" | "pr-link" => {
                }
                other => {
                    session.note_partial(format!("unhandled event type: {}", bounded(other)));
                    continue;
                }
            }

            let msg = build_message(&obj, event_type, seq, line_start, &mut session);
            contexts.push((str_field(&obj, "cwd"), str_field(&obj, "gitBranch")));
            if let Some(ts) = msg.ts {
                session.started_at = Some(session.started_at.map_or(ts, |s| s.min(ts)));
                session.ended_at = Some(session.ended_at.map_or(ts, |e| e.max(ts)));
            }
            if session.primary_model.is_none() {
                session.primary_model = msg.model.clone();
            }
            session.messages.push(msg);
            seq += 1;
        }

        assign_segments(&mut session, &contexts);
        session
    }
}

/// Build one `ParsedMessage` from an event object.
fn build_message(
    obj: &Value,
    event_type: &str,
    seq: i64,
    offset: i64,
    session: &mut ParsedSession,
) -> ParsedMessage {
    let message = obj.get("message");
    let role = role_for(event_type, message);
    let event_kind = event_kind_for(event_type);
    let is_sidechain = obj
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ts = str_field(obj, "timestamp").as_deref().and_then(epoch_ms);

    let mut tokens = Tokens::default();
    let mut model = None;
    let mut stop_reason = None;
    if let Some(m) = message {
        model = str_field(m, "model");
        stop_reason = str_field(m, "stop_reason");
        if let Some(usage) = m.get("usage") {
            tokens = Tokens {
                input: usage.get("input_tokens").and_then(Value::as_i64),
                output: usage.get("output_tokens").and_then(Value::as_i64),
                cache: usage.get("cache_read_input_tokens").and_then(Value::as_i64),
            };
        }
    }

    let parts = match message.and_then(|m| m.get("content")) {
        Some(content) => parts_from_content(content, session),
        None => obj
            .get("attachment")
            .map(|a| vec![raw_part(0, PartKind::Attachment, a)])
            .unwrap_or_default(),
    };

    ParsedMessage {
        seq,
        segment_ix: 0,
        native_uuid: str_field(obj, "uuid"),
        parent_native_uuid: str_field(obj, "parentUuid"),
        role,
        event_kind,
        is_sidechain,
        ts,
        model,
        tokens,
        stop_reason,
        source_offset: Some(offset),
        metadata_json: None,
        parts,
    }
}

/// Turn a `message.content` value (string or array of typed blocks) into ordered parts.
fn parts_from_content(content: &Value, session: &mut ParsedSession) -> Vec<ParsedPart> {
    if let Some(text) = content.as_str() {
        return vec![ParsedPart {
            ordinal: 0,
            kind: PartKind::Text,
            text: Some(text.to_string()),
            content_json: None,
            searchable: true,
            metadata_json: None,
        }];
    }
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };

    let mut parts = Vec::with_capacity(blocks.len());
    for (i, block) in blocks.iter().enumerate() {
        let ordinal = i as i64;
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("");
        let part = match btype {
            "text" => ParsedPart {
                ordinal,
                kind: PartKind::Text,
                text: str_field(block, "text"),
                content_json: None,
                searchable: true,
                metadata_json: None,
            },
            "thinking" => ParsedPart {
                ordinal,
                kind: PartKind::Thinking,
                text: str_field(block, "thinking"),
                content_json: None,
                // Thinking is preserved but not searchable by default (privacy).
                searchable: false,
                metadata_json: str_field(block, "signature").and_then(|s| {
                    serde_json::to_string(&serde_json::json!({ "signature": s })).ok()
                }),
            },
            "tool_use" => raw_part(ordinal, PartKind::ToolUse, block),
            "tool_result" => {
                // Result content may be a string or a structured array.
                let text = block
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                ParsedPart {
                    ordinal,
                    kind: PartKind::ToolResult,
                    text,
                    content_json: json_field(block, "content"),
                    searchable: true,
                    metadata_json: None,
                }
            }
            other => {
                session.note_partial(format!("unknown content block: {}", bounded(other)));
                raw_part(ordinal, PartKind::Other, block)
            }
        };
        parts.push(part);
    }
    parts
}

fn raw_part(ordinal: i64, kind: PartKind, block: &Value) -> ParsedPart {
    ParsedPart {
        ordinal,
        kind,
        text: None,
        content_json: serde_json::to_string(block).ok(),
        searchable: false,
        metadata_json: None,
    }
}

/// Group consecutive messages sharing (cwd, branch) into segments and assign
/// each message its `segment_ix`.
fn assign_segments(session: &mut ParsedSession, contexts: &[(Option<String>, Option<String>)]) {
    session.segments.clear();
    let mut current: Option<(Option<String>, Option<String>)> = None;
    for (i, ctx) in contexts.iter().enumerate() {
        let seq = session.messages[i].seq;
        if current.as_ref() != Some(ctx) {
            if let Some(last) = session.segments.last_mut() {
                last.seq_end = session.messages[i.saturating_sub(1)].seq;
            }
            session.segments.push(ParsedSegment {
                seq_start: seq,
                seq_end: seq,
                cwd: ctx.0.clone(),
                model: None,
                provider: None,
                git_branch: ctx.1.clone(),
                git_commit_sha: None,
                git_remote_url: None,
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

fn role_for(event_type: &str, message: Option<&Value>) -> Role {
    if let Some(role) = message.and_then(|m| m.get("role")).and_then(Value::as_str) {
        return match role {
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            _ => Role::User,
        };
    }
    match event_type {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "summary" | "mode" | "pr-link" => Role::Meta,
        _ => Role::User,
    }
}

fn event_kind_for(event_type: &str) -> EventKind {
    match event_type {
        "attachment" => EventKind::Attachment,
        "summary" => EventKind::Summary,
        "mode" => EventKind::Mode,
        "pr-link" => EventKind::PrLink,
        _ => EventKind::Message,
    }
}

fn str_field(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

fn json_field(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| serde_json::to_string(v).ok())
}

/// Parse an RFC3339 timestamp to epoch milliseconds.
fn epoch_ms(s: &str) -> Option<i64> {
    let dt = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(dt.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Bound a schema token used in a diagnostic (never user content).
fn bounded(s: &str) -> String {
    s.chars().take(40).collect()
}

impl AgentAdapter for ClaudeCodeAdapter {
    fn id(&self) -> AgentId {
        AGENT_ID
    }

    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            display_name: "Claude Code",
            format_id: "claude-code/jsonl",
            doc_link: "docs/agents/CLAUDE_CODE.md",
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            messages: true,
            thinking: true,
            tool_calls: true,
            file_events: true, // derived from tool use (later step)
            token_usage: true,
            cost: true,
            model_name: true,
            summaries: true,
            git_context: true, // branch only
            message_tree: true,
            durations: false,
            encrypted_regions: false,
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
            collect_jsonl(&root, 3, &mut out);
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

/// Recursively collect `*.jsonl` files up to `depth` levels below `dir`.
fn collect_jsonl(dir: &std::path::Path, depth: usize, out: &mut Vec<SessionRef>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_jsonl(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/claude_code")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn parses_basic_text_session() {
        let a = ClaudeCodeAdapter::new();
        let s = a.parse_str(&fixture("basic_text.jsonl"), "fallback");

        assert_eq!(s.status, crate::model::ParseStatus::Ok);
        assert_eq!(
            s.native_session_id.as_deref(),
            Some("aaaaaaaa-0000-4000-8000-000000000001")
        );
        assert_eq!(s.agent_version.as_deref(), Some("1.2.3"));
        assert_eq!(s.title.as_deref(), Some("Add health check endpoint"));
        assert_eq!(s.messages.len(), 2, "title event is not a message");

        let user = &s.messages[0];
        assert_eq!(user.role, Role::User);
        assert_eq!(user.parts.len(), 1);
        assert_eq!(user.parts[0].kind, PartKind::Text);

        let asst = &s.messages[1];
        assert_eq!(asst.role, Role::Assistant);
        assert_eq!(asst.parts.len(), 2, "thinking + text");
        assert_eq!(asst.parts[0].kind, PartKind::Thinking);
        assert!(
            !asst.parts[0].searchable,
            "thinking not searchable by default"
        );
        assert!(asst.parts[0]
            .metadata_json
            .as_deref()
            .is_some_and(|m| m.contains("signature")));
        assert_eq!(asst.parts[1].kind, PartKind::Text);
        assert_eq!(asst.tokens.input, Some(1200));
        assert_eq!(asst.tokens.output, Some(40));
        assert_eq!(asst.tokens.cache, Some(800));
        assert_eq!(asst.model.as_deref(), Some("claude-x"));
    }

    #[test]
    fn single_segment_for_constant_context() {
        let a = ClaudeCodeAdapter::new();
        let s = a.parse_str(&fixture("basic_text.jsonl"), "fallback");
        assert_eq!(s.segments.len(), 1);
        assert_eq!(s.segments[0].cwd.as_deref(), Some("/repo/app"));
        assert_eq!(s.segments[0].git_branch.as_deref(), Some("main"));
        assert!(s.messages.iter().all(|m| m.segment_ix == 0));
        assert!(s.started_at.is_some() && s.started_at <= s.ended_at);
    }

    #[test]
    fn truncated_final_line_degrades_to_partial() {
        let a = ClaudeCodeAdapter::new();
        let mut content = fixture("basic_text.jsonl");
        content.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assis");
        let s = a.parse_str(&content, "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert_eq!(s.messages.len(), 2, "complete messages still parsed");
    }

    #[test]
    fn unknown_event_type_notes_partial_without_failing() {
        let a = ClaudeCodeAdapter::new();
        let content = "{\"type\":\"tool_search_call\",\"sessionId\":\"x\"}\n";
        let s = a.parse_str(content, "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert!(s.messages.is_empty());
    }
}
