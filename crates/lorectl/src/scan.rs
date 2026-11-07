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
//!   → stamp scan.last_completed_at → compact()
//! ```
//!
//! After a scan that finished, `lorectl scan` compacts the archive the way the
//! desktop worker does after its startup scan, with the same
//! [`lore_core::compact::compact`]: a backup before the first converted row,
//! old attachment and tool rows moved to stored-once bodies, failures recorded
//! under `compact.error`, and the one `VACUUM` only with room for it. Still
//! under the writer lock. `--no-compact` skips it.
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

use lore_core::adapters::AdapterRegistry;
use lore_core::compact::{CompactReport, SlimVacuumOutcome, VacuumOutcome};
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
    let since = invocation.since.as_deref().map(window).transpose()?;
    let archive_dir = invocation.archive_dir()?;
    let at = since.as_ref().map(|(_, at)| *at);
    let (conn, recovered, summary, _, compacted) =
        scan_until(&archive_dir, at, None, !invocation.no_compact)?;
    // A failed compaction leaves every row it had not verified as it was and
    // is recorded under `compact.error`; the scan itself completed, so it is
    // said, not fatal.
    let compacted = compacted.map(|result| {
        result
            .map(|report| (report, page_size(&conn)))
            .map_err(|e| {
                eprintln!("lorectl: compacting the archive failed ({e}); the scan completed");
                e.to_string()
            })
    });

    // `Worker::scan` already stamped completion; read it back so the report
    // shows exactly what a later `status` will read, rather than a second clock
    // reading that could differ.
    let completed_at = match since {
        Some(_) => settings::last_window_scan(&conn)?.map(|(at, _)| at),
        None => settings::last_scan_completed_at(&conn)?,
    }
    .unwrap_or_else(now_ms);

    let mut stdout = std::io::stdout().lock();
    let report = if invocation.json {
        json_report(
            &archive_dir,
            recovered,
            &summary,
            completed_at,
            compacted.as_ref(),
        )
    } else {
        let mut text = text_report(&archive_dir, recovered, &summary);
        if let Some((given, _)) = &since {
            text.push_str(&format!("\nonly sources modified in the last {given}"));
        }
        if let Some(Ok((report, page_size))) = &compacted {
            text.push_str(&compact_text(report, *page_size));
        }
        text
    };
    let _ = writeln!(stdout, "{report}");

    Ok(exit::OK)
}

/// What [`scan_until`] returns: the archive connection, the recovered job
/// count, what the scan did, how many sources it left queued, and the
/// compaction's outcome when it ran.
type Scanned = (
    rusqlite::Connection,
    usize,
    DrainSummary,
    usize,
    Option<storage::Result<CompactReport>>,
);

/// Scan every configured source root into the archive at `archive_dir`,
/// creating it if need be, and only sources modified since `since` when
/// given, starting no new source after `deadline`. Then compact the archive
/// when `compact` is set and the scan left nothing queued. `lorectl init`
/// scans through this too.
pub(crate) fn scan_until(
    archive_dir: &Path,
    since: Option<std::time::SystemTime>,
    deadline: Option<std::time::Instant>,
    compact: bool,
) -> Result<Scanned, CliError> {
    std::fs::create_dir_all(archive_dir).map_err(|_| CliError::Storage(StorageError::Io))?;

    // Before anything opens the database. See the module docs.
    let _lock = ScanLock::acquire(archive_dir)?;

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
    let (summary, left) = worker.scan_until(&NullSink, since, deadline)?;
    // Only after a scan that finished, as the desktop worker does, and still
    // under the lock: it converts rows and may rewrite the file.
    let compacted = (compact && left == 0).then(|| worker.compact());

    // Close the worker's connection before stamping, so the write below is the
    // only one open. Not required for correctness — WAL permits both, and the
    // worker is idle by now — but it keeps "the scan is over" a fact about the
    // process rather than a claim about the worker's internal state.
    drop(worker);
    Ok((conn, recovered, summary, left, compacted))
}

fn page_size(conn: &rusqlite::Connection) -> u64 {
    conn.query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap_or(0)
}

/// Rows a compaction moved to stored-once bodies.
fn converted(report: &CompactReport) -> u64 {
    report.dedupe.converted + report.slim.converted
}

/// Bytes a compaction returned to the file system: the `VACUUM`'s, or the
/// free pages it reclaimed on an archive already in incremental mode, plus
/// the `VACUUM` after the tool output conversion.
fn reclaimed_bytes(report: &CompactReport, page_size: u64) -> u64 {
    let mut pages = match report.vacuum {
        VacuumOutcome::Vacuumed {
            pages_before,
            pages_after,
        } => pages_before.saturating_sub(pages_after),
        _ => report.reclaimed_pages,
    };
    if let SlimVacuumOutcome::Vacuumed {
        pages_before,
        pages_after,
    } = report.vacuum_after_slim
    {
        pages = pages.saturating_add(pages_before.saturating_sub(pages_after));
    }
    u64::try_from(pages).unwrap_or(0).saturating_mul(page_size)
}

/// The compaction's lines in the text report: nothing when it did nothing.
fn compact_text(report: &CompactReport, page_size: u64) -> String {
    let (rows, bytes) = (converted(report), reclaimed_bytes(report, page_size));
    let mut text = String::new();
    if rows > 0 || bytes > 0 {
        text.push_str(&format!(
            "\ncompacted {rows} row(s), reclaimed {bytes} bytes"
        ));
    }
    if let VacuumOutcome::LowSpace { needed, available } = report.vacuum {
        text.push_str(&format!(
            "\ncompact skipped the one-time VACUUM: it needs {needed} bytes free, \
             {available} available"
        ));
    }
    if let SlimVacuumOutcome::LowSpace { needed, available } = report.vacuum_after_slim {
        text.push_str(&format!(
            "\ncompact skipped the VACUUM after the tool output conversion: it needs \
             {needed} bytes free, {available} available"
        ));
    }
    text
}

/// `--since` as given and the moment it reaches back to: a whole number of
/// days, hours, or minutes, such as `30d`, `12h`, or `90m`.
pub(crate) fn window(given: &str) -> Result<(String, std::time::SystemTime), CliError> {
    let bad = || CliError::Usage(format!("--since `{given}` must be like 30d, 12h, or 90m"));
    let unit = match given.chars().last() {
        Some('d') => 86_400,
        Some('h') => 3_600,
        Some('m') => 60,
        _ => return Err(bad()),
    };
    let count: u64 = given[..given.len() - 1].parse().map_err(|_| bad())?;
    if count == 0 {
        return Err(bad());
    }
    let back = std::time::Duration::from_secs(count.checked_mul(unit).ok_or_else(bad)?);
    let at = std::time::SystemTime::now()
        .checked_sub(back)
        .ok_or_else(bad)?;
    Ok((given.to_owned(), at))
}

/// Epoch millis, saturating rather than panicking on a clock before the epoch.
/// Shared with `status`, which reads the stamp this writes.
pub fn now_ms() -> i64 {
    lore_core::now_ms()
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
    compacted: Option<&Result<(CompactReport, u64), String>>,
) -> String {
    let compact = match compacted {
        None => serde_json::Value::Null,
        Some(Ok((report, page_size))) => serde_json::json!({
            "converted": converted(report),
            "failed": report.dedupe.failed + report.slim.failed,
            "vacuum": report.vacuum.as_str(),
            "vacuum_after_slim": report.vacuum_after_slim.as_str(),
            "reclaimed_bytes": reclaimed_bytes(report, *page_size),
        }),
        Some(Err(error)) => serde_json::json!({ "error": error }),
    };
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
        "compact": compact,
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
        let raw = json_report(
            Path::new("/tmp/archive"),
            1,
            &summary(),
            1_725_000_000_000,
            None,
        );
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
        let raw = json_report(Path::new(r#"/tmp/we"ird\path"#), 0, &summary(), 0, None);
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(value["archive"], r#"/tmp/we"ird\path"#);
    }

    #[test]
    fn the_json_report_is_one_line() {
        // So a caller can read a scan's result with `read`/`jq` per line.
        let raw = json_report(Path::new("/tmp/a"), 0, &summary(), 0, None);
        assert!(!raw.contains('\n'), "{raw}");
    }

    #[test]
    fn now_ms_is_a_plausible_wall_clock() {
        // Guards the saturating conversion: a negative or zero stamp would make
        // "when did Lore last look?" unanswerable.
        assert!(now_ms() > 1_700_000_000_000, "clock before 2023");
    }

    #[test]
    fn window_requires_positive_count() {
        assert!(window("0d").is_err());
        assert!(window("0h").is_err());
        assert!(window("0m").is_err());
        assert!(window("30d").is_ok());
    }
}
