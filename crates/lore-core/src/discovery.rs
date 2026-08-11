//! Filesystem discovery of agent session source artifacts.
//!
//! Discovery is adapter-driven and read-only. Root overrides are injectable so
//! automated tests never inspect a developer's real agent history. Results are
//! deduplicated and sorted to make first scans deterministic.

use std::collections::{BTreeMap, BTreeSet};

use crate::adapters::{AdapterRegistry, AgentMetadata, Detection, DiscoveryRoots, SessionRef};

/// Per-adapter discovery roots. An absent override asks the adapter to use its
/// documented default; an explicit empty root list intentionally does the same.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryConfig {
    roots: BTreeMap<String, DiscoveryRoots>,
}

impl DiscoveryConfig {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_roots(&mut self, agent_id: impl Into<String>, roots: DiscoveryRoots) {
        self.roots.insert(agent_id.into(), roots);
    }

    fn roots_for(&self, agent_id: &str) -> DiscoveryRoots {
        self.roots.get(agent_id).cloned().unwrap_or_default()
    }
}

/// Installation probe for one registered adapter.
#[derive(Debug, Clone)]
pub struct AgentDiscovery {
    pub id: &'static str,
    pub metadata: AgentMetadata,
    pub detection: Detection,
}

/// Stable snapshot of one discovery pass.
#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    pub agents: Vec<AgentDiscovery>,
    pub sessions: Vec<SessionRef>,
}

/// Probe every registered adapter and enumerate candidate session files.
#[must_use]
pub fn discover(registry: &AdapterRegistry, config: &DiscoveryConfig) -> DiscoveryReport {
    let mut agents = Vec::with_capacity(registry.len());
    let mut sessions = Vec::new();

    for adapter in registry.iter() {
        let roots = config.roots_for(adapter.id().0);
        agents.push(AgentDiscovery {
            id: adapter.id().0,
            metadata: adapter.metadata(),
            detection: adapter.detect_installation(&roots),
        });
        sessions.extend(adapter.discover_sessions(&roots));
    }

    agents.sort_by_key(|agent| agent.id);
    sessions.sort_by(|left, right| {
        left.agent
            .0
            .cmp(right.agent.0)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut seen = BTreeSet::new();
    sessions.retain(|session| seen.insert((session.agent.0, session.path.clone())));

    DiscoveryReport { agents, sessions }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn v0_discovery_is_injectable_deduplicated_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let claude_root = dir.path().join("claude-projects");
        let claude_project = claude_root.join("encoded-repo");
        let codex_root = dir.path().join("codex-sessions");
        let codex_day = codex_root.join("2026/08/11");
        fs::create_dir_all(&claude_project).unwrap();
        fs::create_dir_all(&codex_day).unwrap();

        let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        fs::copy(
            fixture_root.join("claude_code/basic_text.jsonl"),
            claude_project.join("session.jsonl"),
        )
        .unwrap();
        fs::copy(
            fixture_root.join("codex/minimal.jsonl"),
            codex_day.join("rollout-test.jsonl"),
        )
        .unwrap();

        let mut config = DiscoveryConfig::new();
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![claude_root.clone(), claude_root]),
        );
        config.set_roots("codex", DiscoveryRoots::new(vec![codex_root]));

        let report = discover(&AdapterRegistry::v0(), &config);
        assert_eq!(report.agents.len(), 2);
        assert!(report.agents.iter().all(|agent| agent.detection.installed));
        assert_eq!(report.sessions.len(), 2, "overlapping roots must dedupe");
        assert_eq!(report.sessions[0].agent.0, "claude-code");
        assert_eq!(report.sessions[1].agent.0, "codex");
        assert!(report.sessions[1]
            .path
            .file_name()
            .is_some_and(|name| name == "rollout-test.jsonl"));
    }
}
