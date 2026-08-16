//! # lore-ipc
//!
//! The versioned data-transfer objects (DTOs) exchanged between the Lore Rust
//! core and the TypeScript UI over Tauri IPC. These Rust types are the **single
//! source of truth**: the files in `bindings/` are generated from them by
//! `ts-rs` (run `cargo test -p lore-ipc`) and must never be hand-edited.
//!
//! This crate has no Tauri, webview, or network dependency, so the contract can
//! be produced and unit-tested without a GUI. `i64` fields carry epoch-ms
//! timestamps and counts that stay within JS's safe-integer range; they are
//! declared as TypeScript `number` to match how serde serializes them to JSON.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// An agent adapter and whether it was detected on this machine.
/// Payload of the `list_detected_agents` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DetectedAgent {
    /// Stable adapter id, e.g. `claude-code` or `codex`.
    pub id: String,
    pub display_name: String,
    /// An installation/root was found.
    pub installed: bool,
    /// Detected agent version, when known.
    pub version: Option<String>,
    /// Sessions ingested for this agent so far.
    #[ts(type = "number")]
    pub session_count: i64,
    /// Effective local folders Lore checks for this adapter.
    pub roots: Vec<String>,
    /// User-selected folders within `roots`; these may be removed in Settings.
    pub custom_roots: Vec<String>,
}

/// A repository grouping for the Repositories view. Payload element of
/// `list_repositories`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepositorySummary {
    pub id: String,
    pub display_name: String,
    /// Identity confidence: `confirmed` | `high` | `medium` | `low`.
    pub identity_confidence: String,
    /// Latest local path hint, when known (may be display-sensitive).
    pub primary_path: Option<String>,
    /// The repository's checkout(s) are gone from disk.
    pub is_missing: bool,
    #[ts(type = "number")]
    pub session_count: i64,
    #[ts(type = "number")]
    pub worktree_count: i64,
}

/// A user-defined folder for organizing threads. Payload of `create_folder`
/// and element of `list_folders`. `session_count` is how many threads are filed in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderSummary {
    pub id: String,
    pub name: String,
    #[ts(type = "number")]
    pub session_count: i64,
    /// User ordering key; the list is sorted by this then name.
    #[ts(type = "number")]
    pub position: i64,
}

/// A one-line session for list views. Payload element of `list_sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    pub id: String,
    pub agent_id: String,
    pub title: Option<String>,
    /// Epoch milliseconds.
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
    /// Epoch milliseconds.
    #[ts(type = "number | null")]
    pub ended_at: Option<i64>,
    #[ts(type = "number")]
    pub message_count: i64,
    #[ts(type = "number")]
    pub tool_call_count: i64,
    pub primary_model: Option<String>,
    /// `ok` | `partial` | `failed`.
    pub parse_status: String,
}

/// One stable newest-first page for the repository/session browser. The
/// cursor is opaque to the UI and must be passed back unchanged. Payload of
/// `list_sessions_page` / `list_repository_sessions_page` / `list_folder_sessions_page`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionPage {
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
}

/// One ordered content block within a message. Opaque/encrypted blocks carry no
/// readable content (`text`/`content_json` are null) and are never rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessagePartDto {
    #[ts(type = "number")]
    pub ordinal: i64,
    /// `text` | `thinking` | `tool_use` | `tool_result` | `attachment` |
    /// `summary` | `opaque` | `other`.
    pub kind: String,
    pub text: Option<String>,
    /// Structured block payload as JSON text, when not plain text.
    pub content_json: Option<String>,
    /// False for opaque/encrypted and (by default) thinking blocks.
    pub searchable: bool,
}

/// One message/turn with its ordered parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageDto {
    pub id: String,
    #[ts(type = "number")]
    pub seq: i64,
    /// `user` | `assistant` | `system` | `tool` | `meta`.
    pub role: String,
    /// `message` | `summary` | `compaction` | `attachment` | `title` | `mode` |
    /// `pr_link` | `other`.
    pub event_kind: String,
    pub is_sidechain: bool,
    #[ts(type = "number | null")]
    pub ts: Option<i64>,
    pub model: Option<String>,
    pub parts: Vec<MessagePartDto>,
}

/// A context segment (cwd/model/provider valid for a seq range), with its
/// resolved repository link and confidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SegmentDto {
    pub id: String,
    #[ts(type = "number")]
    pub seq_start: i64,
    #[ts(type = "number")]
    pub seq_end: i64,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub repository_id: Option<String>,
    /// `high` | `medium` | `low` | `unresolved`.
    pub resolution_confidence: String,
}

/// A file the session touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileEventDto {
    /// Stable id; pass to `get_file_patch` to fetch the recorded patch.
    pub id: String,
    pub path: String,
    /// `edit` | `write` | `create` | `delete` | `move` | `read` | `patch`.
    pub change_kind: String,
    pub old_path: Option<String>,
    #[ts(type = "number | null")]
    pub lines_added: Option<i64>,
    #[ts(type = "number | null")]
    pub lines_removed: Option<i64>,
    /// Provenance: `agent_patch` | `agent_tool_input` | `lore_capture`.
    pub source: String,
    /// A byte-faithful recorded patch payload is stored and can be fetched.
    pub has_patch: bool,
}

/// One provenance-labeled git observation for the session's git rail. The UI
/// must keep `agent_recorded`, `lore_captured`, and `lore_reverified` visually
/// distinct and never collapse them into one unlabeled "session-time" rail.
/// Agent-recorded patches are not git observations — they surface as
/// `file_event.source = agent_patch` (see GIT_INTEGRATION.md §4). Payload
/// element of `get_git_snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitObservationDto {
    pub segment_id: Option<String>,
    /// `agent_recorded` | `agent_patch` | `lore_captured` | `lore_reverified`.
    pub source: String,
    /// Time claimed by the source event, when any.
    #[ts(type = "number | null")]
    pub event_ts: Option<i64>,
    /// When Lore read/verified it.
    #[ts(type = "number")]
    pub observed_at: i64,
    /// `exact_event` | `near_event` | `retrospective` | `current_only`.
    pub temporal_confidence: String,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    /// Credential-free normalized remote.
    pub remote_url_norm: Option<String>,
    pub is_dirty: Option<bool>,
    /// Re-verification result: the recorded commit still exists.
    pub commit_exists: Option<bool>,
}

/// The full read of one session in context. Payload of `get_session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    /// Bounded, content-free parser diagnostic. Detail-only so list pages stay
    /// compact while the opened session can explain partial/failed parsing.
    pub parse_note: Option<String>,
    pub segments: Vec<SegmentDto>,
    pub messages: Vec<MessageDto>,
    pub file_events: Vec<FileEventDto>,
}

/// Content-free progress for a running scan. Payload of the `scan_progress`
/// event — counts only, never session content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScanProgress {
    #[ts(type = "number")]
    pub discovered: i64,
    #[ts(type = "number")]
    pub ingested: i64,
    #[ts(type = "number")]
    pub skipped: i64,
    #[ts(type = "number")]
    pub failed: i64,
    #[ts(type = "number")]
    pub enriched: i64,
    /// The scan pass has finished.
    pub done: bool,
}

/// One search result over the redacted projections. `snippet` contains
/// highlight markers (`\u{e000}`…`\u{e001}`) around matched terms; the raw text
/// is redacted, so a flagged secret never appears. Payload element of `search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    pub session_id: String,
    /// `message_part` | `file_event` | `session`.
    pub source_kind: String,
    pub source_id: String,
    pub field: String,
    /// Redacted snippet with highlight markers around matches.
    pub snippet: String,
    /// FTS5 BM25 score (more negative = better match).
    #[ts(type = "number")]
    pub rank: f64,
    pub title: Option<String>,
    pub agent_id: String,
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
}

/// One page of search results plus an opaque keyset cursor. `next_cursor` is
/// `Some` only when a full page was returned (more results may follow); pass it
/// back verbatim as the `cursor` argument to fetch the next page. A cursor is
/// valid only for the identical query that produced it. Payload of `search_page`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub next_cursor: Option<String>,
}

/// The user-configurable automatic-backup schedule. `interval` is one of
/// `"off"`, `"daily"`, `"weekly"`; `keep` is how many newest backups to retain.
/// Payload of `get_backup_schedule` / argument of `set_backup_schedule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BackupScheduleDto {
    pub interval: String,
    #[ts(type = "number")]
    pub keep: i64,
}

/// Result of forgetting sessions or entire archive content. Payload of `forget_session`
/// and `forget_everything`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForgetReport {
    #[ts(type = "number")]
    pub blobs_removed: i64,
    /// Original agent source paths Lore does not own and did not delete.
    pub source_paths: Vec<String>,
}

/// Final tally returned by the `rescan` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RescanResult {
    #[ts(type = "number")]
    pub discovered: i64,
    #[ts(type = "number")]
    pub ingested: i64,
    #[ts(type = "number")]
    pub skipped: i64,
    #[ts(type = "number")]
    pub failed: i64,
    #[ts(type = "number")]
    pub enriched: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtos_round_trip_through_json() {
        let agent = DetectedAgent {
            id: "codex".into(),
            display_name: "Codex".into(),
            installed: true,
            version: None,
            session_count: 3,
            roots: vec!["/Users/example/.codex/sessions".into()],
            custom_roots: vec![],
        };
        let json = serde_json::to_string(&agent).unwrap();
        assert_eq!(serde_json::from_str::<DetectedAgent>(&json).unwrap(), agent);
    }

    #[test]
    fn timestamps_serialize_as_plain_json_numbers() {
        // The TS contract declares these `number`; serde must agree.
        let summary = SessionSummary {
            id: "s".into(),
            agent_id: "codex".into(),
            title: None,
            started_at: Some(1_700_000_000_000),
            ended_at: None,
            message_count: 2,
            tool_call_count: 0,
            primary_model: None,
            parse_status: "ok".into(),
        };
        let value: serde_json::Value = serde_json::to_value(&summary).unwrap();
        assert!(value["started_at"].is_number());
        assert!(value["ended_at"].is_null());
    }
}
