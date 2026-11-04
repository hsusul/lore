//! Per-day progress for the newest days: sources queued, then sessions read,
//! each counted on the day it was last active.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::DiscoveryConfig;
use lore_core::pipeline::NullSink;
use lore_core::recent::recent_days;
use lore_core::storage::blob::BlobStore;
use lore_core::worker::{open_worker, WorkerConfig};

const AUG_9: i64 = 1_786_233_600_000;
const DAY: i64 = 86_400_000;

fn fixture(rel: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(rel),
    )
    .unwrap()
}

fn write_at(path: &Path, content: &str, at_ms: i64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
    let at = UNIX_EPOCH + Duration::from_millis(u64::try_from(at_ms).unwrap());
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[test]
fn a_day_counts_what_is_queued_then_what_was_read() {
    let home = tempfile::tempdir().unwrap();
    let claude = home.path().join("claude/projects");
    let codex = home.path().join("codex/sessions");
    // Each file's mtime is its session's last message, as an agent leaves it.
    write_at(
        &claude.join("repo/claude.jsonl"),
        &fixture("claude_code/basic_text.jsonl"),
        AUG_9 + DAY + 36_005_000,
    );
    write_at(
        &codex.join("2026/08/11/rollout-a.jsonl"),
        &fixture("codex/minimal.jsonl"),
        AUG_9 + 2 * DAY + 36_004_000,
    );
    let mut config = DiscoveryConfig::new();
    config.set_roots("claude-code", DiscoveryRoots::new(vec![claude]));
    config.set_roots("codex", DiscoveryRoots::new(vec![codex]));
    let blobs = tempfile::tempdir().unwrap();
    let db = home.path().join("lore.db");
    let worker = open_worker(
        &db,
        AdapterRegistry::v0(),
        BlobStore::open(blobs.path()).unwrap(),
        config,
        WorkerConfig::default(),
    )
    .unwrap();
    let reader = lore_core::storage::open(&db).unwrap();
    let now = AUG_9 + 2 * DAY + 43_200_000;
    let days = || -> Vec<(i64, i64, i64)> {
        recent_days(&reader, now, 0, 3)
            .unwrap()
            .into_iter()
            .map(|d| ((d.day_start_ms - AUG_9) / DAY, d.read, d.waiting))
            .collect()
    };

    // A deadline already past queues both sources and reads neither.
    let (_, left) = worker
        .scan_until(&NullSink, None, Some(std::time::Instant::now()))
        .unwrap();
    assert_eq!(left, 2);
    assert_eq!(days(), [(0, 0, 0), (1, 0, 1), (2, 0, 1)]);

    worker.scan(&NullSink).unwrap();
    assert_eq!(days(), [(0, 0, 0), (1, 1, 0), (2, 1, 0)]);
}

#[test]
fn days_start_at_local_midnight_oldest_first() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    // 01:00 UTC on Aug 11 is still Aug 10 at UTC-6.
    let now = AUG_9 + 2 * DAY + 3_600_000;
    let central = -6 * 3_600_000;
    let starts: Vec<i64> = recent_days(&conn, now, central, 2)
        .unwrap()
        .into_iter()
        .map(|d| d.day_start_ms)
        .collect();
    assert_eq!(starts, [AUG_9 + 6 * 3_600_000, AUG_9 + DAY + 6 * 3_600_000]);
}
