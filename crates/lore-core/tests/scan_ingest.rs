//! M1/M2 vertical acceptance: injected Claude + Codex roots discover and ingest
//! through one registry/pipeline, and a restart skips already-committed sources.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;

use lore_core::adapters::{AdapterRegistry, AgentId, DiscoveryRoots, SessionRef};
use lore_core::discovery::{discover, DiscoveryConfig};
use lore_core::ingest::{ingest_discovered, IngestFailureKind};

fn fixture_home() -> (tempfile::TempDir, DiscoveryConfig) {
    let dir = tempfile::tempdir().unwrap();
    let claude_root = dir.path().join("claude/projects");
    let claude_project = claude_root.join("encoded-repo");
    let codex_root = dir.path().join("codex/sessions");
    let codex_day = codex_root.join("2026/08/11");
    fs::create_dir_all(&claude_project).unwrap();
    fs::create_dir_all(&codex_day).unwrap();

    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    fs::copy(
        fixtures.join("claude_code/basic_text.jsonl"),
        claude_project.join("claude-session.jsonl"),
    )
    .unwrap();
    fs::copy(
        fixtures.join("codex/minimal.jsonl"),
        codex_day.join("rollout-test.jsonl"),
    )
    .unwrap();

    let mut config = DiscoveryConfig::new();
    config.set_roots("claude-code", DiscoveryRoots::new(vec![claude_root]));
    config.set_roots("codex", DiscoveryRoots::new(vec![codex_root]));
    (dir, config)
}

#[test]
fn both_v0_agents_ingest_end_to_end_and_restart_skips() {
    let (_home, config) = fixture_home();
    let registry = AdapterRegistry::v0();
    let discovery = discover(&registry, &config);
    let conn = lore_core::storage::open_in_memory().unwrap();

    let first = ingest_discovered(&conn, &registry, &discovery.sessions);
    assert_eq!(first.processed(), 2);
    assert_eq!(first.ingested.len(), 2);
    assert_eq!(first.skipped, 0);
    assert!(first.failures.is_empty());

    let (agents, sessions, messages): (i64, i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT count(*) FROM agent),
                (SELECT count(*) FROM agent_session),
                (SELECT count(*) FROM message)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(agents, 2);
    assert_eq!(sessions, 2);
    assert_eq!(messages, 5, "Claude fixture has 2; Codex fixture has 3");

    let restart = ingest_discovered(&conn, &registry, &discovery.sessions);
    assert_eq!(restart.processed(), 2);
    assert!(restart.ingested.is_empty());
    assert_eq!(restart.skipped, 2);
    assert!(restart.failures.is_empty());
}

#[test]
fn unknown_adapter_and_unreadable_source_fail_independently() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.jsonl");
    let sessions = vec![
        SessionRef {
            agent: AgentId("not-registered"),
            path: missing.clone(),
            mtime: None,
            size: 0,
            native_id: None,
        },
        SessionRef {
            agent: AgentId("claude-code"),
            path: missing,
            mtime: None,
            size: 0,
            native_id: None,
        },
    ];
    let conn = lore_core::storage::open_in_memory().unwrap();

    let report = ingest_discovered(&conn, &AdapterRegistry::v0(), &sessions);
    assert_eq!(report.processed(), 2);
    assert_eq!(report.failures.len(), 2);
    assert_eq!(
        report.failures[0].kind,
        IngestFailureKind::AdapterNotRegistered
    );
    assert_eq!(report.failures[1].kind, IngestFailureKind::SourceFailed);
}
