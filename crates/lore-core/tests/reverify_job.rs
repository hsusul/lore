//! I2 acceptance: commit re-verification is scheduled as a coalesced,
//! low-priority durable job drained by the bounded worker — not run inside the
//! ingest path — and is idempotent across passes (an unchanged verdict refreshes
//! its row rather than appending).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use lore_core::adapters::codex::CodexAdapter;
use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::DiscoveryConfig;
use lore_core::enrich::enrich_session;
use lore_core::ingest::persist_session;
use lore_core::pipeline::{NullSink, Pipeline};
use lore_core::storage::blob::BlobStore;
use lore_core::worker::{spawn, WorkerConfig};
use rusqlite::Connection;

fn git_raw(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
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
        .expect("git must be installed to run reverify-job tests")
}

fn git(dir: &Path, args: &[&str]) {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = git_raw(dir, args);
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
}

fn blobs() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    (dir, store)
}

/// Persist a Codex session recording `commit`/branch main at `cwd`, then enrich.
fn persist_and_enrich(conn: &Connection, blobs: &BlobStore, cwd: &Path, commit: &str) -> String {
    let content = format!(
        concat!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"rv\",\"cli_version\":\"1\",\"cwd\":\"{cwd}\",\"model_provider\":\"openai\",\"git\":{{\"branch\":\"main\",\"commit_hash\":\"{commit}\"}}}}}}\n",
            "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}}}\n"
        ),
        cwd = cwd.to_string_lossy(),
        commit = commit
    );
    let parsed = CodexAdapter::new().parse_str(&content, "rv");
    let sid = persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
    enrich_session(conn, &sid).unwrap();
    sid
}

fn count(conn: &Connection, sql: &str, arg: &str) -> i64 {
    conn.query_row(sql, [arg], |r| r.get(0)).unwrap()
}

fn job_count(conn: &Connection, kind: &str, state: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM job WHERE kind = ?1 AND state = ?2",
        [kind, state],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn reverify_job_enqueues_drains_and_is_idempotent() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blobs();
    let sid = persist_and_enrich(&conn, &blobs, repo.path(), &head);

    let registry = AdapterRegistry::v0();
    let config = DiscoveryConfig::new();
    let pipeline = Pipeline::new(&conn, &registry, &blobs, &config, 128);

    // Enqueue: one coalesced (worktree, commit) job, low priority.
    assert_eq!(pipeline.enqueue_reverify().unwrap(), 1);
    assert_eq!(job_count(&conn, "reverify", "pending"), 1);

    // Drain it through the bounded worker loop: produces one reverified row.
    let summary = pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(summary.reverified, 1, "the reverify job was drained");
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='lore_reverified'",
            &sid
        ),
        1
    );

    // Re-trigger re-arms the finished job; a second pass with nothing changed
    // refreshes the row in place instead of appending a second one.
    assert_eq!(pipeline.enqueue_reverify().unwrap(), 1);
    assert_eq!(job_count(&conn, "reverify", "pending"), 1);
    let summary2 = pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(summary2.reverified, 1);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='lore_reverified'",
            &sid
        ),
        1,
        "an unchanged verdict must not append"
    );
}

/// A `Send` sink so the threaded worker can run without a channel consumer.
struct DropSink;

impl lore_core::pipeline::ProgressSink for DropSink {
    fn emit(&self, _event: lore_core::pipeline::ProgressEvent) {}
}

#[test]
fn worker_shuts_down_cleanly_with_a_reverify_job_queued() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("lore.db");
    let (_bd, blobs) = blobs();

    // Pin both adapters to empty roots so the worker's initial scan never
    // touches real history.
    let mut config = DiscoveryConfig::new();
    config.set_roots(
        "claude-code",
        DiscoveryRoots::new(vec![db_dir.path().join("claude")]),
    );
    config.set_roots(
        "codex",
        DiscoveryRoots::new(vec![db_dir.path().join("codex")]),
    );

    // Persist + enrich + enqueue on a peer connection to the same DB file.
    {
        let peer = lore_core::storage::open(&db_path).unwrap();
        let _sid = persist_and_enrich(&peer, &blobs, repo.path(), &head);
        let registry = AdapterRegistry::v0();
        let pipeline = Pipeline::new(&peer, &registry, &blobs, &config, 128);
        assert_eq!(pipeline.enqueue_reverify().unwrap(), 1);
    }

    // Spawn the worker and shut it down deterministically. The initial scan's
    // bounded drain claims and completes the pending reverify job.
    let cfg = WorkerConfig {
        idle_poll: Duration::from_secs(3600),
        ..WorkerConfig::default()
    };
    let worker =
        lore_core::worker::open_worker(&db_path, AdapterRegistry::v0(), blobs, config, cfg)
            .unwrap();
    let handle = spawn(worker, None, DropSink);
    handle.shutdown();

    let check = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(
        check
            .query_row::<i64, _, _>("SELECT count(*) FROM job WHERE state='running'", [], |r| r
                .get(0))
            .unwrap(),
        0,
        "no job left running after a clean shutdown"
    );
    assert_eq!(job_count(&check, "reverify", "done"), 1);
    assert_eq!(
        check
            .query_row::<i64, _, _>(
                "SELECT count(*) FROM git_observation WHERE source='lore_reverified'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
        1,
        "the queued reverify job drained to a reverified observation"
    );
}
