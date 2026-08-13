//! M3 scale acceptance: a deterministic synthetic profile drives the background
//! worker at volume. Proves the initial scan ingests every generated session
//! incrementally, that re-scanning dedupes, and (opt-in, `--ignored`) that a
//! 10k-session home streams to completion with a bounded queue and no OOM.
//!
//! Everything is synthetic and local — `lore_core::synthetic` writes a fake
//! agent home under a temp dir; no real `~/.claude` / `~/.codex` is ever read.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::AdapterRegistry;
use lore_core::pipeline::{ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_core::synthetic::{generate, ProfileSpec};
use lore_core::worker::{open_worker, WorkerConfig};
use rusqlite::Connection;
use std::cell::RefCell;

#[derive(Default)]
struct CountingSink {
    ingested: RefCell<usize>,
}

impl ProgressSink for CountingSink {
    fn emit(&self, event: ProgressEvent) {
        if let ProgressEvent::Ingested { .. } = event {
            *self.ingested.borrow_mut() += 1;
        }
    }
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn synthetic_profile_ingests_incrementally_and_dedupes() {
    let home = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("lore.db");

    let spec = ProfileSpec {
        claude_sessions: 90,
        codex_sessions: 90,
        max_extra_turns: 3,
        seed: 7,
    };
    let profile = generate(home.path(), &spec).unwrap();
    let total = profile.claude_files + profile.codex_files;

    // A deliberately small drain batch forces multiple bounded passes: the scan
    // must still land every session.
    let cfg = WorkerConfig {
        drain_batch: 16,
        ..WorkerConfig::default()
    };
    let worker = open_worker(
        &db_path,
        AdapterRegistry::v0(),
        BlobStore::open(blob_dir.path()).unwrap(),
        profile.discovery_config(),
        cfg,
    )
    .unwrap();

    let sink = CountingSink::default();
    let summary = worker.scan(&sink).unwrap();
    assert_eq!(summary.ingested, total, "every generated session ingests");
    assert_eq!(summary.failed, 0, "generated fixtures parse cleanly");
    assert_eq!(
        *sink.ingested.borrow(),
        total,
        "one incremental progress event per source"
    );
    assert_eq!(worker.queue_depth().unwrap().active(), 0, "queue drained");

    // Durable, exact effect on a peer read connection.
    let peer = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(
        count(&peer, "SELECT count(*) FROM agent_session"),
        total as i64
    );
    assert_eq!(
        count(&peer, "SELECT count(*) FROM message"),
        profile.message_count as i64,
        "message tally matches the generator's exact accounting"
    );

    // Re-scanning the unchanged home dedupes: nothing re-ingests, no duplicates.
    let again = worker.scan(&CountingSink::default()).unwrap();
    assert_eq!(again.ingested, 0);
    assert_eq!(again.skipped, total);
    assert_eq!(
        count(&peer, "SELECT count(*) FROM agent_session"),
        total as i64,
        "a second scan creates no duplicate sessions"
    );
}

/// Opt-in heavy test (`cargo test -p lore-core --test scale -- --ignored`).
/// 10k sessions must stream to completion with a bounded queue and no OOM.
#[test]
#[ignore = "heavy: ~10k-session ingest; run explicitly for scale validation"]
fn scan_ten_thousand_sessions_streams_and_stays_bounded() {
    let home = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("lore.db");

    let spec = ProfileSpec {
        claude_sessions: 5_000,
        codex_sessions: 5_000,
        max_extra_turns: 4,
        seed: 2026,
    };
    let profile = generate(home.path(), &spec).unwrap();
    let total = profile.claude_files + profile.codex_files;
    assert_eq!(total, 10_000);

    let cfg = WorkerConfig {
        drain_batch: 500,
        ..WorkerConfig::default()
    };
    let worker = open_worker(
        &db_path,
        AdapterRegistry::v0(),
        BlobStore::open(blob_dir.path()).unwrap(),
        profile.discovery_config(),
        cfg,
    )
    .unwrap();

    let summary = worker.scan(&CountingSink::default()).unwrap();
    assert_eq!(summary.ingested, total, "all 10k sessions ingest");
    assert_eq!(summary.failed, 0);
    assert_eq!(worker.queue_depth().unwrap().active(), 0);

    let peer = lore_core::storage::open(&db_path).unwrap();
    assert_eq!(count(&peer, "SELECT count(*) FROM agent_session"), 10_000);
    assert_eq!(
        count(&peer, "SELECT count(*) FROM message"),
        profile.message_count as i64
    );
}

/// Opt-in helper to seed a synthetic profile on disk for manual app QA
/// (`LORE_SEED_DIR=/tmp/lore-fix cargo test -p lore-core --test scale \
///   seed_dev_profile -- --ignored --nocapture`), then launch the app with
/// `LORE_DEV_CLAUDE_ROOT` / `LORE_DEV_CODEX_ROOT` pointing at it. Never writes
/// unless `LORE_SEED_DIR` is set.
#[test]
#[ignore = "manual: seeds a synthetic profile for cargo tauri dev"]
fn seed_dev_profile() {
    let Ok(dir) = std::env::var("LORE_SEED_DIR") else {
        eprintln!("LORE_SEED_DIR not set; nothing to seed");
        return;
    };
    let n = |var: &str, default: usize| {
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let spec = ProfileSpec {
        claude_sessions: n("LORE_SEED_CLAUDE", 200),
        codex_sessions: n("LORE_SEED_CODEX", 200),
        max_extra_turns: 6,
        seed: n("LORE_SEED", 1) as u64,
    };
    let profile = generate(std::path::Path::new(&dir), &spec).unwrap();
    eprintln!(
        "seeded {} sessions ({} messages)\n  LORE_DEV_CLAUDE_ROOT={}\n  LORE_DEV_CODEX_ROOT={}",
        profile.claude_files + profile.codex_files,
        profile.message_count,
        profile.claude_root.display(),
        profile.codex_root.display(),
    );
}
