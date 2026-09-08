//! # lore-core
//!
//! The local-first, git-aware archive core for coding-agent sessions.
//!
//! This crate is deliberately free of any GUI/Tauri dependency so the archive
//! logic (discovery, parsing, normalization, git evidence, storage, search,
//! secret scanning, jobs) is fully unit-testable with `cargo test` and never
//! links a webview.
//!
//! ## Hard invariants (see `docs/AGENTS.md`, `docs/architecture/*`)
//! - **No network capability in archive modules.** Only a separate, default-off
//!   updater may reach the network; it does not live in this crate.
//! - **Read-only** on all agent files.
//! - **Tolerant parsing:** never panic on untrusted input; degrade to a partial
//!   result with a bounded, content-free diagnostic.
//! - **Provenance is preserved:** agent-recorded vs Lore-observed git evidence,
//!   and opaque/encrypted regions, are never blurred together.

pub mod adapters;
pub mod backup;
pub mod discovery;
pub mod enrich;
pub mod export;
pub mod folders;
pub mod forget;
pub mod git;
pub mod hash;
pub mod ingest;
pub mod jobs;
pub mod landing;
pub mod lock;
pub mod model;
pub mod paths;
pub mod pipeline;
pub mod query;
pub mod recovery;
pub mod search;
pub mod secrets;
pub mod settings;
pub mod source_roots;
pub mod storage;
pub mod synthetic;
pub mod watcher;
pub mod worker;

use std::time::{SystemTime, UNIX_EPOCH};

/// The `lore-core` crate version (from `CARGO_PKG_VERSION`).
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Milliseconds since Unix epoch, saturating on clock skew.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Check if a character is an invisible zero-width Unicode codepoint.
#[must_use]
pub fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{feff}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}'
    )
}

/// Check whether a text token is invalid: empty, longer than `max_len`, or contains
/// control characters or zero-width codepoints.
#[must_use]
pub fn is_invalid_text_token(s: &str, max_len: usize) -> bool {
    s.is_empty() || s.len() > max_len || s.chars().any(|c| c.is_control() || is_zero_width(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_reported() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }

    #[test]
    fn is_zero_width_detects_all_zero_width_codepoints() {
        assert!(is_zero_width('\u{feff}'));
        assert!(is_zero_width('\u{200b}'));
        assert!(is_zero_width('\u{200c}'));
        assert!(is_zero_width('\u{200d}'));
        assert!(is_zero_width('\u{2060}'));
        assert!(!is_zero_width('a'));
        assert!(!is_zero_width(' '));
        assert!(!is_zero_width('\n'));
    }

    #[test]
    fn now_ms_reports_plausible_epoch_time() {
        let t = now_ms();
        assert!(t > 1_700_000_000_000, "clock before 2023: {t}");
    }

    #[test]
    fn test_is_invalid_text_token() {
        assert!(!is_invalid_text_token("valid_token_123", 64));
        assert!(is_invalid_text_token("", 64));
        assert!(is_invalid_text_token("invalid\x07", 64));
        assert!(is_invalid_text_token("invalid\u{200C}", 64));
        assert!(is_invalid_text_token(&"x".repeat(65), 64));
    }
}
