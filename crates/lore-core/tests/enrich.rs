//! M4 acceptance: git enrichment resolves repository/worktree identity from a
//! segment's cwd, records a `lore_captured` observation separate from the
//! agent-recorded one, groups linked worktrees under one repository, and leaves
//! a non-git cwd unlinked.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::enrich::enrich_session;
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed to run M4 enrich tests");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
}

/// A one-message Claude session whose recorded cwd is `cwd`.
fn persist_session_at(conn: &Connection, blobs: &BlobStore, dedupe: &str, cwd: &Path) -> String {
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{dedupe}\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        cwd.to_string_lossy()
    );
    let parsed = ClaudeCodeAdapter::new().parse_str(&content, dedupe);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

fn blobs() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    (dir, store)
}

#[test]
fn enrich_resolves_repository_and_records_lore_captured_observation() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "enrich", repo.path());
    let linked = enrich_session(&conn, &sid).unwrap();
    assert_eq!(linked, 1);

    // The segment is now linked with high confidence.
    let (repo_id, confidence): (String, String) = conn
        .query_row(
            "SELECT repository_id, resolution_confidence FROM session_segment
             WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(confidence, "high");

    let (rep_conf, is_primary): (String, i64) = conn
        .query_row(
            "SELECT r.identity_confidence, w.is_primary
             FROM repository r JOIN worktree w ON w.repository_id = r.id
             WHERE r.id = ?1",
            [&repo_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rep_conf, "high");
    assert_eq!(is_primary, 1, "the main worktree is primary");

    // A git_common_dir evidence row backs the identity.
    let ev: i64 = conn
        .query_row(
            "SELECT count(*) FROM repository_identity_evidence
             WHERE repository_id = ?1 AND kind = 'git_common_dir'",
            [&repo_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ev, 1);

    // Both provenances coexist and are separate: agent_recorded (branch) and
    // lore_captured (current state, dirty=false, real commit).
    let (agent, captured): (i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='agent_recorded'),
                (SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='lore_captured')",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(agent, 1);
    assert_eq!(captured, 1);

    let (tconf, branch, is_dirty, sha_len): (String, String, i64, usize) = conn
        .query_row(
            "SELECT temporal_confidence, branch, is_dirty, length(commit_sha)
             FROM git_observation WHERE session_id=?1 AND source='lore_captured'",
            [&sid],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)? as usize,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        tconf, "current_only",
        "captured state is never session-time"
    );
    assert_eq!(branch, "main");
    assert_eq!(is_dirty, 0);
    assert_eq!(sha_len, 40);
}

#[test]
fn linked_worktrees_group_under_one_repository() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main");
    std::fs::create_dir(&main).unwrap();
    init_repo(&main);
    let linked = root.path().join("feature-wt");
    git(
        &main,
        &["worktree", "add", linked.to_str().unwrap(), "-b", "feature"],
    );

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let main_sid = persist_session_at(&conn, &store, "main-sess", &main);
    let linked_sid = persist_session_at(&conn, &store, "linked-sess", &linked);
    enrich_session(&conn, &main_sid).unwrap();
    enrich_session(&conn, &linked_sid).unwrap();

    let repo_of = |sid: &str| -> String {
        conn.query_row(
            "SELECT repository_id FROM session_segment WHERE session_id = ?1",
            [sid],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        repo_of(&main_sid),
        repo_of(&linked_sid),
        "linked worktrees resolve to one repository identity"
    );

    // One repository, two distinct worktrees.
    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    let worktrees: i64 = conn
        .query_row("SELECT count(*) FROM worktree", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 1);
    assert_eq!(worktrees, 2);
}

#[test]
fn non_git_cwd_is_left_unlinked() {
    let plain = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "plain", plain.path());
    assert_eq!(enrich_session(&conn, &sid).unwrap(), 0);

    let repo_id: Option<String> = conn
        .query_row(
            "SELECT repository_id FROM session_segment WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        repo_id.is_none(),
        "a non-git cwd stays under 'No repository'"
    );
    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 0);
}

#[test]
fn pipeline_enriches_automatically_on_ingest() {
    use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
    use lore_core::discovery::DiscoveryConfig;
    use lore_core::pipeline::{NullSink, Pipeline};

    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    // A discoverable Claude session file whose recorded cwd is the repo.
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let project = projects.join("encoded");
    std::fs::create_dir_all(&project).unwrap();
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"pipe\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        repo.path().to_string_lossy()
    );
    std::fs::write(project.join("s.jsonl"), content).unwrap();

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let registry = AdapterRegistry::v0();
    let mut config = DiscoveryConfig::new();
    config.set_roots("claude-code", DiscoveryRoots::new(vec![projects]));
    // Isolate Codex to an empty root so discovery never touches real ~/.codex.
    config.set_roots(
        "codex",
        DiscoveryRoots::new(vec![home.path().join("codex-empty")]),
    );
    let pipeline = Pipeline::new(&conn, &registry, &store, &config, 64);

    pipeline.enqueue_scan(&NullSink).unwrap();
    let summary = pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(summary.ingested, 1);
    assert_eq!(summary.enriched, 1, "ingest triggers enrichment");
    assert_eq!(summary.enrich_failed, 0);

    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 1);
}

#[test]
fn enrich_is_idempotent() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "idem", repo.path());
    enrich_session(&conn, &sid).unwrap();
    // Second pass finds the segment already linked → no new work, no duplicates.
    assert_eq!(enrich_session(&conn, &sid).unwrap(), 0);

    for (table, expected) in [("repository", 1), ("worktree", 1)] {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, expected, "{table} must not duplicate");
    }
    let captured: i64 = conn
        .query_row(
            "SELECT count(*) FROM git_observation WHERE source='lore_captured'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(captured, 1);
}
