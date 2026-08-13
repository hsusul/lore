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
pub mod discovery;
pub mod enrich;
pub mod export;
pub mod forget;
pub mod git;
pub mod ingest;
pub mod jobs;
pub mod model;
pub mod pipeline;
pub mod query;
pub mod search;
pub mod secrets;
pub mod storage;
pub mod synthetic;
pub mod watcher;
pub mod worker;

/// The `lore-core` crate version (from `CARGO_PKG_VERSION`).
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_reported() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}
