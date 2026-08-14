//! M3 acceptance: a synthetic fixture home drives the full pipeline —
//! discovery → durable queue → claim → ingest → checkpoint → completion →
//! restart — with per-source isolation, coalescing, and no duplicate sessions
//! across archive moves or repeated events.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::DiscoveryConfig;
use lore_core::pipeline::{Pipeline, ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

/// Collects progress events for assertions (single-threaded drain).
#[derive(Default)]
struct RecordingSink {
    events: RefCell<Vec<ProgressEvent>>,
}

impl ProgressSink for RecordingSink {
    fn emit(&self, event: ProgressEvent) {
        self.events.borrow_mut().push(event);
    }
}

impl RecordingSink {
    fn events(&self) -> Vec<ProgressEvent> {
        self.events.borrow().clone()
    }
}

struct Home {
    _dir: tempfile::TempDir,
    _blob_dir: tempfile::TempDir,
    claude_root: PathBuf,
    codex_root: PathBuf,
    config: DiscoveryConfig,
    blobs: BlobStore,
}

fn fixture(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(rel);
    fs::read_to_string(path).unwrap()
}

fn fixture_home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let claude_root = dir.path().join("claude/projects");
    let codex_root = dir.path().join("codex/sessions");
    fs::create_dir_all(claude_root.join("encoded-repo")).unwrap();
    fs::create_dir_all(codex_root.join("2026/08/11")).unwrap();

    fs::write(
        claude_root.join("encoded-repo/claude.jsonl"),
        fixture("claude_code/basic_text.jsonl"),
    )
    .unwrap();
    fs::write(
        codex_root.join("2026/08/11/rollout-a.jsonl"),
        fixture("codex/minimal.jsonl"),
    )
    .unwrap();

    let mut config = DiscoveryConfig::new();
    config.set_roots(
        "claude-code",
        DiscoveryRoots::new(vec![claude_root.clone()]),
    );
    config.set_roots("codex", DiscoveryRoots::new(vec![codex_root.clone()]));

    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    Home {
        _dir: dir,
        _blob_dir: blob_dir,
        claude_root,
        codex_root,
        blobs,
        config,
    }
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn discovery_queue_ingest_checkpoint_completion_and_restart() {
    let home = fixture_home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    let sink = RecordingSink::default();

    // Discovery → queue: both sources are scheduled as durable pending jobs.
    let enqueued = pipeline.enqueue_scan(&sink).unwrap();
    assert_eq!(enqueued, 2);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM job WHERE state='pending'"),
        2
    );

    // Claim → ingest → checkpoint → completion.
    let summary = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(summary.ingested, 2);
    assert_eq!(summary.failed, 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM agent_session"), 2);
    assert_eq!(count(&conn, "SELECT count(*) FROM message"), 5);
    // Every source committed a checkpoint and its job is done.
    assert_eq!(
        count(&conn, "SELECT count(*) FROM ingest_state WHERE state='ok'"),
        2
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM job WHERE state='done'"),
        2
    );

    // Restart: nothing was left running; the unchanged source fingerprints are
    // already complete, so a fresh scan does not requeue or reread them.
    assert_eq!(pipeline.recover().unwrap(), 0);
    pipeline.enqueue_scan(&sink).unwrap();
    let restart = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(restart.skipped, 0);
    assert_eq!(restart.ingested, 0);
    assert_eq!(restart.processed(), 0);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM agent_session"),
        2,
        "no duplicate sessions"
    );

    // A scan event was emitted with both sources discovered.
    assert!(home_scan_emitted(&sink));
}

fn home_scan_emitted(sink: &RecordingSink) -> bool {
    sink.events()
        .iter()
        .any(|e| matches!(e, ProgressEvent::ScanEnqueued { discovered: 2, .. }))
}

#[test]
fn archive_move_and_repeated_events_do_not_duplicate_sessions() {
    let home = fixture_home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    let sink = RecordingSink::default();

    pipeline.enqueue_scan(&sink).unwrap();
    pipeline.drain(&sink, 10).unwrap();
    let sessions_before = count(&conn, "SELECT count(*) FROM agent_session");

    // Move the Codex rollout into an archive dir (still under the codex root).
    let original = home.codex_root.join("2026/08/11/rollout-a.jsonl");
    let archive = home.codex_root.join("archived");
    fs::create_dir_all(&archive).unwrap();
    let moved = archive.join("rollout-a.jsonl");
    fs::rename(&original, &moved).unwrap();

    // A watcher event for the new path resolves to Codex and ingests as a skip
    // (same content/native id): the session is not duplicated, and the artifact
    // records both paths.
    assert!(pipeline.enqueue_path(&moved).unwrap().is_some());
    let summary = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(summary.skipped, 1);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM agent_session"),
        sessions_before,
        "an archive move must not create a new session"
    );

    // Repeated events for the unchanged completed path stay completed.
    let first = pipeline.enqueue_path(&moved).unwrap().unwrap();
    let second = pipeline.enqueue_path(&moved).unwrap().unwrap();
    use lore_core::jobs::SourceSchedule;
    assert_eq!(first, SourceSchedule::CoalescedDone);
    assert_eq!(second, SourceSchedule::CoalescedDone);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM job WHERE state='pending'"),
        0
    );
}

#[test]
fn one_unreadable_source_does_not_stop_peers() {
    let home = fixture_home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    let sink = RecordingSink::default();

    // Schedule the good Claude source plus a bogus path under the Claude root.
    let missing = home.claude_root.join("encoded-repo/missing.jsonl");
    assert!(pipeline.enqueue_path(&missing).unwrap().is_some());
    pipeline
        .enqueue_path(&home.claude_root.join("encoded-repo/claude.jsonl"))
        .unwrap();

    let summary = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(summary.ingested, 1, "the readable source still ingests");
    assert_eq!(
        summary.failed, 1,
        "the unreadable source fails in isolation"
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM agent_session"), 1);
    // The failed job is durably marked failed with a bounded diagnostic.
    assert_eq!(
        count(&conn, "SELECT count(*) FROM job WHERE state='failed'"),
        1
    );
}

#[test]
fn a_change_during_a_run_is_reprocessed_after_completion() {
    // Simulate a restart with an interrupted (running) job: recover returns it
    // to pending and the next drain reprocesses it from the committed state.
    let home = fixture_home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    let sink = RecordingSink::default();

    pipeline.enqueue_scan(&sink).unwrap();
    // Claim one job to leave it "running" as if the process died mid-ingest.
    let running = lore_core::jobs::claim_next(&conn).unwrap().unwrap();
    assert_eq!(
        lore_core::jobs::load(&conn, &running.id)
            .unwrap()
            .unwrap()
            .state,
        lore_core::jobs::JobState::Running
    );

    // Restart recovery returns the interrupted job to pending.
    assert_eq!(pipeline.recover().unwrap(), 1);
    let summary = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(
        summary.ingested, 2,
        "the recovered job is reprocessed with its peer"
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM agent_session"), 2);
}

#[test]
fn newest_sources_are_ingested_first() {
    let home = fixture_home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    let sink = RecordingSink::default();

    pipeline.enqueue_scan(&sink).unwrap();
    let newest = home.codex_root.join("2026/08/11/rollout-a.jsonl");
    let file = fs::OpenOptions::new().write(true).open(&newest).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000))
        .unwrap();
    // Refresh priorities for already-pending jobs after the mtime change.
    pipeline.enqueue_scan(&sink).unwrap();

    let claimed = lore_core::jobs::claim_next(&conn).unwrap().unwrap();
    let payload = claimed.payload_json.unwrap();
    assert!(
        payload.contains("rollout-a.jsonl"),
        "the most recently modified source should be claimed first"
    );
}

#[test]
fn unchanged_scan_does_not_requeue_or_read_sources() {
    let home = fixture_home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    let sink = RecordingSink::default();

    assert_eq!(pipeline.enqueue_scan(&sink).unwrap(), 2);
    let first = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(first.ingested, 2);

    assert_eq!(pipeline.enqueue_scan(&sink).unwrap(), 0);
    let second = pipeline.drain(&sink, 10).unwrap();
    assert_eq!(second.processed(), 0);
}
