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
