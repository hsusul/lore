//! Claude Code adapter (see `docs/agents/CLAUDE_CODE.md`).
//!
//! Reads `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl` (honoring
//! `CLAUDE_CONFIG_DIR`). Each line is one event. This module parses the message
//! envelope and ordered content blocks (text/thinking now; tool pairing and
//! file events in later steps) into the normalized model. It is read-only and
//! tolerant: an unparseable or truncated line degrades the session to
//! `partial` with a bounded, content-free note — never a panic.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use super::common::{bounded, epoch_ms, fallback_title, json_field, sanitize_path, str_field};
use super::{
    AgentAdapter, AgentId, AgentMetadata, Capabilities, Detection, DiscoveryRoots, SessionRef,
};
use crate::model::{
    EventKind, FileChangeKind, FileEventSource, ParsedFileEvent, ParsedMessage, ParsedPart,
    ParsedSegment, ParsedSession, ParsedToolCall, PartKind, Role, Tokens,
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
        // native_call_id -> index into session.tool_calls, for result pairing.
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
            extract_tool_calls(&obj, seq, &mut session, &mut id_map);
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
        super::common::resolve_file_event_segments(&mut session);
        if session.title.is_none() {
            session.title = fallback_title(&session.messages);
            session.title_is_synthetic = session.title.is_some();
        }
        session
    }
}

/// Extract tool calls and results from a message's content blocks, pairing
/// results to calls by `tool_use_id`, and derive file events from file-mutating
/// tool invocations.
fn extract_tool_calls(
    obj: &Value,
    seq: i64,
    session: &mut ParsedSession,
    id_map: &mut HashMap<String, usize>,
) {
    let Some(blocks) = obj
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for (i, block) in blocks.iter().enumerate() {
        let ordinal = i as i64;
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "tool_use" => {
                let Some(id) = str_field(block, "id") else {
                    session.note_partial("tool_use without id");
                    continue;
                };
                let name = str_field(block, "name").unwrap_or_default();
                let idx = session.tool_calls.len();
                session.tool_calls.push(ParsedToolCall {
                    native_call_id: id.clone(),
                    name: name.clone(),
                    call_ref: (seq, ordinal),
                    result_ref: None,
                    input_json: json_field(block, "input"),
                    output_text: None,
                    is_error: None,
                    duration_ms: None,
                });
                id_map.insert(id.clone(), idx);
                if let Some(fe) = file_event_from_tool(&name, block, id) {
                    session.file_events.push(fe);
                }
            }
            "tool_result" => {
                let Some(tid) = str_field(block, "tool_use_id") else {
                    session.note_partial("tool_result without tool_use_id");
                    continue;
                };
                if let Some(&idx) = id_map.get(&tid) {
                    let tc = &mut session.tool_calls[idx];
                    tc.result_ref = Some((seq, ordinal));
                    tc.output_text = tool_result_text(block);
                    tc.is_error = block.get("is_error").and_then(Value::as_bool);
                } else {
                    session.note_partial("tool_result without matching tool_use");
                }
            }
            _ => {}
        }
    }
}

/// Derive a file event from a file-mutating tool invocation (Claude has no
/// structured patch event; the file path comes from the tool input).
fn file_event_from_tool(name: &str, block: &Value, call_id: String) -> Option<ParsedFileEvent> {
    let change_kind = match name {
        "Edit" | "MultiEdit" | "NotebookEdit" => FileChangeKind::Edit,
        "Write" => FileChangeKind::Write,
        _ => return None,
    };
    let raw = block
        .get("input")
        .and_then(|inp| {
            inp.get("file_path")
                .or_else(|| inp.get("notebook_path"))
                .or_else(|| inp.get("path"))
        })
        .and_then(Value::as_str)?;
    Some(ParsedFileEvent {
        segment_ix: 0,
        tool_native_call_id: Some(call_id),
        path: sanitize_path(raw),
        change_kind,
        old_path: None,
        lines_added: None,
        lines_removed: None,
        patch_text: None,
        source: FileEventSource::AgentToolInput,
        event_ts: None,
    })
}

/// Extract textual tool-result output (string, or concatenated text blocks).
fn tool_result_text(block: &Value) -> Option<String> {
    let content = block.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        let joined: Vec<String> = arr
            .iter()
            .filter_map(|b| {
                b.as_str()
                    .map(str::to_string)
                    .or_else(|| b.get("text").and_then(Value::as_str).map(str::to_string))
            })
            .collect();
        if !joined.is_empty() {
            return Some(joined.join("\n"));
        }
    }
    None
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
        if let Some(s) = block.as_str() {
            parts.push(ParsedPart {
                ordinal,
                kind: PartKind::Text,
                text: Some(s.to_string()),
                content_json: None,
                searchable: true,
                metadata_json: None,
            });
            continue;
        }
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
                let text = tool_result_text(block);
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
            file_events: true, // derived from tool use
            token_usage: true,
            // Cost estimation (est_cost_usd) is not implemented for any adapter
            // yet; token usage is captured but no priced cost is produced.
            cost: false,
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

    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf> {
        Self::effective_roots(overrides)
    }

    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef> {
        let mut out = Vec::new();
        for root in Self::effective_roots(roots) {
            collect_jsonl(&root, &mut out);
        }
        out
    }

    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        self.parse_str(content, fallback_dedupe)
    }
}

/// Recursively collect `*.jsonl` transcripts below `dir`.
///
/// There is no depth limit. Claude Code writes transcripts at several nesting
/// levels — `<session>.jsonl` at the project root, `subagents/agent-<id>.jsonl`,
/// and `subagents/workflows/<wf-id>/agent-<id>.jsonl` — and upstream has added
/// levels before, so a hardcoded budget silently skips the deepest transcripts.
/// Instead we walk the whole tree and select on the `.jsonl` extension. Only
/// real directories are descended into: [`std::fs::DirEntry::file_type`] does not
/// follow symlinks, so a symlink cycle cannot recurse forever.
fn collect_jsonl(dir: &std::path::Path, out: &mut Vec<SessionRef>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_jsonl(&path, out);
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
    fn capabilities_reflect_implemented_output() {
        let c = ClaudeCodeAdapter::new().capabilities();
        // Implemented normalized output.
        assert!(c.messages && c.tool_calls && c.file_events && c.token_usage);
        assert!(c.thinking && c.model_name && c.git_context && c.message_tree);
        // Claude Code has no opaque/encrypted regions.
        assert!(!c.encrypted_regions);
        // Not yet implemented anywhere.
        assert!(!c.cost, "cost estimation is not implemented");
        assert!(!c.durations, "per-tool durations are not normalized");
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
    fn derives_title_when_native_title_event_is_absent() {
        let content = concat!(
            "{\"type\":\"user\",\"sessionId\":\"s\",\"message\":{\"role\":\"user\",\"content\":\"Investigate the cache miss\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"s\",\"message\":{\"role\":\"assistant\",\"content\":\"On it\"}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(content, "fallback");
        assert_eq!(session.title.as_deref(), Some("Investigate the cache miss"));
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

    #[test]
    fn pairs_tool_use_with_result_and_derives_file_event() {
        let a = ClaudeCodeAdapter::new();
        let s = a.parse_str(&fixture("tool_use.jsonl"), "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Ok);

        assert_eq!(s.tool_calls.len(), 1);
        let tc = &s.tool_calls[0];
        assert_eq!(tc.native_call_id, "toolu_1");
        assert_eq!(tc.name, "Edit");
        assert_eq!(tc.call_ref.0, 1, "invoked in the assistant message (seq 1)");
        assert!(tc.result_ref.is_some(), "result paired");
        assert_eq!(tc.output_text.as_deref(), Some("File edited successfully"));
        assert_eq!(tc.is_error, Some(false));
        assert!(tc
            .input_json
            .as_deref()
            .is_some_and(|j| j.contains("app.ts")));

        assert_eq!(s.file_events.len(), 1);
        let fe = &s.file_events[0];
        assert_eq!(fe.path, "src/app.ts");
        assert_eq!(fe.change_kind, FileChangeKind::Edit);
        assert_eq!(fe.source, FileEventSource::AgentToolInput);
        assert_eq!(fe.tool_native_call_id.as_deref(), Some("toolu_1"));
    }

    #[test]
    fn tool_result_error_flag_is_captured() {
        let a = ClaudeCodeAdapter::new();
        let content = concat!(
            "{\"type\":\"assistant\",\"uuid\":\"u1\",\"sessionId\":\"s\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"c9\",\"name\":\"Bash\",\"input\":{\"command\":\"npm test\"}},{\"type\":\"tool_use\",\"id\":\"c10\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo check\"}},{\"type\":\"tool_use\",\"id\":\"c11\",\"name\":\"Bash\",\"input\":{\"command\":\"echo ok\"}},{\"type\":\"tool_use\",\"id\":\"c12\",\"name\":\"Bash\",\"input\":{\"command\":\"ls\"}}]}}\n",
            "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"s\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"c9\",\"is_error\":true,\"content\":\"1 failing\"},{\"type\":\"tool_result\",\"tool_use_id\":\"c10\",\"is_error\":true,\"content\":null},{\"type\":\"tool_result\",\"tool_use_id\":\"c11\",\"is_error\":\"false\",\"content\":\"ok\"},{\"type\":\"tool_result\",\"tool_use_id\":\"c12\",\"is_error\":1,\"content\":\"files\"}]}}\n"
        );
        let s = a.parse_str(content, "fallback");
        assert_eq!(s.tool_calls.len(), 4);
        assert_eq!(s.tool_calls[0].is_error, Some(true));
        assert_eq!(s.tool_calls[0].output_text.as_deref(), Some("1 failing"));

        assert_eq!(s.tool_calls[1].is_error, Some(true));
        assert_eq!(s.tool_calls[1].output_text, None);

        assert_eq!(s.tool_calls[2].is_error, None);
        assert_eq!(s.tool_calls[2].output_text.as_deref(), Some("ok"));

        assert_eq!(s.tool_calls[3].is_error, None);
        assert_eq!(s.tool_calls[3].output_text.as_deref(), Some("files"));
        // Bash is not a file-mutating tool: no file event.
        assert!(s.file_events.is_empty());
    }

    #[test]
    fn orphan_tool_result_degrades_partial() {
        let a = ClaudeCodeAdapter::new();
        let content = "{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"missing\",\"content\":\"x\"}]}}\n";
        let s = a.parse_str(content, "fallback");
        assert_eq!(s.status, crate::model::ParseStatus::Partial);
        assert!(s.tool_calls.is_empty());
    }

    #[test]
    fn sanitize_path_neutralizes_traversal() {
        assert_eq!(super::sanitize_path("../../etc/passwd"), "etc/passwd");
        assert_eq!(super::sanitize_path(r"..\..\etc\passwd"), "etc/passwd");
        assert_eq!(super::sanitize_path("./src/../src/app.ts"), "src/app.ts");
        assert_eq!(super::sanitize_path(r".\src\..\src\app.ts"), "src/app.ts");
        assert_eq!(super::sanitize_path("/abs/path.rs"), "abs/path.rs");
        assert_eq!(super::sanitize_path(r"\abs\path.rs"), "abs/path.rs");
    }

    #[test]
    fn cwd_change_starts_a_new_segment() {
        let a = ClaudeCodeAdapter::new();
        let s = a.parse_str(&fixture("segments.jsonl"), "fallback");
        assert_eq!(s.segments.len(), 2, "cwd change splits segments");

        assert_eq!(s.segments[0].cwd.as_deref(), Some("/repo/app"));
        assert_eq!(s.segments[0].seq_start, 0);
        assert_eq!(s.segments[0].seq_end, 1);
        assert_eq!(s.segments[1].cwd.as_deref(), Some("/repo/lib"));
        assert_eq!(s.segments[1].seq_start, 2);
        assert_eq!(s.segments[1].seq_end, 3);

        assert_eq!(s.messages[0].segment_ix, 0);
        assert_eq!(s.messages[1].segment_ix, 0);
        assert_eq!(s.messages[2].segment_ix, 1);
        assert_eq!(s.messages[3].segment_ix, 1);
    }

    // Regression: a hardcoded recursion budget of 3 stopped at `subagents/` and
    // never reached `subagents/workflows/<wf-id>/`, silently dropping workflow
    // subagent transcripts. Discovery must reach every nesting level.
    #[test]
    fn discovers_workflow_subagent_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let project = root.join("encoded-repo");
        let session_dir = project.join("aaaaaaaa-0000-4000-8000-000000000001");
        let subagents = session_dir.join("subagents");
        let workflow = subagents.join("workflows/wf-0001");
        std::fs::create_dir_all(&workflow).unwrap();

        std::fs::write(
            project.join("aaaaaaaa-0000-4000-8000-000000000001.jsonl"),
            "{}\n",
        )
        .unwrap();
        std::fs::write(subagents.join("agent-1.jsonl"), "{}\n").unwrap();
        std::fs::write(workflow.join("agent-2.jsonl"), "{}\n").unwrap();

        let sessions = ClaudeCodeAdapter::new().discover_sessions(&DiscoveryRoots::new(vec![root]));
        let mut found: Vec<String> = sessions
            .iter()
            .filter_map(|s| s.path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec![
                "aaaaaaaa-0000-4000-8000-000000000001.jsonl",
                "agent-1.jsonl",
                "agent-2.jsonl",
            ]
        );
    }

    // Removing the depth ceiling must not reintroduce unbounded recursion: a
    // symlink pointing back at an ancestor must be skipped, not followed.
    #[cfg(unix)]
    #[test]
    fn discovery_does_not_follow_symlinked_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let project = root.join("encoded-repo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("session.jsonl"), "{}\n").unwrap();
        std::os::unix::fs::symlink(&root, project.join("loop")).unwrap();

        let sessions = ClaudeCodeAdapter::new().discover_sessions(&DiscoveryRoots::new(vec![root]));
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn extracts_notebook_edit_file_events() {
        let jsonl = r#"{"type":"user","message":{"role":"user","content":"edit notebook"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call_nb","name":"NotebookEdit","input":{"notebook_path":"analysis.ipynb"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_nb","content":["Cell 1 updated","Cell 2 updated"]}]}}
"#;
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "dedupe");
        assert_eq!(session.file_events.len(), 1);
        assert_eq!(session.file_events[0].path, "analysis.ipynb");
        assert_eq!(session.file_events[0].change_kind, FileChangeKind::Edit);

        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(
            session.tool_calls[0].output_text.as_deref(),
            Some("Cell 1 updated\nCell 2 updated")
        );

        // Tool result message part must also carry the parsed concatenated text.
        assert_eq!(session.messages[2].parts.len(), 1);
        assert_eq!(
            session.messages[2].parts[0].text.as_deref(),
            Some("Cell 1 updated\nCell 2 updated")
        );
    }

    #[test]
    fn parses_empty_and_null_content_blocks_cleanly() {
        let jsonl = concat!(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":null}}\n",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"\"}]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "empty-content");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 4);
        assert_eq!(session.messages[0].parts.len(), 1);
        assert_eq!(session.messages[0].parts[0].text.as_deref(), Some(""));
        assert_eq!(session.messages[1].parts.len(), 0);
        assert_eq!(session.messages[2].parts.len(), 0);
        assert_eq!(session.messages[3].parts.len(), 1);
        assert_eq!(session.messages[3].parts[0].text.as_deref(), Some(""));
    }

    #[test]
    fn parses_sidechain_subagent_messages_with_parent_uuids() {
        let jsonl = concat!(
            "{\"type\":\"user\",\"uuid\":\"u-root\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":\"launch subagent\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a-side\",\"parentUuid\":\"u-root\",\"sessionId\":\"s1\",\"isSidechain\":true,\"message\":{\"role\":\"assistant\",\"content\":\"subagent working\"}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "sidechain");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].native_uuid.as_deref(), Some("u-root"));
        assert_eq!(session.messages[0].parent_native_uuid, None);
        assert!(!session.messages[0].is_sidechain);

        assert_eq!(session.messages[1].native_uuid.as_deref(), Some("a-side"));
        assert_eq!(
            session.messages[1].parent_native_uuid.as_deref(),
            Some("u-root")
        );
        assert!(session.messages[1].is_sidechain);
    }

    #[test]
    fn parses_thinking_content_blocks_with_and_without_signatures() {
        let jsonl = "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"contemplating solution\",\"signature\":\"sig123\"},{\"type\":\"thinking\",\"thinking\":\"second thought\"}]}}\n";
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "thinking-blocks");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].parts.len(), 2);

        let part1 = &session.messages[0].parts[0];
        assert_eq!(part1.kind, PartKind::Thinking);
        assert_eq!(part1.text.as_deref(), Some("contemplating solution"));
        assert!(!part1.searchable);
        assert_eq!(
            part1.metadata_json.as_deref(),
            Some("{\"signature\":\"sig123\"}")
        );

        let part2 = &session.messages[0].parts[1];
        assert_eq!(part2.kind, PartKind::Thinking);
        assert_eq!(part2.text.as_deref(), Some("second thought"));
        assert!(!part2.searchable);
        assert_eq!(part2.metadata_json, None);
    }

    #[test]
    fn parses_tool_use_with_non_object_and_primitive_inputs() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"call_str\",\"name\":\"Bash\",\"input\":\"echo hello\"},",
            "{\"type\":\"tool_use\",\"id\":\"call_num\",\"name\":\"Compute\",\"input\":42},",
            "{\"type\":\"tool_use\",\"id\":\"call_null\",\"name\":\"Status\",\"input\":null},",
            "{\"type\":\"tool_use\",\"name\":\"MissingId\",\"input\":{}}",
            "]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "primitive-tools");
        assert_eq!(session.status, crate::model::ParseStatus::Partial);
        assert_eq!(session.tool_calls.len(), 3);

        assert_eq!(session.tool_calls[0].native_call_id, "call_str");
        assert_eq!(
            session.tool_calls[0].input_json.as_deref(),
            Some("\"echo hello\"")
        );

        assert_eq!(session.tool_calls[1].native_call_id, "call_num");
        assert_eq!(session.tool_calls[1].input_json.as_deref(), Some("42"));

        assert_eq!(session.tool_calls[2].native_call_id, "call_null");
        assert_eq!(session.tool_calls[2].input_json.as_deref(), Some("null"));
    }

    #[test]
    fn parses_tool_result_with_mixed_text_array_and_empty_elements() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"c_arr\",\"name\":\"Bash\",\"input\":{}}]}}\n",
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"c_arr\",\"content\":[\"stdout line 1\",{\"text\":\"stdout line 2\"},{\"invalid\":123}]}]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "tool-result-arrays");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(
            session.tool_calls[0].output_text.as_deref(),
            Some("stdout line 1\nstdout line 2")
        );
    }

    #[test]
    fn parses_file_mutating_tools_with_missing_and_empty_paths() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"c_empty\",\"name\":\"Edit\",\"input\":{\"file_path\":\"\"}},",
            "{\"type\":\"tool_use\",\"id\":\"c_nopath\",\"name\":\"Write\",\"input\":{\"content\":\"hello\"}},",
            "{\"type\":\"tool_use\",\"id\":\"c_nullinput\",\"name\":\"NotebookEdit\",\"input\":null}",
            "]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "empty-paths");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.tool_calls.len(), 3);
        assert_eq!(session.file_events.len(), 1);
        assert_eq!(
            session.file_events[0].tool_native_call_id.as_deref(),
            Some("c_empty")
        );
        assert_eq!(session.file_events[0].path, "");
    }

    #[test]
    fn parses_tool_use_with_missing_and_empty_name() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"c_noname\",\"input\":{}},",
            "{\"type\":\"tool_use\",\"id\":\"c_blankname\",\"name\":\"\",\"input\":{}},",
            "{\"type\":\"tool_use\",\"id\":\"c_numname\",\"name\":123,\"input\":{}},",
            "{\"type\":\"tool_use\",\"id\":\"c_boolname\",\"name\":true,\"input\":{}},",
            "{\"type\":\"tool_use\",\"id\":\"c_arrname\",\"name\":[\"Read\"],\"input\":{}}",
            "]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "empty-name-tools");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.tool_calls.len(), 5);
        assert_eq!(session.tool_calls[0].name, "");
        assert_eq!(session.tool_calls[1].name, "");
        assert_eq!(session.tool_calls[2].name, "");
        assert_eq!(session.tool_calls[3].name, "");
        assert_eq!(session.tool_calls[4].name, "");
    }

    #[test]
    fn tool_use_with_null_or_missing_id_degrades_to_partial() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":null,\"name\":\"Bash\",\"input\":{}},",
            "{\"type\":\"tool_use\",\"name\":\"Glob\",\"input\":{}}",
            "]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "null-id-tools");
        assert_eq!(session.status, crate::model::ParseStatus::Partial);
        assert!(session
            .notes
            .iter()
            .any(|n| n.message == "tool_use without id"));
    }

    #[test]
    fn parses_tool_use_with_extra_metadata_attributes() {
        let jsonl = "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"call_meta\",\"name\":\"Glob\",\"input\":{\"pattern\":\"*.rs\"},\"extra_field\":true,\"caller\":\"subagent_42\"}]}}\n";
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "extra-meta");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(session.tool_calls[0].native_call_id, "call_meta");
        assert_eq!(session.tool_calls[0].name, "Glob");
    }

    #[test]
    fn parses_user_message_with_empty_content_array() {
        let jsonl = "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":[]}}\n";
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "empty-user-content");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].parts.len(), 0);
    }

    #[test]
    fn parses_tool_result_with_is_error_flag() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"call_err\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"call_err\",\"content\":\"error[E0425]: cannot find value\",\"is_error\":true}]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "tool-err");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(session.tool_calls[0].is_error, Some(true));
        assert_eq!(
            session.tool_calls[0].output_text.as_deref(),
            Some("error[E0425]: cannot find value")
        );

        let jsonl_ok = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"call_ok\",\"name\":\"Bash\",\"input\":{\"command\":\"echo ok\"}}]}}\n",
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"call_ok\",\"content\":\"ok\",\"is_error\":false}]}}\n"
        );
        let session_ok = ClaudeCodeAdapter::new().parse_str(jsonl_ok, "tool-ok");
        assert_eq!(session_ok.status, crate::model::ParseStatus::Ok);
        assert_eq!(session_ok.tool_calls[0].is_error, Some(false));
    }

    #[test]
    fn parses_user_message_with_null_content_and_whitespace_tool_result() {
        let jsonl = concat!(
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":null}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"c_ws\",\"name\":\"Bash\",\"input\":{}}]}}\n",
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"c_ws\",\"content\":\"   \\n\\t  \"}]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "null-content-ws-result");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[0].parts.len(), 0);
        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(
            session.tool_calls[0].output_text.as_deref(),
            Some("   \n\t  ")
        );
    }

    #[test]
    fn parses_assistant_message_with_null_content_field() {
        let jsonl = "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":null}}\n";
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "null-assistant-content");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].parts.len(), 0);
    }

    #[test]
    fn claude_config_dir_environment_variable_overrides_default_root() {
        let temp = tempfile::tempdir().unwrap();
        let custom_dir = temp.path().join("custom_claude");
        std::env::set_var("CLAUDE_CONFIG_DIR", &custom_dir);
        let root = ClaudeCodeAdapter::default_root();
        std::env::remove_var("CLAUDE_CONFIG_DIR");

        assert_eq!(root, Some(custom_dir.join("projects")));
    }

    #[test]
    fn parses_stop_reason_and_sparse_token_usage() {
        let jsonl = concat!(
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet-20241022\",\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":150,\"output_tokens\":42},\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet-20241022\",\"stop_reason\":\"tool_use\",\"usage\":{\"cache_read_input_tokens\":80},\"content\":[{\"type\":\"text\",\"text\":\"calling tool\"}]}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "stop-reason-usage");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 2);

        assert_eq!(session.messages[0].stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(session.messages[0].tokens.input, Some(150));
        assert_eq!(session.messages[0].tokens.output, Some(42));
        assert_eq!(session.messages[0].tokens.cache, None);

        assert_eq!(session.messages[1].stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(session.messages[1].tokens.input, None);
        assert_eq!(session.messages[1].tokens.output, None);
        assert_eq!(session.messages[1].tokens.cache, Some(80));
    }

    #[test]
    fn parses_token_usage_with_null_and_mixed_fields() {
        let jsonl =
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":null,\"output_tokens\":200,\"cache_read_input_tokens\":\"invalid\"},\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n";
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "null-usage");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].tokens.input, None);
        assert_eq!(session.messages[0].tokens.output, Some(200));
        assert_eq!(session.messages[0].tokens.cache, None);
    }

    #[test]
    fn parses_message_with_null_and_non_array_content() {
        let jsonl =
            "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{\"role\":\"assistant\",\"content\":null}}\n{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":42}}\n";
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "non-array-content");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 2);
        assert!(session.messages[0].parts.is_empty());
        assert!(session.messages[1].parts.is_empty());
    }

    #[test]
    fn collect_jsonl_discovers_nested_workflow_and_deep_subagents() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        let sub_wf = root.join("subagents").join("workflows").join("wf_123");
        std::fs::create_dir_all(&sub_wf).unwrap();

        std::fs::write(root.join("main_session.jsonl"), b"{}\n").unwrap();
        std::fs::write(root.join("subagents").join("agent-1.jsonl"), b"{}\n").unwrap();
        std::fs::write(sub_wf.join("agent-sub-2.jsonl"), b"{}\n").unwrap();
        std::fs::write(sub_wf.join("ignore.txt"), b"not a jsonl\n").unwrap();

        let mut refs = Vec::new();
        collect_jsonl(root, &mut refs);
        assert_eq!(refs.len(), 3);

        let mut names: Vec<_> = refs.iter().filter_map(|r| r.native_id.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["agent-1", "agent-sub-2", "main_session"]);
    }

    #[test]
    fn parses_custom_and_ai_title_events_with_precedence_and_overrides() {
        let jsonl = concat!(
            "{\"type\":\"ai-title\",\"aiTitle\":\"Initial AI Title\"}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"User Custom Title\"}\n",
            "{\"type\":\"user\",\"sessionId\":\"s_title\",\"message\":{\"role\":\"user\",\"content\":\"my prompt\"}}\n"
        );
        let session = ClaudeCodeAdapter::new().parse_str(jsonl, "s_title");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.title.as_deref(), Some("User Custom Title"));
        assert!(!session.title_is_synthetic);
    }

    #[test]
    fn parses_title_events_ordering_and_fallback_synthetic_flag() {
        // AI title after custom title does not overwrite custom title
        let jsonl_custom_first = concat!(
            "{\"type\":\"custom-title\",\"customTitle\":\"Pinned Custom Title\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Late AI Title\"}\n",
            "{\"type\":\"user\",\"sessionId\":\"s1\",\"message\":{\"role\":\"user\",\"content\":\"some request\"}}\n"
        );
        let session1 = ClaudeCodeAdapter::new().parse_str(jsonl_custom_first, "s1");
        assert_eq!(session1.title.as_deref(), Some("Pinned Custom Title"));
        assert!(!session1.title_is_synthetic);

        // Fallback synthetic title from user prompt
        let jsonl_fallback =
            "{\"type\":\"user\",\"sessionId\":\"s2\",\"message\":{\"role\":\"user\",\"content\":\"Analyze database indexing strategy\"}}\n";
        let session2 = ClaudeCodeAdapter::new().parse_str(jsonl_fallback, "s2");
        assert_eq!(
            session2.title.as_deref(),
            Some("Analyze database indexing strategy")
        );
        assert!(session2.title_is_synthetic);
    }

    #[test]
    fn claude_code_adapter_capabilities_and_ignored_events() {
        let adapter = ClaudeCodeAdapter::new();
        assert_eq!(adapter.id(), AgentId("claude-code"));
        let meta = adapter.metadata();
        assert_eq!(meta.display_name, "Claude Code");
        assert_eq!(meta.format_id, "claude-code/jsonl");

        let caps = adapter.capabilities();
        assert!(caps.message_tree);
        assert!(caps.tool_calls);
        assert!(caps.file_events);

        // queue-operation and last-prompt events are ignored without notes or error
        let jsonl = concat!(
            "{\"type\":\"queue-operation\",\"operation\":\"enqueue\"}\n",
            "{\"type\":\"last-prompt\",\"prompt\":\"ignored\"}\n",
            "{\"type\":\"user\",\"sessionId\":\"s_ignore\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n"
        );
        let session = adapter.parse_str(jsonl, "s_ignore");
        assert_eq!(session.status, crate::model::ParseStatus::Ok);
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn claude_code_adapter_effective_roots_custom_vs_empty() {
        let adapter = ClaudeCodeAdapter::new();
        let custom_dir = PathBuf::from("/custom/claude/projects");
        let custom_roots = DiscoveryRoots::new(vec![custom_dir.clone()]);
        let roots = adapter.roots(&custom_roots);
        assert_eq!(roots, vec![custom_dir]);

        let empty_roots = DiscoveryRoots::new(vec![]);
        let default_roots = adapter.roots(&empty_roots);
        // Default roots derive from CLAUDE_CONFIG_DIR or HOME or empty
        assert!(default_roots.len() <= 1);
    }

    #[test]
    fn claude_code_adapter_detect_installation_with_custom_roots() {
        let adapter = ClaudeCodeAdapter::new();
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let roots = DiscoveryRoots::new(vec![projects_dir.clone()]);
        let detection = adapter.detect_installation(&roots);
        assert!(detection.installed);
        assert_eq!(detection.roots_found, vec![projects_dir]);

        let missing = temp.path().join("missing_projects");
        let missing_roots = DiscoveryRoots::new(vec![missing]);
        let missing_det = adapter.detect_installation(&missing_roots);
        assert!(!missing_det.installed);
        assert!(missing_det.roots_found.is_empty());
    }

    #[test]
    fn claude_code_ai_title_precedence_and_empty_content() {
        let adapter = ClaudeCodeAdapter;
        let jsonl = concat!(
            "{\"type\":\"ai-title\",\"aiTitle\":\"AI Generated Title\"}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"User Custom Title\"}\n",
            "{\"type\":\"user\",\"sessionId\":\"s_title\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n"
        );
        let session = adapter.parse_str(jsonl, "s_title");
        assert_eq!(session.title.as_deref(), Some("User Custom Title"));
        assert!(!session.title_is_synthetic);
        assert_eq!(session.messages.len(), 1);
    }
}
