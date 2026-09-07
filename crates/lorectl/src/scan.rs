//! `lorectl scan` — the CLI's only writer.
//!
//! Drives the same ingestion the desktop app drives, synchronously: no thread,
//! no watcher, no daemon. A command-line scan is a bounded piece of work with an
//! exit code, so it runs on the calling thread and finishes. The app's live
//! watcher is a different shape of the same pipeline, not a different pipeline.
//!
//! ## Order of operations, and why it is this order
//!
//! ```text
//! resolve archive dir → create it → TAKE THE WRITER LOCK
//!   → storage::open (creates + migrates) → BlobStore::open
//!   → discovery_config → Worker::new → recover() → scan()
//!   → stamp scan.last_completed_at
//! ```
//!
//! The lock is taken **before** the database is opened, not after. `recover()`
//! is the dangerous call — it returns every `running` job to `pending` with no
//! way to tell whose job it was (see [`lore_core::lock`]) — and anything that
//! happens between opening and locking is a window in which two writers both
//! believe they are alone. Taking the lock first makes the window zero.
//!
//! The completion stamp is written **after** a successful scan and never
//! otherwise, because its only job is to answer "when did Lore last look?".
//! Stamping a scan that failed halfway would make that answer a lie, and it is
//! exactly the kind of claim Lore must not make.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use lore_core::adapters::AdapterRegistry;
use lore_core::lock::ScanLock;
use lore_core::paths::{ARCHIVE_DB_FILENAME, BLOBS_DIRNAME};
use lore_core::pipeline::{DrainSummary, NullSink};
use lore_core::storage::blob::BlobStore;
use lore_core::storage::StorageError;
use lore_core::worker::{self, WorkerConfig};
use lore_core::{settings, source_roots, storage};

use crate::cli::Invocation;
use crate::exit::{self, CliError};

// The completion stamp lives in `lore_core::settings` so the desktop worker
// and this command record the same key; `Worker::scan` writes it.

/// Run a full scan of every configured source root into the archive.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let archive_dir = invocation.archive_dir()?;
    std::fs::create_dir_all(&archive_dir).map_err(|_| CliError::Storage(StorageError::Io))?;

    // Before anything opens the database. See the module docs.
    let _lock = ScanLock::acquire(&archive_dir)?;

    let db_path = archive_dir.join(ARCHIVE_DB_FILENAME);
    let conn = storage::open(&db_path)?;
    let blobs = BlobStore::open(archive_dir.join(BLOBS_DIRNAME))?;
    let registry = AdapterRegistry::v0();
    let config = source_roots::discovery_config(&conn, &registry)?;

    // A dedicated worker connection, exactly as the desktop app does: WAL lets
    // the two coexist, and `Worker` owns its connection for its lifetime.
    let worker = worker::open_worker(
        &db_path,
        AdapterRegistry::v0(),
        blobs,
        config,
        WorkerConfig::default(),
    )?;

    // Safe here only because the lock is held: without it this would reclaim a
    // concurrently-running writer's jobs.
    let recovered = worker.recover()?;
    let summary = worker.scan(&NullSink)?;

    // Close the worker's connection before stamping, so the write below is the
    // only one open. Not required for correctness — WAL permits both, and the
    // worker is idle by now — but it keeps "the scan is over" a fact about the
    // process rather than a claim about the worker's internal state.
    drop(worker);

    // `Worker::scan` already stamped completion; read it back so the report
    // shows exactly what a later `status` will read, rather than a second clock
    // reading that could differ.
    let completed_at = settings::last_scan_completed_at(&conn)?.unwrap_or_else(now_ms);

    let mut stdout = std::io::stdout().lock();
    let report = if invocation.json {
        json_report(&archive_dir, recovered, &summary, completed_at)
    } else {
        text_report(&archive_dir, recovered, &summary)
    };
    let _ = writeln!(stdout, "{report}");

    Ok(exit::OK)
}

/// Epoch millis, saturating rather than panicking on a clock before the epoch.
/// Shared with `status`, which reads the stamp this writes.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Human-readable summary.
///
/// Names the archive deliberately: `SECURITY.md` §5 asks that a non-default
/// archive location be visible in any output someone acts on, and a scan writing
/// to an unexpected archive is precisely that case. Reports counts only — no
/// claim about what the work *means*.
fn text_report(archive_dir: &Path, recovered: usize, summary: &DrainSummary) -> String {
    let mut line = format!(
        "archive {}\ningested {}  skipped {}  failed {}",
        archive_dir.display(),
        summary.ingested,
        summary.skipped,
        summary.failed
    );
    // Only mention the unusual things when they happened, so a normal scan reads
    // as one short line.
    if recovered > 0 {
        line.push_str(&format!("\nrecovered {recovered} interrupted job(s)"));
    }
    if summary.requeued > 0 {
        line.push_str(&format!(
            "\nrequeued {} source(s) changed mid-scan",
            summary.requeued
        ));
    }
    line
}

/// Machine-readable summary.
///
/// Deliberately flat and additive: every field is a count or a timestamp with an
/// obvious meaning, so later commands can add keys without reshaping this one.
/// Built with `serde_json` rather than string formatting because the archive
/// path is caller-supplied and must be escaped correctly.
fn json_report(
    archive_dir: &Path,
    recovered: usize,
    summary: &DrainSummary,
    completed_at: i64,
) -> String {
    let value = serde_json::json!({
        "archive": archive_dir.display().to_string(),
        "completed_at_ms": completed_at,
        "recovered": recovered,
        "ingested": summary.ingested,
        "skipped": summary.skipped,
        "failed": summary.failed,
        "requeued": summary.requeued,
        "enriched": summary.enriched,
        "enrich_failed": summary.enrich_failed,
        "reverified": summary.reverified,
    });
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> DrainSummary {
        DrainSummary {
            ingested: 3,
            skipped: 1,
            failed: 0,
            requeued: 0,
            enriched: 2,
            enrich_failed: 0,
            reverified: 0,
        }
    }

    #[test]
    fn the_text_report_names_the_archive_and_the_counts() {
        let text = text_report(Path::new("/tmp/archive"), 0, &summary());
        assert!(text.contains("/tmp/archive"), "{text}");
        assert!(text.contains("ingested 3"), "{text}");
        assert!(text.contains("skipped 1"), "{text}");
        assert!(text.contains("failed 0"), "{text}");
    }

    #[test]
    fn a_quiet_scan_stays_quiet() {
        // Nothing recovered and nothing requeued must not print lines about
        // either; a normal run should not look eventful.
        let text = text_report(Path::new("/tmp/a"), 0, &summary());
        assert!(!text.contains("recovered"), "{text}");
        assert!(!text.contains("requeued"), "{text}");
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn recovered_and_requeued_are_reported_when_they_happen() {
        let mut s = summary();
        s.requeued = 4;
        let text = text_report(Path::new("/tmp/a"), 2, &s);
        assert!(text.contains("recovered 2"), "{text}");
        assert!(text.contains("requeued 4"), "{text}");
    }

    #[test]
    fn the_json_report_parses_and_carries_every_count() {
        let raw = json_report(Path::new("/tmp/archive"), 1, &summary(), 1_725_000_000_000);
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(value["archive"], "/tmp/archive");
        assert_eq!(value["completed_at_ms"], 1_725_000_000_000_i64);
        assert_eq!(value["recovered"], 1);
        assert_eq!(value["ingested"], 3);
        assert_eq!(value["skipped"], 1);
        assert_eq!(value["failed"], 0);
        assert_eq!(value["enriched"], 2);
    }

    #[test]
    fn the_json_report_escapes_an_awkward_archive_path() {
        // The path comes from `--archive`, so it is caller-supplied text landing
        // in a machine-read format. Hand-built JSON would break here.
        let raw = json_report(Path::new(r#"/tmp/we"ird\path"#), 0, &summary(), 0);
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(value["archive"], r#"/tmp/we"ird\path"#);
    }

    #[test]
    fn the_json_report_is_one_line() {
        // So a caller can read a scan's result with `read`/`jq` per line.
        let raw = json_report(Path::new("/tmp/a"), 0, &summary(), 0);
        assert!(!raw.contains('\n'), "{raw}");
    }

    #[test]
    fn now_ms_is_a_plausible_wall_clock() {
        // Guards the saturating conversion: a negative or zero stamp would make
        // "when did Lore last look?" unanswerable.
        assert!(now_ms() > 1_700_000_000_000, "clock before 2023");
    }
}
