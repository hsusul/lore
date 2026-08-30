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

impl std::fmt::Display for ParseStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

impl std::fmt::Display for PartKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

impl std::fmt::Display for FileChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

impl std::fmt::Display for FileEventSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
        assert_eq!(format!("{}", Role::Assistant), "assistant");
        assert_eq!(PartKind::ToolResult.as_str(), "tool_result");
        assert_eq!(format!("{}", PartKind::ToolResult), "tool_result");
        assert_eq!(EventKind::PrLink.as_str(), "pr_link");
        assert_eq!(format!("{}", EventKind::PrLink), "pr_link");
        assert_eq!(FileChangeKind::Patch.as_str(), "patch");
        assert_eq!(format!("{}", FileChangeKind::Patch), "patch");
        assert_eq!(FileEventSource::AgentPatch.as_str(), "agent_patch");
        assert_eq!(format!("{}", FileEventSource::AgentPatch), "agent_patch");
        assert_eq!(ParseStatus::Partial.as_str(), "partial");
        assert_eq!(format!("{}", ParseStatus::Partial), "partial");
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

    #[test]
    fn tokens_is_empty_handles_zero_values_and_partial_initialization() {
        let zero_input = Tokens {
            input: Some(0),
            ..Default::default()
        };
        assert!(!zero_input.is_empty());

        let zero_output = Tokens {
            output: Some(0),
            ..Default::default()
        };
        assert!(!zero_output.is_empty());

        let zero_cache = Tokens {
            cache: Some(0),
            ..Default::default()
        };
        assert!(!zero_cache.is_empty());
    }

    #[test]
    fn all_domain_enums_have_consistent_as_str_and_display_implementations() {
        for status in [ParseStatus::Ok, ParseStatus::Partial, ParseStatus::Failed] {
            assert_eq!(status.as_str(), status.to_string().as_str());
        }

        for role in [
            Role::User,
            Role::Assistant,
            Role::System,
            Role::Tool,
            Role::Meta,
        ] {
            assert_eq!(role.as_str(), role.to_string().as_str());
        }

        for kind in [
            EventKind::Message,
            EventKind::Summary,
            EventKind::Compaction,
            EventKind::Attachment,
            EventKind::Title,
            EventKind::Mode,
            EventKind::PrLink,
            EventKind::Other,
        ] {
            assert_eq!(kind.as_str(), kind.to_string().as_str());
        }

        for part_kind in [
            PartKind::Text,
            PartKind::Thinking,
            PartKind::ToolUse,
            PartKind::ToolResult,
            PartKind::Attachment,
            PartKind::Opaque,
        ] {
            assert_eq!(part_kind.as_str(), part_kind.to_string().as_str());
        }

        for change_kind in [
            FileChangeKind::Edit,
            FileChangeKind::Write,
            FileChangeKind::Create,
            FileChangeKind::Delete,
            FileChangeKind::Move,
            FileChangeKind::Read,
            FileChangeKind::Patch,
        ] {
            assert_eq!(change_kind.as_str(), change_kind.to_string().as_str());
        }

        for source in [
            FileEventSource::AgentToolInput,
            FileEventSource::AgentPatch,
            FileEventSource::LoreCapture,
        ] {
            assert_eq!(source.as_str(), source.to_string().as_str());
        }
    }

    #[test]
    fn parsed_session_new_defaults_and_note_collection() {
        let mut session = ParsedSession::new("test_dedupe_key");
        assert_eq!(session.dedupe_key, "test_dedupe_key");
        assert_eq!(session.status, ParseStatus::Ok);
        assert!(!session.title_is_synthetic);
        assert!(session.notes.is_empty());
        assert!(session.messages.is_empty());
        assert!(session.segments.is_empty());
        assert!(session.tool_calls.is_empty());
        assert!(session.file_events.is_empty());

        session.note_partial("first note");
        session.note_partial("second note");
        assert_eq!(session.status, ParseStatus::Partial);
        assert_eq!(session.notes.len(), 2);
        assert_eq!(session.notes[0].message, "first note");
        assert_eq!(session.notes[1].message, "second note");
    }

    #[test]
    fn token_counts_and_parsed_message_helpers() {
        let tc_empty = Tokens::default();
        assert!(tc_empty.is_empty());

        let tc = Tokens {
            input: Some(100),
            output: Some(50),
            cache: Some(10),
        };
        assert!(!tc.is_empty());
        let tc_clone = tc;
        assert_eq!(tc, tc_clone);

        let msg = ParsedMessage {
            seq: 1,
            segment_ix: 0,
            native_uuid: Some("msg_1".to_string()),
            parent_native_uuid: None,
            role: Role::User,
            event_kind: EventKind::Message,
            is_sidechain: false,
            ts: Some(1000),
            model: Some("claude-3-5-sonnet".to_string()),
            tokens: tc,
            stop_reason: None,
            source_offset: None,
            metadata_json: None,
            parts: vec![],
        };
        let msg_clone = msg.clone();
        assert_eq!(msg.native_uuid, msg_clone.native_uuid);
        assert_eq!(msg.role, Role::User);
    }

    #[test]
    fn parsed_tool_call_and_file_event_clones() {
        let tc = ParsedToolCall {
            native_call_id: "call_1".to_string(),
            name: "read_file".to_string(),
            call_ref: (1, 0),
            result_ref: Some((2, 0)),
            input_json: Some("{\"path\":\"test.rs\"}".to_string()),
            output_text: Some("file content".to_string()),
            is_error: Some(false),
            duration_ms: Some(150),
        };
        let tc_clone = tc.clone();
        assert_eq!(tc.native_call_id, tc_clone.native_call_id);
        assert_eq!(tc.name, tc_clone.name);
        assert_eq!(tc.duration_ms, Some(150));

        let fe = ParsedFileEvent {
            segment_ix: 0,
            tool_native_call_id: Some("call_1".to_string()),
            path: "test.rs".to_string(),
            change_kind: FileChangeKind::Edit,
            old_path: None,
            lines_added: Some(10),
            lines_removed: Some(2),
            patch_text: Some("diff content".to_string()),
            source: FileEventSource::AgentPatch,
            event_ts: Some(123456789),
        };
        let fe_clone = fe.clone();
        assert_eq!(fe.path, fe_clone.path);
        assert_eq!(fe.change_kind, fe_clone.change_kind);
        assert_eq!(fe.lines_added, Some(10));
    }
}
