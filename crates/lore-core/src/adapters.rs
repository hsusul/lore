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

pub(crate) mod common;

pub mod claude_code;
pub mod codex;

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

    /// The effective roots this adapter scans under the given overrides (its
    /// documented defaults when the overrides are empty). Used to set up
    /// filesystem watches and to resolve which adapter owns an observed path.
    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf>;

    /// Enumerate candidate session files. Idempotent and cheap; no parsing.
    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef>;

    /// Parse already-read session content into the normalized model. This is the
    /// bounded interface the ingest layer prefers: a caller that has already read
    /// the source bytes (for hashing/checkpointing) passes them straight in,
    /// avoiding a second whole-file read. Deterministic and side-effect-free;
    /// tolerant, so unknown/partial input degrades to `ParseStatus::Partial`
    /// rather than panicking. `fallback_dedupe` seeds the dedupe key when the
    /// source carries no native session id.
    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession;

    /// Parse one session file into the normalized model. The default reads the
    /// file once and delegates to [`AgentAdapter::parse_content`]; adapters need
    /// not override it.
    fn parse_session(&self, source: &SessionRef) -> Result<ParsedSession, AdapterError> {
        let content = std::fs::read_to_string(&source.path).map_err(|_| AdapterError::Io)?;
        let fallback = source
            .native_id
            .clone()
            .unwrap_or_else(|| source.path.to_string_lossy().into_owned());
        Ok(self.parse_content(&content, &fallback))
    }
}

/// Registered adapter set. Registration rejects duplicate stable ids so a
/// discovered source can always be routed to exactly one parser.
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn AgentAdapter>>,
}

impl AdapterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    /// The native V0 adapter set, in stable display order.
    #[must_use]
    pub fn v0() -> Self {
        Self {
            adapters: vec![
                Box::new(claude_code::ClaudeCodeAdapter::new()),
                Box::new(codex::CodexAdapter::new()),
            ],
        }
    }

    /// Add an adapter, rejecting a second implementation for the same id.
    pub fn register(&mut self, adapter: Box<dyn AgentAdapter>) -> Result<(), RegistryError> {
        let id = adapter.id().0;
        if self.get(id).is_some() {
            return Err(RegistryError::DuplicateId(id));
        }
        self.adapters.push(adapter);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&dyn AgentAdapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.id().0 == id)
            .map(Box::as_ref)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn AgentAdapter> {
        self.adapters.iter().map(Box::as_ref)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry construction failure. Adapter ids are static schema identifiers,
/// so including one in a diagnostic cannot expose session content.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("duplicate adapter id: {0}")]
    DuplicateId(&'static str),
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

    #[test]
    fn v0_registry_routes_each_native_adapter() {
        let registry = AdapterRegistry::v0();
        assert_eq!(registry.len(), 2);
        assert!(registry.get("claude-code").is_some());
        assert!(registry.get("codex").is_some());
        assert!(registry.get("unknown").is_none());
    }

    #[test]
    fn duplicate_adapter_ids_are_rejected() {
        let mut registry = AdapterRegistry::new();
        registry
            .register(Box::new(claude_code::ClaudeCodeAdapter::new()))
            .unwrap();
        let error = registry
            .register(Box::new(claude_code::ClaudeCodeAdapter::new()))
            .unwrap_err();
        assert_eq!(error, RegistryError::DuplicateId("claude-code"));
    }
}
