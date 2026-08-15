//! M3 lifecycle acceptance: the background worker binds discovery, the durable
//! coalescing queue, and the bounded pipeline drain into continuous ingestion
//! tied to an app-style lifecycle.
//!
//! Every test is deterministic — temporary directories, injected roots, and
//! direct method/step calls (or a control channel for the threaded case). No
//! sleeps and no real `~/.claude` / `~/.codex` history.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::DiscoveryConfig;
use lore_core::jobs;
use lore_core::pipeline::{ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_core::worker::{spawn, Worker, WorkerConfig};
use rusqlite::Connection;

/// Single-threaded event recorder for method-level assertions.
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
    fn count<F: Fn(&ProgressEvent) -> bool>(&self, f: F) -> usize {
        self.events.borrow().iter().filter(|e| f(e)).count()
    }
}

/// `Send` sink for the threaded worker: forwards events over a channel.
struct ChannelSink(mpsc::Sender<ProgressEvent>);

impl ProgressSink for ChannelSink {
    fn emit(&self, event: ProgressEvent) {
        let _ = self.0.send(event);
    }
}

fn fixture(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(rel);
    fs::read_to_string(path).unwrap()
}

/// A synthetic agent home with one Claude and one Codex session under injected
/// roots. Returns the config plus the two source paths.
struct Home {
    _dir: tempfile::TempDir,
    _blob_dir: tempfile::TempDir,
    claude_root: PathBuf,
    codex_root: PathBuf,
    claude_file: PathBuf,
    config: DiscoveryConfig,
    blobs: BlobStore,
}

fn home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let claude_root = dir.path().join("claude/projects");
    let codex_root = dir.path().join("codex/sessions");
    fs::create_dir_all(claude_root.join("encoded-repo")).unwrap();
    fs::create_dir_all(codex_root.join("2026/08/11")).unwrap();

    let claude_file = claude_root.join("encoded-repo/claude.jsonl");
    fs::write(&claude_file, fixture("claude_code/basic_text.jsonl")).unwrap();
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
        claude_file,
        config,
        blobs,
    }
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn worker_in_memory(home: &Home, cfg: WorkerConfig) -> Worker {
    let conn = lore_core::storage::open_in_memory().unwrap();
    Worker::new(
        conn,
        AdapterRegistry::v0(),
        home.blobs.clone(),
        home.config.clone(),
        cfg,
    )
}

#[test]
fn full_scan_emits_pass_boundaries() {
    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());
    let sink = RecordingSink::default();

    worker.scan(&sink).unwrap();
    let events = sink.events();
    assert!(matches!(
        events.first(),
        Some(ProgressEvent::ScanEnqueued { .. })
    ));
    assert!(matches!(events.last(), Some(ProgressEvent::ScanFinished)));
}

// 1. Initial scan populates incrementally: even with a batch size of one, the
//    initial scan drains every discovered source and reports per-source
//    progress (results land one source at a time, not in a single monolith).
#[test]
fn initial_scan_populates_incrementally() {
    let home = home();
    let cfg = WorkerConfig {
        drain_batch: 1,
        ..WorkerConfig::default()
    };
    let worker = worker_in_memory(&home, cfg);
    let sink = RecordingSink::default();

    let summary = worker.scan(&sink).unwrap();
    assert_eq!(summary.ingested, 2, "both sources ingest across batches");
    assert_eq!(summary.failed, 0);
    // One Ingested event per source — progress is incremental, not one lump.
    assert_eq!(
        sink.count(|e| matches!(e, ProgressEvent::Ingested { .. })),
        2
    );
    assert_eq!(worker.queue_depth().unwrap().active(), 0, "queue drained");
}

// 2. New and appended activity is ingested without a manual full rescan: a
//    debounced path handed to the worker ingests the appended turn, and a
//    brand-new file appearing under a root ingests as a new session.
#[test]
fn appended_and_new_activity_ingested_without_manual_rescan() {
    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());
    let sink = RecordingSink::default();

    // Initial state.
    worker.scan(&sink).unwrap();
    // Snapshot Claude message count via a peer read connection is not available
    // (the worker owns its conn); assert through re-ingest deltas instead.

    // Append a new user turn to the existing Claude session file.
    let appended = format!(
        "{}{}\n",
        fixture("claude_code/basic_text.jsonl"),
        r#"{"type":"user","uuid":"33333333-3333-4333-8333-333333333333","parentUuid":"22222222-2222-4222-8222-222222222222","sessionId":"aaaaaaaa-0000-4000-8000-000000000001","timestamp":"2026-08-10T10:01:00.000Z","cwd":"/repo/app","gitBranch":"main","version":"1.2.3","isSidechain":false,"userType":"external","message":{"role":"user","content":"also add readiness"}}"#
    );
    fs::write(&home.claude_file, appended).unwrap();

    // A brand-new Codex session appears under the codex root.
    let new_codex = home.codex_root.join("2026/08/11/rollout-b.jsonl");
    let mut second = fixture("codex/minimal.jsonl");
    // Give it a distinct native session id so it is a new session, not a dupe.
    second = second.replace(
        "019e0000-0000-7000-8000-000000000001",
        "019e0000-0000-7000-8000-0000000000ff",
    );
    fs::write(&new_codex, second).unwrap();

    // Hand the two debounced paths to the worker — no full rescan call.
    let pass = worker
        .run_pending(&[home.claude_file.clone(), new_codex.clone()], &sink)
        .unwrap();
    assert_eq!(pass.enqueued, 2);
    assert_eq!(pass.drained.ingested, 2, "appended + new both ingest");

    // Verify the durable effect on a peer read connection to a shared file DB
    // would be ideal, but the in-memory worker owns its connection; instead
    // assert via a fresh scan that unchanged fingerprints schedule no work.
    let follow = worker.scan(&sink).unwrap();
    assert_eq!(follow.processed(), 0, "already-ingested sources stay done");
}

// 3. Truncate/rewrite after ingest re-parses to the correct state (no stale
//    rows, no duplicate session).
#[test]
fn truncate_after_ingest_reparses_correctly() {
    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());
    let sink = RecordingSink::default();
    worker.scan(&sink).unwrap();

    // Truncate the Claude file to just its first line (a shorter session).
    let first_line = fixture("claude_code/basic_text.jsonl")
        .lines()
        .next()
        .unwrap()
        .to_string();
    fs::write(&home.claude_file, format!("{first_line}\n")).unwrap();

    let pass = worker
        .run_pending(std::slice::from_ref(&home.claude_file), &sink)
        .unwrap();
    assert_eq!(
        pass.drained.ingested, 1,
        "the truncated file re-parses as a change"
    );
    // A subsequent scan finds every completed fingerprint unchanged and queues
    // nothing, so no duplicate session can be introduced.
    let follow = worker.scan(&sink).unwrap();
    assert_eq!(follow.processed(), 0);
}

// 4. Repeated event storms coalesce onto a single pending job.
#[test]
fn event_storm_coalesces_to_one_job() {
    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());

    // Ten events for the same source within one enqueue burst.
    let storm: Vec<PathBuf> = vec![home.claude_file.clone(); 10];
    let enqueued = worker.enqueue_paths(&storm).unwrap();
    assert_eq!(enqueued, 1, "a storm enqueues exactly one job");
    assert_eq!(worker.queue_depth().unwrap().pending, 1);
}

// 5. Restart recovers work left running by a terminated process. A peer
//    connection to a shared file DB leaves a claimed (running) job behind; a
//    fresh worker on its own connection recovers and drains it.
#[test]
fn restart_recovers_running_work_safely() {
    let home = home();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lore.db");

    // "Previous process": enqueue a real source job and claim it (now running),
    // then drop the connection as if the process died mid-ingest.
    {
        let prev = lore_core::storage::open(&db_path).unwrap();
        let registry = AdapterRegistry::v0();
        let pipeline =
            lore_core::pipeline::Pipeline::new(&prev, &registry, &home.blobs, &home.config, 128);
        assert!(pipeline.enqueue_path(&home.claude_file).unwrap().is_some());
        let running = jobs::claim_next(&prev).unwrap().unwrap();
        assert_eq!(running.state, jobs::JobState::Running);
    }

    // "Restart": a fresh worker on its own connection to the same file recovers
    // the interrupted job and drains it to completion.
    let worker = lore_core::worker::open_worker(
        &db_path,
        AdapterRegistry::v0(),
        home.blobs.clone(),
        home.config.clone(),
        WorkerConfig::default(),
    )
    .unwrap();
    assert_eq!(worker.recover().unwrap(), 1, "the running job is recovered");
    let sink = RecordingSink::default();
    let summary = worker.drain_batch(&sink).unwrap();
    assert_eq!(summary.ingested, 1);

    let check = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(count(&check, "SELECT count(*) FROM agent_session"), 1);
}

// 6. Queue capacity/backpressure is bounded and observable.
#[test]
fn backpressure_is_bounded_and_observable() {
    let home = home();
    // Capacity of two runnable jobs.
    let worker = worker_in_memory(
        &home,
        WorkerConfig {
            queue_capacity: 2,
            ..WorkerConfig::default()
        },
    );

    let under_root = |name: &str| home.claude_root.join("encoded-repo").join(name);
    let a = under_root("a.jsonl");
    let b = under_root("b.jsonl");
    let c = under_root("c.jsonl");

    assert_eq!(worker.enqueue_paths(&[a, b]).unwrap(), 2);
    assert_eq!(
        worker.queue_depth().unwrap().active(),
        2,
        "depth observable"
    );

    // The third exceeds the ceiling and is rejected, not silently queued.
    let err = worker.enqueue_paths(&[c]).unwrap_err();
    assert!(matches!(err, jobs::JobQueueError::Full { limit: 2 }));
    assert_eq!(
        worker.queue_depth().unwrap().active(),
        2,
        "capacity stays bounded"
    );
}

// 7. A single malformed/unreadable source fails in isolation without stopping
//    its peers in the same drain.
#[test]
fn one_bad_source_does_not_stop_peers() {
    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());
    let sink = RecordingSink::default();

    let missing = home.claude_root.join("encoded-repo/missing.jsonl");
    let pass = worker
        .run_pending(&[missing, home.claude_file.clone()], &sink)
        .unwrap();
    assert_eq!(pass.drained.ingested, 1, "the readable peer still ingests");
    assert_eq!(pass.drained.failed, 1, "the bad source fails alone");
    assert_eq!(sink.count(|e| matches!(e, ProgressEvent::Failed { .. })), 1);
}

// 8a. Shutdown with queued and running work is deterministic and lossless: the
//     step-level shutdown sequence (stop draining, recover) leaves every job
//     recoverable and re-drainable — nothing is lost.
#[test]
fn shutdown_leaves_queued_and_running_work_recoverable() {
    let home = home();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lore.db");

    // Queue both sources; claim one so we have one running + one pending when
    // "shutdown" happens.
    let worker = lore_core::worker::open_worker(
        &db_path,
        AdapterRegistry::v0(),
        home.blobs.clone(),
        home.config.clone(),
        WorkerConfig::default(),
    )
    .unwrap();
    // Enqueue via a peer connection so we can claim deterministically.
    {
        let peer = lore_core::storage::open(&db_path).unwrap();
        let registry = AdapterRegistry::v0();
        let pipeline =
            lore_core::pipeline::Pipeline::new(&peer, &registry, &home.blobs, &home.config, 128);
        pipeline.enqueue_scan(&RecordingSink::default()).unwrap();
        let _running = jobs::claim_next(&peer).unwrap().unwrap();
    }
    let before = worker.queue_depth().unwrap();
    assert_eq!(before.active(), 2, "one running + one pending");
    assert_eq!(before.running, 1);

    // Shutdown recovery: running → pending, nothing lost.
    assert_eq!(worker.recover().unwrap(), 1);
    assert_eq!(worker.queue_depth().unwrap().pending, 2);

    // Draining after restart completes all work with no duplicates.
    let summary = worker.scan(&RecordingSink::default()).unwrap();
    assert_eq!(summary.ingested, 2);
    let check = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(count(&check, "SELECT count(*) FROM agent_session"), 2);
}

// 8b. The threaded worker starts (recover + initial scan) and shuts down
//     cleanly on demand. Deterministic: `shutdown` joins the thread, and the
//     initial scan always runs before the control loop, so its results are
//     observable afterward. A large idle poll means no wall-clock time passes.
#[test]
fn spawned_worker_scans_then_shuts_down_clean() {
    let home = home();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lore.db");

    let cfg = WorkerConfig {
        // Block on the control channel; never spin on a timer during the test.
        idle_poll: std::time::Duration::from_secs(3600),
        ..WorkerConfig::default()
    };
    let worker = lore_core::worker::open_worker(
        &db_path,
        AdapterRegistry::v0(),
        home.blobs.clone(),
        home.config.clone(),
        cfg,
    )
    .unwrap();
    let watcher = lore_core::watcher::SessionWatcher::new(
        &[home.claude_root.clone(), home.codex_root.clone()],
        std::time::Duration::from_millis(10),
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let handle = spawn(worker, Some(watcher), ChannelSink(tx));

    // Deterministic stop: joins the thread after the initial scan completed.
    handle.shutdown();

    // The initial scan ingested both sources before shutdown.
    let check = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(count(&check, "SELECT count(*) FROM agent_session"), 2);
    assert_eq!(
        count(&check, "SELECT count(*) FROM job WHERE state='done'"),
        2,
        "no job left running after a clean shutdown"
    );
    // At least the initial scan emitted content-free progress.
    let events: Vec<ProgressEvent> = rx.try_iter().collect();
    assert!(events
        .iter()
        .any(|e| matches!(e, ProgressEvent::Ingested { .. })));
}

#[test]
fn spawned_worker_reconfigures_roots_without_restarting_the_app() {
    let home = home();
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("lore.db");
    let worker = lore_core::worker::open_worker(
        &db_path,
        AdapterRegistry::v0(),
        home.blobs.clone(),
        home.config.clone(),
        WorkerConfig {
            idle_poll: std::time::Duration::from_secs(3600),
            ..WorkerConfig::default()
        },
    )
    .unwrap();
    let initial_watcher = lore_core::watcher::SessionWatcher::new(
        &[home.claude_root.clone(), home.codex_root.clone()],
        std::time::Duration::from_millis(10),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let handle = spawn(worker, Some(initial_watcher), ChannelSink(tx));

    while !matches!(
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
        ProgressEvent::ScanFinished
    ) {}

    let extra = tempfile::tempdir().unwrap();
    let extra_root = extra.path().join("codex/sessions");
    fs::create_dir_all(extra_root.join("2026/08/12")).unwrap();
    let extra_file = extra_root.join("2026/08/12/rollout-extra.jsonl");
    let content = fixture("codex/minimal.jsonl").replace(
        "019e0000-0000-7000-8000-000000000001",
        "019e0000-0000-7000-8000-0000000000ee",
    );
    fs::write(extra_file, content).unwrap();

    let mut replacement = DiscoveryConfig::new();
    replacement.set_roots(
        "claude-code",
        DiscoveryRoots::new(vec![extra.path().join("claude/projects")]),
    );
    replacement.set_roots("codex", DiscoveryRoots::new(vec![extra_root.clone()]));
    let replacement_watcher = lore_core::watcher::SessionWatcher::new(
        std::slice::from_ref(&extra_root),
        std::time::Duration::from_millis(10),
    )
    .unwrap();
    handle.reconfigure(replacement, Some(replacement_watcher));

    while !matches!(
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
        ProgressEvent::ScanFinished
    ) {}
    handle.shutdown();

    let check = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(
        count(&check, "SELECT count(*) FROM agent_session"),
        3,
        "the replacement root is scanned without restarting Lore"
    );
}

// 9. Read-only invariant: Lore never needs write access to a source. Ingesting
//    a source whose file is mode 0o444 succeeds and leaves the mode untouched.
#[cfg(unix)]
#[test]
fn source_files_are_never_opened_writable() {
    use std::os::unix::fs::PermissionsExt;

    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());
    let sink = RecordingSink::default();

    // Make the Claude source read-only for everyone.
    let ro = fs::Permissions::from_mode(0o444);
    fs::set_permissions(&home.claude_file, ro).unwrap();

    let pass = worker
        .run_pending(std::slice::from_ref(&home.claude_file), &sink)
        .unwrap();
    assert_eq!(pass.drained.ingested, 1, "a read-only source still ingests");

    let mode = fs::metadata(&home.claude_file)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o444, "Lore must not alter source permissions");
}

// 10. Progress events are content-free: they carry counts, a static adapter id,
//     and an outcome — never a filesystem path or session content.
#[test]
fn progress_events_are_content_free() {
    let home = home();
    let worker = worker_in_memory(&home, WorkerConfig::default());
    let sink = RecordingSink::default();
    worker.scan(&sink).unwrap();

    // A distinctive token from the source tree that must never appear in events.
    let home_marker = home.claude_root.to_string_lossy().to_string();
    for event in sink.events() {
        let rendered = format!("{event:?}");
        assert!(
            !rendered.contains(&home_marker),
            "event leaked a source path: {rendered}"
        );
        assert!(
            !rendered.contains("health"),
            "event leaked session content: {rendered}"
        );
        // Any agent id present is a static schema id, not a path.
        if let ProgressEvent::Ingested { agent_id, .. }
        | ProgressEvent::Skipped { agent_id }
        | ProgressEvent::Failed { agent_id, .. }
        | ProgressEvent::Requeued { agent_id } = &event
        {
            assert!(
                agent_id == "claude-code" || agent_id == "codex",
                "unexpected agent id {agent_id}"
            );
        }
    }
}
