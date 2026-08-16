//! Normalized domain model produced by adapters and persisted by the ingest
//! layer (see `docs/architecture/DATA_MODEL.md`).
//!
//! Adapters parse an agent's on-disk session into a [`ParsedSession`]: an
//! in-memory, faithful, ordered representation. Persistence maps it onto the
//! SQLite schema. Enum string forms match the values stored in the database.

/// Outcome of parsing a session. Never `Failed` for merely-unknown events —
/// unknown/partial input degrades to `Partial` with a bounded note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    Ok,
    Partial,
    Failed,
}

impl ParseStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ParseStatus::Ok => "ok",
            ParseStatus::Partial => "partial",
            ParseStatus::Failed => "failed",
        }
    }
}

/// Message author role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
    Meta,
}

impl Role {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
            Role::Meta => "meta",
        }
    }
}

/// The kind of event a message envelope represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Message,
    Summary,
    Compaction,
    Attachment,
    Title,
    Mode,
    PrLink,
    Other,
}

impl EventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Message => "message",
            EventKind::Summary => "summary",
            EventKind::Compaction => "compaction",
            EventKind::Attachment => "attachment",
            EventKind::Title => "title",
            EventKind::Mode => "mode",
            EventKind::PrLink => "pr_link",
            EventKind::Other => "other",
        }
    }
}

/// Kind of an ordered content block within a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
    Attachment,
    Summary,
    Opaque,
    Other,
}

impl PartKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PartKind::Text => "text",
            PartKind::Thinking => "thinking",
            PartKind::ToolUse => "tool_use",
            PartKind::ToolResult => "tool_result",
            PartKind::Attachment => "attachment",
            PartKind::Summary => "summary",
            PartKind::Opaque => "opaque",
            PartKind::Other => "other",
        }
    }
}

/// How a file was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Edit,
    Write,
    Create,
    Delete,
    Move,
    Read,
    Patch,
}

impl FileChangeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FileChangeKind::Edit => "edit",
            FileChangeKind::Write => "write",
            FileChangeKind::Create => "create",
            FileChangeKind::Delete => "delete",
            FileChangeKind::Move => "move",
            FileChangeKind::Read => "read",
            FileChangeKind::Patch => "patch",
        }
    }
}

/// Provenance of a file event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventSource {
    AgentPatch,
    AgentToolInput,
    LoreCapture,
}

impl FileEventSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FileEventSource::AgentPatch => "agent_patch",
            FileEventSource::AgentToolInput => "agent_tool_input",
            FileEventSource::LoreCapture => "lore_capture",
        }
    }
}

/// Per-turn token counts, when the agent records them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache: Option<i64>,
}

impl Tokens {
    /// True when no token counts are recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.input.is_none() && self.output.is_none() && self.cache.is_none()
    }
}

/// One ordered content block within a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPart {
    pub ordinal: i64,
    pub kind: PartKind,
    /// Small canonical text (large payloads offload to a blob during persist).
    pub text: Option<String>,
    /// Structured block payload as JSON text when not plain text.
    pub content_json: Option<String>,
    /// Excluded from search by default for opaque/encrypted and thinking blocks.
    pub searchable: bool,
    /// Bounded extras (e.g. a thinking signature); never flattened away.
    pub metadata_json: Option<String>,
}

/// One event/turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub seq: i64,
    pub segment_ix: usize,
    pub native_uuid: Option<String>,
    pub parent_native_uuid: Option<String>,
    pub role: Role,
    pub event_kind: EventKind,
    pub is_sidechain: bool,
    pub ts: Option<i64>,
    pub model: Option<String>,
    pub tokens: Tokens,
    pub stop_reason: Option<String>,
    pub source_offset: Option<i64>,
    pub metadata_json: Option<String>,
    pub parts: Vec<ParsedPart>,
}

/// A tool invocation and (optionally) its result, referenced by source blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToolCall {
    pub native_call_id: String,
    pub name: String,
    /// Ordinal path to the invocation block: (message seq, part ordinal).
    pub call_ref: (i64, i64),
    pub result_ref: Option<(i64, i64)>,
    pub input_json: Option<String>,
    pub output_text: Option<String>,
    pub is_error: Option<bool>,
    pub duration_ms: Option<i64>,
}

/// A file the session touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFileEvent {
    pub segment_ix: usize,
    pub tool_native_call_id: Option<String>,
    pub path: String,
    pub change_kind: FileChangeKind,
    pub old_path: Option<String>,
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
    /// Byte-faithful recorded patch/diff content. Offloaded to a content-
    /// addressed blob during persist and referenced by `file_event.patch_blob_id`.
    /// `None` when the source only names the file (e.g. a tool input).
    pub patch_text: Option<String>,
    pub source: FileEventSource,
    pub event_ts: Option<i64>,
}

/// Context valid for an inclusive `seq` range within a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSegment {
    pub seq_start: i64,
    pub seq_end: i64,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Recorded git branch (agent-recorded observation is created during persist).
    pub git_branch: Option<String>,
    pub git_commit_sha: Option<String>,
    pub git_remote_url: Option<String>,
}

/// A bounded, content-free parser diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseNote {
    pub message: String,
}

/// Faithful in-memory result of parsing one session file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSession {
    pub native_session_id: Option<String>,
    pub dedupe_key: String,
    pub title: Option<String>,
    /// True when `title` was derived from message content as a display fallback
    /// (no native `custom-title`/`ai-title` event). A synthetic title merely
    /// echoes the first user message, so it is kept for display but never
    /// scanned or indexed — doing so would duplicate the message's own secret
    /// findings and search hits (`SEARCH.md` §6, "without duplicates").
    pub title_is_synthetic: bool,
    pub agent_version: Option<String>,
    pub primary_model: Option<String>,
    /// Session-level totals when the source records cumulative usage. When a
    /// field is absent, persistence falls back to summing per-message usage.
    pub total_tokens: Tokens,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub status: ParseStatus,
    pub note: Option<String>,
    pub segments: Vec<ParsedSegment>,
    pub messages: Vec<ParsedMessage>,
    pub tool_calls: Vec<ParsedToolCall>,
    pub file_events: Vec<ParsedFileEvent>,
    pub notes: Vec<ParseNote>,
}

impl ParsedSession {
    /// A new empty session shell with a deterministic dedupe key.
    #[must_use]
    pub fn new(dedupe_key: impl Into<String>) -> Self {
        ParsedSession {
            native_session_id: None,
            dedupe_key: dedupe_key.into(),
            title: None,
            title_is_synthetic: false,
            agent_version: None,
            primary_model: None,
            total_tokens: Tokens::default(),
            started_at: None,
            ended_at: None,
            status: ParseStatus::Ok,
            note: None,
            segments: Vec::new(),
            messages: Vec::new(),
            tool_calls: Vec::new(),
            file_events: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Record a bounded, content-free note and mark the parse partial.
    pub fn note_partial(&mut self, message: impl Into<String>) {
        if self.status == ParseStatus::Ok {
            self.status = ParseStatus::Partial;
        }
        self.notes.push(ParseNote {
            message: message.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_string_forms_match_schema() {
        assert_eq!(Role::Assistant.as_str(), "assistant");
        assert_eq!(PartKind::ToolResult.as_str(), "tool_result");
        assert_eq!(EventKind::PrLink.as_str(), "pr_link");
        assert_eq!(FileChangeKind::Patch.as_str(), "patch");
        assert_eq!(FileEventSource::AgentPatch.as_str(), "agent_patch");
        assert_eq!(ParseStatus::Partial.as_str(), "partial");
    }

    #[test]
    fn note_partial_downgrades_status_once() {
        let mut s = ParsedSession::new("k");
        assert_eq!(s.status, ParseStatus::Ok);
        s.note_partial("unknown event type");
        assert_eq!(s.status, ParseStatus::Partial);
        s.note_partial("another");
        assert_eq!(s.status, ParseStatus::Partial);
        assert_eq!(s.notes.len(), 2);
    }

    #[test]
    fn tokens_is_empty_checks_all_fields() {
        let empty = Tokens::default();
        assert!(empty.is_empty());

        let with_input = Tokens {
            input: Some(10),
            ..Default::default()
        };
        assert!(!with_input.is_empty());

        let with_output = Tokens {
            output: Some(5),
            ..Default::default()
        };
        assert!(!with_output.is_empty());

        let with_cache = Tokens {
            cache: Some(20),
            ..Default::default()
        };
        assert!(!with_cache.is_empty());
    }
}
