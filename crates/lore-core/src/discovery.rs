//! Filesystem discovery of agent session source artifacts.
//!
//! Discovery is adapter-driven and read-only. Root overrides are injectable so
//! automated tests never inspect a developer's real agent history. Results are
//! deduplicated and sorted to make first scans deterministic.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::adapters::{
    AdapterRegistry, AgentId, AgentMetadata, Detection, DiscoveryRoots, SessionRef,
};

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

    /// Configured roots for one adapter. An empty value asks the adapter to use
    /// its documented defaults.
    #[must_use]
    pub fn roots_for(&self, agent_id: &str) -> DiscoveryRoots {
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

/// The union of every registered adapter's effective roots — the set a
/// filesystem watcher should observe. Deduplicated and sorted for determinism.
#[must_use]
pub fn watch_roots(registry: &AdapterRegistry, config: &DiscoveryConfig) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = registry
        .iter()
        .flat_map(|adapter| adapter.roots(&config.roots_for(adapter.id().0)))
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// Resolve which adapter owns an observed path by matching it against each
/// adapter's effective roots (longest matching root wins, so nested roots
/// resolve deterministically). Returns `None` for a path under no known root.
///
/// Matching is done on **canonicalized** forms so a symlinked prefix does not
/// break resolution: OS filesystem watchers (macOS FSEvents in particular) emit
/// fully resolved paths (`/private/var/...`), while a configured root may still
/// carry an unresolved prefix (`/var/...`, `/tmp/...`, or a symlinked home). A
/// raw `starts_with` would silently miss those, dropping live updates.
/// Canonicalization falls back to the original path when it cannot be resolved
/// (e.g. a not-yet-created root), preserving the lexical behavior for those.
#[must_use]
pub fn owner_of(
    registry: &AdapterRegistry,
    config: &DiscoveryConfig,
    path: &Path,
) -> Option<AgentId> {
    // Only the observed path's canonical form is resolvable when it exists (a
    // real watcher event); a missing/hypothetical path stays lexical. Match on
    // whichever agrees so both raw roots (missing peers) and symlinked roots
    // (canonical watcher paths) resolve.
    let path_canon = std::fs::canonicalize(path).ok();
    let mut best: Option<(usize, AgentId)> = None;
    for adapter in registry.iter() {
        for root in adapter.roots(&config.roots_for(adapter.id().0)) {
            let matches_raw = path.starts_with(&root);
            let matches_canon = match (&path_canon, std::fs::canonicalize(&root).ok()) {
                (Some(p), Some(r)) => p.starts_with(&r),
                _ => false,
            };
            if matches_raw || matches_canon {
                let depth = root.components().count();
                if best.is_none_or(|(d, _)| depth > d) {
                    best = Some((depth, adapter.id()));
                }
            }
        }
    }
    best.map(|(_, id)| id)
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

    #[test]
    fn owner_of_resolves_paths_and_watch_roots_are_deduped() {
        let claude_root = std::path::PathBuf::from("/home/u/.claude/projects");
        let codex_root = std::path::PathBuf::from("/home/u/.codex/sessions");
        let mut config = DiscoveryConfig::new();
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![claude_root.clone()]),
        );
        config.set_roots("codex", DiscoveryRoots::new(vec![codex_root.clone()]));
        let registry = AdapterRegistry::v0();

        assert_eq!(
            owner_of(&registry, &config, &claude_root.join("repo/session.jsonl")).map(|a| a.0),
            Some("claude-code")
        );
        assert_eq!(
            owner_of(
                &registry,
                &config,
                &codex_root.join("2026/08/11/rollout-x.jsonl")
            )
            .map(|a| a.0),
            Some("codex")
        );
        assert!(owner_of(
            &registry,
            &config,
            std::path::Path::new("/tmp/elsewhere.jsonl")
        )
        .is_none());

        let roots = watch_roots(&registry, &config);
        assert_eq!(roots, vec![claude_root, codex_root]);
    }

    // Regression: OS watchers emit canonicalized paths. If the configured root
    // carries a symlinked prefix, a raw `starts_with` misses the event and live
    // ingestion silently stalls. Build a real symlink and assert a canonicalized
    // child path still resolves to the owning adapter.
    #[cfg(unix)]
    #[test]
    fn owner_of_matches_across_a_symlinked_root_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real/codex/sessions");
        fs::create_dir_all(real.join("2026/08/11")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path().join("real"), &link).unwrap();

        // A real event fires for a file that exists; the watcher reports it by
        // its canonical path. Root is spelled through the symlink.
        let live = real.join("2026/08/11/rollout-live.jsonl");
        fs::write(&live, "{}\n").unwrap();
        let root_via_link = link.join("codex/sessions");
        let observed = live.canonicalize().unwrap();

        let mut config = DiscoveryConfig::new();
        config.set_roots("codex", DiscoveryRoots::new(vec![root_via_link]));
        assert_eq!(
            owner_of(&AdapterRegistry::v0(), &config, &observed).map(|a| a.0),
            Some("codex"),
            "a canonical event path must resolve through a symlinked root"
        );
    }

    #[test]
    fn discovery_tolerates_non_existent_custom_roots() {
        let mut config = DiscoveryConfig::new();
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![std::path::PathBuf::from("/non/existent/claude/root")]),
        );
        config.set_roots(
            "codex",
            DiscoveryRoots::new(vec![std::path::PathBuf::from("/non/existent/codex/root")]),
        );

        let report = discover(&AdapterRegistry::v0(), &config);
        assert_eq!(report.agents.len(), 2);
        assert!(!report.agents[0].detection.installed);
        assert!(!report.agents[1].detection.installed);
        assert!(report.sessions.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_traversal_tolerates_broken_symlinks_and_hidden_entries() {
        let dir = tempfile::tempdir().unwrap();
        let claude_root = dir.path().join("claude_projects");
        let codex_root = dir.path().join("codex_sessions");
        fs::create_dir_all(&claude_root).unwrap();
        fs::create_dir_all(&codex_root).unwrap();

        // Add valid sessions
        fs::write(claude_root.join("valid-session.jsonl"), "{}\n").unwrap();
        fs::write(codex_root.join("rollout-2026-08-29-valid.jsonl"), "{}\n").unwrap();

        // Add broken symlinks
        let broken_target = dir.path().join("non_existent_target");
        std::os::unix::fs::symlink(&broken_target, claude_root.join("broken_link.jsonl")).unwrap();
        std::os::unix::fs::symlink(&broken_target, codex_root.join("rollout-broken.jsonl"))
            .unwrap();

        // Add hidden subdirectories and ignored extensions
        let hidden_dir = claude_root.join(".hidden");
        fs::create_dir_all(&hidden_dir).unwrap();
        fs::write(hidden_dir.join("ignore_me.txt"), "text\n").unwrap();

        let mut config = DiscoveryConfig::new();
        config.set_roots("claude-code", DiscoveryRoots::new(vec![claude_root]));
        config.set_roots("codex", DiscoveryRoots::new(vec![codex_root]));

        let report = discover(&AdapterRegistry::v0(), &config);
        assert_eq!(report.agents.len(), 2);
        assert!(report.agents[0].detection.installed);
        assert!(report.agents[1].detection.installed);

        // Discovery completes cleanly finding only real session files
        let session_names: Vec<_> = report
            .sessions
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(session_names.contains(&"valid-session.jsonl".to_string()));
        assert!(session_names.contains(&"rollout-2026-08-29-valid.jsonl".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_traversal_on_nested_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let claude_root = dir.path().join("claude_projects");
        let codex_root = dir.path().join("codex_sessions");

        let claude_nested = claude_root.join(".hidden/sub/.another_hidden");
        let codex_nested = codex_root.join(".hidden/2026/08/29");
        fs::create_dir_all(&claude_nested).unwrap();
        fs::create_dir_all(&codex_nested).unwrap();

        fs::write(claude_nested.join("nested-session.jsonl"), "{}\n").unwrap();
        fs::write(codex_nested.join("rollout-nested.jsonl"), "{}\n").unwrap();

        let mut config = DiscoveryConfig::new();
        config.set_roots("claude-code", DiscoveryRoots::new(vec![claude_root]));
        config.set_roots("codex", DiscoveryRoots::new(vec![codex_root]));

        let report = discover(&AdapterRegistry::v0(), &config);
        assert_eq!(report.sessions.len(), 2);
        let names: Vec<_> = report
            .sessions
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"nested-session.jsonl".to_string()));
        assert!(names.contains(&"rollout-nested.jsonl".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_traversal_on_unreadable_files_and_directories() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let claude_root = dir.path().join("claude_projects");
        let codex_root = dir.path().join("codex_sessions");
        fs::create_dir_all(&claude_root).unwrap();
        fs::create_dir_all(&codex_root).unwrap();

        // Valid accessible sessions
        fs::write(claude_root.join("accessible.jsonl"), "{}\n").unwrap();
        fs::write(codex_root.join("rollout-accessible.jsonl"), "{}\n").unwrap();

        // Unreadable file (000 permissions)
        let unreadable_file = claude_root.join("unreadable.jsonl");
        fs::write(&unreadable_file, "{}\n").unwrap();
        fs::set_permissions(&unreadable_file, fs::Permissions::from_mode(0o000)).unwrap();

        // Unreadable directory
        let unreadable_dir = claude_root.join("unreadable_dir");
        fs::create_dir_all(&unreadable_dir).unwrap();
        fs::write(unreadable_dir.join("inaccessible.jsonl"), "{}\n").unwrap();
        fs::set_permissions(&unreadable_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let mut config = DiscoveryConfig::new();
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![claude_root.clone()]),
        );
        config.set_roots("codex", DiscoveryRoots::new(vec![codex_root]));

        let report = discover(&AdapterRegistry::v0(), &config);

        // Restore permissions so tempdir can be cleaned up cleanly
        let _ = fs::set_permissions(&unreadable_file, fs::Permissions::from_mode(0o644));
        let _ = fs::set_permissions(&unreadable_dir, fs::Permissions::from_mode(0o755));

        assert!(report.agents.iter().all(|a| a.detection.installed));
        let names: Vec<_> = report
            .sessions
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"accessible.jsonl".to_string()));
        assert!(names.contains(&"rollout-accessible.jsonl".to_string()));
        // unreadable_dir was skipped without error
        assert!(!names.contains(&"inaccessible.jsonl".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_broken_symlink_as_root() {
        let dir = tempfile::tempdir().unwrap();
        let broken_root = dir.path().join("broken_root_link");
        let non_existent_target = dir.path().join("target_does_not_exist");
        std::os::unix::fs::symlink(&non_existent_target, &broken_root).unwrap();

        let mut config = DiscoveryConfig::new();
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![broken_root.clone()]),
        );
        config.set_roots("codex", DiscoveryRoots::new(vec![broken_root.clone()]));

        let report = discover(&AdapterRegistry::v0(), &config);
        assert_eq!(report.agents.len(), 2);
        assert!(!report.agents[0].detection.installed);
        assert!(!report.agents[1].detection.installed);
        assert!(report.sessions.is_empty());

        let roots = watch_roots(&AdapterRegistry::v0(), &config);
        assert_eq!(roots, vec![broken_root]);
    }

    #[test]
    fn discovery_custom_root_configs_nested_and_overlapping() {
        let dir = tempfile::tempdir().unwrap();
        let parent_root = dir.path().join("all_agents");
        let nested_codex_root = parent_root.join("sub/codex");
        fs::create_dir_all(&nested_codex_root).unwrap();

        fs::write(parent_root.join("session.jsonl"), "{}\n").unwrap();
        fs::write(nested_codex_root.join("rollout-sub.jsonl"), "{}\n").unwrap();

        let mut config = DiscoveryConfig::new();
        // Overlapping roots for claude-code
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![parent_root.clone(), parent_root.clone()]),
        );
        config.set_roots(
            "codex",
            DiscoveryRoots::new(vec![nested_codex_root.clone()]),
        );

        let registry = AdapterRegistry::v0();
        let report = discover(&registry, &config);
        assert_eq!(report.agents.len(), 2);
        assert!(report.agents.iter().all(|a| a.detection.installed));

        // Overlapping duplicate roots deduplicate sessions
        let claude_sessions: Vec<_> = report
            .sessions
            .iter()
            .filter(|s| s.agent.0 == "claude-code")
            .collect();
        assert_eq!(claude_sessions.len(), 2);

        // owner_of selects the longest matching prefix for nested paths
        let nested_file = nested_codex_root.join("rollout-sub.jsonl");
        assert_eq!(
            owner_of(&registry, &config, &nested_file).map(|a| a.0),
            Some("codex")
        );

        let parent_file = parent_root.join("session.jsonl");
        assert_eq!(
            owner_of(&registry, &config, &parent_file).map(|a| a.0),
            Some("claude-code")
        );
    }
}
