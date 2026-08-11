//! Agent adapters: the only place agent-specific parsing lives.
//!
//! An adapter is **read-only** over an agent's on-disk sessions. It reports what
//! it can extract via [`Capabilities`] (so the UI/search degrade gracefully),
//! discovers candidate session files, and parses one into the normalized
//! [`crate::model::ParsedSession`]. Adapters never touch the network, never
//! mutate agent files, and never hard-fail on unknown input.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::model::ParsedSession;

/// Stable adapter identity, matching the `agent.id` primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentId(pub &'static str);

/// What an adapter can extract. Drives UI/search degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub messages: bool,
    pub thinking: bool,
    pub tool_calls: bool,
    pub file_events: bool,
    pub token_usage: bool,
    pub cost: bool,
    pub model_name: bool,
    pub summaries: bool,
    pub git_context: bool,
    pub message_tree: bool,
    pub durations: bool,
    /// Opaque/encrypted regions that must never be indexed or exported.
    pub encrypted_regions: bool,
}

/// Result of probing for an installation. Cheap and side-effect-free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    pub installed: bool,
    pub version: Option<String>,
    pub roots_found: Vec<PathBuf>,
}

/// Static adapter metadata (documentation + discovery hints).
#[derive(Debug, Clone)]
pub struct AgentMetadata {
    pub display_name: &'static str,
    pub format_id: &'static str,
    pub doc_link: &'static str,
}

/// Injectable discovery roots so tests point adapters at fixtures instead of a
/// real `~/.claude` / `~/.codex`.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryRoots {
    pub roots: Vec<PathBuf>,
}

impl DiscoveryRoots {
    #[must_use]
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }
}

/// A discovered-but-unparsed session file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub agent: AgentId,
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub size: u64,
    pub native_id: Option<String>,
}

/// Content-free adapter error (never embeds file content).
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("io error reading source")]
    Io,
    #[error("source unreadable or unsupported")]
    Unreadable,
}

/// The read-only interface every agent adapter implements.
pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> AgentId;
    fn metadata(&self) -> AgentMetadata;
    fn capabilities(&self) -> Capabilities;

    /// Probe for an installation under the given roots (or default roots when
    /// empty). Cheap; no parsing.
    fn detect_installation(&self, roots: &DiscoveryRoots) -> Detection;

    /// Enumerate candidate session files. Idempotent and cheap; no parsing.
    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef>;

    /// Parse one session file into the normalized model. Must be tolerant:
    /// unknown/partial input degrades to `ParseStatus::Partial`, never a panic.
    fn parse_session(&self, source: &SessionRef) -> Result<ParsedSession, AdapterError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_is_all_false() {
        let c = Capabilities::default();
        assert!(!c.messages && !c.tool_calls && !c.git_context && !c.encrypted_regions);
    }

    #[test]
    fn agent_id_is_comparable() {
        assert_eq!(AgentId("claude-code"), AgentId("claude-code"));
        assert_ne!(AgentId("claude-code"), AgentId("codex"));
    }
}
