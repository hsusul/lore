//! Lossless archive compaction: store each attachment body once, and return
//! free pages to the file system.
//!
//! Two independent wins, neither of which changes a byte any reader sees:
//!
//! - **One copy per attachment body.** Claude Code injects the same context
//!   into many sessions, so most attachment parts repeat a body another part
//!   already holds. [`store_body`] keeps each distinct body once in
//!   `part_body`, addressed by the BLAKE3 hash of its exact bytes, and the part
//!   points at it through `body_hash`. Every reader goes through the
//!   `message_part_full` view (migration 0019), which returns `content_json`
//!   exactly as ingested however the row is stored. [`dedupe_attachments`]
//!   converts rows ingested before this existed.
//! - **Free pages leave the file.** A forget or a re-ingest frees pages that
//!   SQLite otherwise keeps inside the file for reuse. With `auto_vacuum =
//!   INCREMENTAL`, [`reclaim_free_pages`] hands them back. New archives get the
//!   mode at creation; an existing one needs one full `VACUUM`
//!   ([`vacuum_once`]), which runs only when the volume has room for it.
//!
//! Lossless is checked, not assumed: a row's old column is cleared only after
//! its body reads back byte-equal through the view, and the conversion stops
//! at the first row that does not, leaving that row as it was.

use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::{Result, StorageError};

/// The part kind whose bodies are stored once. Attachments are where the
/// duplication is: 948 MB of `content_json` on one archive, 89 MB distinct.
pub const ATTACHMENT: &str = "attachment";

/// Rows converted per transaction by [`dedupe_attachments`]. Small enough that
/// the write lock is held briefly, large enough that commits do not dominate.
pub const DEDUPE_BATCH: usize = 500;

/// Free pages below which [`reclaim_if_worthwhile`] leaves the file alone: 8
/// MiB at the default 4 KiB page. Smaller amounts are reused by the next
/// ingest anyway, and truncating and regrowing the file for them is churn.
pub const RECLAIM_MIN_FREE_PAGES: i64 = 2048;

/// The setting that records the one-time `VACUUM`'s outcome, content-free.
pub const VACUUM_SETTING: &str = "compact.vacuum";

/// The setting that records why the last [`compact`] failed, content-free:
/// `{"stage": "backup" | "dedupe" | "vacuum", "sqlite_code": n | null}`,
/// or JSON `null` after a run that finished without error.
pub const ERROR_SETTING: &str = "compact.error";

/// BLAKE3 of `body`'s exact bytes, as lowercase hex. The address of a body in
/// `part_body`.
#[must_use]
pub fn body_hash(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

/// Store `body` once in `part_body` and return its hash, or `None` when the
/// row already held under that hash is not byte-equal to `body`. That never
/// happens unless the table was damaged, and the caller then keeps the body
/// inline, so a bad row can never stand in for good content.
///
/// Runs in the caller's transaction.
pub(crate) fn store_body(tx: &Connection, body: &str) -> Result<Option<String>> {
    let hash = body_hash(body);
    tx.prepare_cached(
        "INSERT INTO part_body (hash, body) VALUES (?1, ?2) ON CONFLICT(hash) DO NOTHING",
    )?
    .execute(params![hash, body])?;
    let same: bool = tx
        .prepare_cached("SELECT body FROM part_body WHERE hash = ?1")?
        .query_row([&hash], |row| {
            Ok(row.get_ref(0)?.as_bytes()? == body.as_bytes())
        })?;
    Ok(same.then_some(hash))
}

/// Delete the bodies in `hashes` that no part references any more. Runs in the
/// caller's transaction, after the parts that used them were deleted.
pub(crate) fn delete_orphan_bodies(tx: &Connection, hashes: &[String]) -> Result<usize> {
    let mut removed = 0;
    let mut delete = tx.prepare_cached(
        "DELETE FROM part_body WHERE hash = ?1
           AND NOT EXISTS (SELECT 1 FROM message_part WHERE body_hash = ?1)",
    )?;
    for hash in hashes {
        removed += delete.execute([hash])?;
    }
    Ok(removed)
}

/// The distinct body hashes a session's parts reference. Read before the
/// session's rows are deleted, so the bodies only it used can be found after.
pub(crate) fn session_body_hashes(tx: &Connection, session_id: &str) -> Result<Vec<String>> {
    let mut stmt = tx.prepare_cached(
        "SELECT DISTINCT p.body_hash FROM message_part p
         JOIN message m ON m.id = p.message_id
         WHERE m.session_id = ?1 AND p.body_hash IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([session_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Delete every body no part references. The archive-wide form of
/// [`delete_orphan_bodies`], for "forget everything".
pub(crate) fn delete_all_orphan_bodies(tx: &Connection) -> Result<usize> {
    Ok(tx.execute(
        "DELETE FROM part_body WHERE NOT EXISTS
            (SELECT 1 FROM message_part p WHERE p.body_hash = part_body.hash)",
        [],
    )?)
}

/// Outcome of [`dedupe_attachments`]. Counts only, never content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DedupeReport {
    /// Attachment rows moved to `part_body` by this call.
    pub converted: u64,
    /// Distinct bodies this call added to `part_body`.
    pub bodies_added: u64,
    /// Attachment rows still stored inline afterwards: zero once the
    /// conversion is complete, nonzero if it was stopped.
    pub remaining: u64,
}

/// Convert every attachment row still stored inline to the one-copy form.
///
/// Batched: each batch of `batch` rows is one transaction under the archive
/// write lock, so a crash or a stop loses at most the batch in flight, and a
/// rerun continues with the rows still inline. Rerunning a finished conversion
/// is a no-op.
///
/// Verified: for each row the body is stored, the row is pointed at it, and
/// the body read back through `message_part_full` is compared byte for byte
/// with the old column. Only then is the old column cleared. The first row
/// that does not compare equal is left exactly as it was, the rows before it
/// are committed, and the conversion stops with an error naming that part.
///
/// `stop` is checked between batches.
pub fn dedupe_attachments(
    conn: &Connection,
    batch: usize,
    stop: &AtomicBool,
) -> Result<DedupeReport> {
    let mut report = DedupeReport::default();
    let mut after: i64 = 0;
    let limit = i64::try_from(batch.max(1)).unwrap_or(i64::MAX);
    while !stop.load(Ordering::Relaxed) {
        let _write = crate::storage::write_lock();
        // IMMEDIATE: the write lock is taken at BEGIN, where a busy archive
        // waits out the busy timeout. A deferred transaction would read first
        // and then fail its first write with SQLITE_BUSY_SNAPSHOT, which no
        // timeout retries, whenever another connection committed in between.
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let rows: Vec<(i64, String, String)> = tx
            .prepare_cached(
                "SELECT rowid, id, content_json FROM message_part
                 WHERE rowid > ?1 AND kind = ?2
                   AND body_hash IS NULL AND content_json IS NOT NULL
                 ORDER BY rowid LIMIT ?3",
            )?
            .query_map(params![after, ATTACHMENT, limit], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let Some(last) = rows.last() else {
            break;
        };
        after = last.0;
        for (_, id, body) in &rows {
            tx.execute_batch("SAVEPOINT dedupe_part")?;
            match convert_row(&tx, id, body)? {
                Some(added) => {
                    tx.execute_batch("RELEASE dedupe_part")?;
                    report.converted += 1;
                    report.bodies_added += u64::from(added);
                }
                None => {
                    tx.execute_batch("ROLLBACK TO dedupe_part; RELEASE dedupe_part")?;
                    tx.commit()?;
                    return Err(StorageError::Migration(format!(
                        "attachment body of part {id} did not read back byte-equal; \
                         the conversion stopped and left that part as it was"
                    )));
                }
            }
        }
        tx.commit()?;
    }
    report.remaining = inline_attachments(conn)?;
    Ok(report)
}

/// Move one row's body to `part_body`. `Some(added)` when it read back
/// byte-equal and the old column was cleared (`added` says whether the body
/// was new); `None` when it did not, in which case the caller rolls back.
fn convert_row(tx: &Connection, part_id: &str, body: &str) -> Result<Option<bool>> {
    let hash = body_hash(body);
    let added = tx
        .prepare_cached(
            "INSERT INTO part_body (hash, body) VALUES (?1, ?2) ON CONFLICT(hash) DO NOTHING",
        )?
        .execute(params![hash, body])?
        > 0;
    tx.prepare_cached("UPDATE message_part SET body_hash = ?2 WHERE id = ?1")?
        .execute(params![part_id, hash])?;
    // The view now serves the row from part_body while the old column still
    // holds the original, so the two can be compared before anything is lost.
    let equal: bool = tx
        .prepare_cached(
            "SELECT v.content_json, p.content_json
             FROM message_part_full v JOIN message_part p ON p.id = v.id
             WHERE v.id = ?1",
        )?
        .query_row([part_id], |row| {
            let served = row.get_ref(0)?;
            let original = row.get_ref(1)?;
            Ok(
                match (served.as_bytes_or_null()?, original.as_bytes_or_null()?) {
                    (Some(served), Some(original)) => {
                        served == original && original == body.as_bytes()
                    }
                    _ => false,
                },
            )
        })?;
    if !equal {
        return Ok(None);
    }
    tx.prepare_cached("UPDATE message_part SET content_json = NULL WHERE id = ?1")?
        .execute([part_id])?;
    Ok(Some(added))
}

/// Attachment rows still stored inline.
pub fn inline_attachments(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM message_part
         WHERE kind = ?1 AND body_hash IS NULL AND content_json IS NOT NULL",
        [ATTACHMENT],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Pages on the archive's freelist.
pub fn freelist_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?)
}

/// Whether the archive returns free pages on `PRAGMA incremental_vacuum`.
pub fn is_incremental(conn: &Connection) -> Result<bool> {
    let mode: i64 = conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
    Ok(mode == 2)
}

/// Return every free page to the file system. A no-op on an archive not yet
/// in incremental mode. The caller holds the write lock and has no
/// transaction open.
pub fn reclaim_free_pages(conn: &Connection) -> Result<()> {
    // The pragma frees one page per step, so it is stepped to the end: run as
    // a batch it would free a single page and stop.
    let mut stmt = conn.prepare("PRAGMA incremental_vacuum")?;
    let mut rows = stmt.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// [`reclaim_free_pages`] when at least [`RECLAIM_MIN_FREE_PAGES`] are free,
/// for paths that run often, such as after an ingest pass. Takes the write
/// lock itself. Best-effort: reclaiming is an optimization, so a failure is
/// swallowed rather than reported to a caller whose work already committed.
pub fn reclaim_if_worthwhile(conn: &Connection) {
    let worthwhile = matches!(is_incremental(conn), Ok(true))
        && freelist_count(conn).is_ok_and(|n| n >= RECLAIM_MIN_FREE_PAGES);
    if worthwhile {
        let _write = crate::storage::write_lock();
        let _ = reclaim_free_pages(conn);
    }
}

/// Outcome of [`vacuum_once`]. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VacuumOutcome {
    /// The archive is already in incremental mode; nothing to do.
    AlreadyIncremental,
    /// Attachment rows are still inline. The `VACUUM` waits for the
    /// conversion, so the file is rewritten once, at its smallest.
    DedupePending,
    /// No file behind the connection (an in-memory archive).
    NoFile,
    /// Too little free space for the `VACUUM`: it writes a full copy of the
    /// archive to SQLite's temporary directory, then writes every page of it
    /// again through the archive's write-ahead log. Each volume needs the
    /// archive's size for each of the two it holds, so one volume holding both
    /// needs twice the size. `needed` and `available` are for the volume that
    /// fell short.
    LowSpace { needed: u64, available: u64 },
    /// The archive was rewritten in incremental mode.
    Vacuumed { pages_before: i64, pages_after: i64 },
}

impl VacuumOutcome {
    /// The stable value recorded under [`VACUUM_SETTING`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyIncremental => "already_incremental",
            Self::DedupePending => "dedupe_pending",
            Self::NoFile => "no_file",
            Self::LowSpace { .. } => "skipped_low_space",
            Self::Vacuumed { .. } => "vacuumed",
        }
    }
}

/// Free space where a `VACUUM` writes, in bytes: the volume holding the
/// archive (its write-ahead log) and the one holding SQLite's temporary
/// directory (the rewritten copy). `same_volume` when they are one volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VacuumSpace {
    pub archive: u64,
    pub temp: u64,
    pub same_volume: bool,
}

impl VacuumSpace {
    /// The same free space for both, on one volume. For tests.
    #[must_use]
    pub fn one_volume(available: u64) -> Self {
        Self {
            archive: available,
            temp: available,
            same_volume: true,
        }
    }

    /// `None` when a `VACUUM` of an archive of `size` bytes fits, else the
    /// shortfall as `(needed, available)` on the volume that lacks it.
    fn shortfall(self, size: u64) -> Option<(u64, u64)> {
        if self.same_volume {
            let needed = size.saturating_mul(2);
            return (self.archive < needed).then_some((needed, self.archive));
        }
        if self.temp < size {
            return Some((size, self.temp));
        }
        (self.archive < size).then_some((size, self.archive))
    }
}

/// Switch an existing archive to `auto_vacuum = INCREMENTAL` with the one
/// full `VACUUM` that takes, after the attachment conversion has finished and
/// only when there is room for it ([`VacuumSpace`]). Otherwise it skips, and a
/// later launch tries again.
///
/// The outcome is recorded under [`VACUUM_SETTING`] so a skip is visible
/// afterwards. The `VACUUM` holds the write lock for its duration (seconds on
/// a multi-gigabyte archive), once.
pub fn vacuum_once(conn: &Connection) -> Result<VacuumOutcome> {
    let space = match conn.path().filter(|p| !p.is_empty()) {
        Some(path) => vacuum_space(std::path::Path::new(path))?,
        None => VacuumSpace::one_volume(0),
    };
    vacuum_once_with(conn, space)
}

/// [`vacuum_once`] with the free space given, so the space rule can be tested
/// without filling a disk.
pub fn vacuum_once_with(conn: &Connection, space: VacuumSpace) -> Result<VacuumOutcome> {
    let outcome = decide_vacuum(conn, space)?;
    let outcome = match outcome {
        Some(outcome) => outcome,
        None => {
            let pages_before = page_count(conn)?;
            {
                let _write = crate::storage::write_lock();
                crate::storage::checkpoint(conn);
                // The rewritten copy goes to a temporary file, not memory:
                // with the connection's usual `temp_store = MEMORY` it would
                // hold the whole archive in RAM. Restored whatever happens.
                conn.execute_batch("PRAGMA temp_store = FILE;")?;
                let vacuumed = conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;");
                conn.execute_batch("PRAGMA temp_store = MEMORY;")?;
                vacuumed?;
            }
            crate::storage::checkpoint(conn);
            VacuumOutcome::Vacuumed {
                pages_before,
                pages_after: page_count(conn)?,
            }
        }
    };
    if outcome != VacuumOutcome::NoFile {
        // `settings::set` takes the write lock itself.
        crate::settings::set(conn, VACUUM_SETTING, &format!("\"{}\"", outcome.as_str()))?;
    }
    Ok(outcome)
}

/// `Some(outcome)` when the `VACUUM` should not run, `None` when it should.
fn decide_vacuum(conn: &Connection, space: VacuumSpace) -> Result<Option<VacuumOutcome>> {
    if is_incremental(conn)? {
        return Ok(Some(VacuumOutcome::AlreadyIncremental));
    }
    if conn.path().filter(|p| !p.is_empty()).is_none() {
        return Ok(Some(VacuumOutcome::NoFile));
    }
    if inline_attachments(conn)? > 0 {
        return Ok(Some(VacuumOutcome::DedupePending));
    }
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let size = u64::try_from(page_count(conn)?.saturating_mul(page_size)).unwrap_or(u64::MAX);
    if let Some((needed, available)) = space.shortfall(size) {
        return Ok(Some(VacuumOutcome::LowSpace { needed, available }));
    }
    Ok(None)
}

fn page_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA page_count", [], |row| row.get(0))?)
}

/// Free space beside the archive at `db_path` and in SQLite's temporary
/// directory.
fn vacuum_space(db_path: &std::path::Path) -> Result<VacuumSpace> {
    let archive_dir = db_path.parent().unwrap_or(db_path);
    let temp_dir = sqlite_temp_dir();
    let (archive, archive_dev) = free_bytes(archive_dir)?;
    let (temp, temp_dev) = free_bytes(&temp_dir)?;
    Ok(VacuumSpace {
        archive,
        temp,
        same_volume: archive_dev == temp_dev,
    })
}

/// Bytes available to this user on the volume holding `dir`, and that
/// volume's device id.
fn free_bytes(dir: &std::path::Path) -> Result<(u64, rustix::fs::Dev)> {
    let stat = rustix::fs::statvfs(dir).map_err(|_| StorageError::Io)?;
    let dev = rustix::fs::stat(dir).map_err(|_| StorageError::Io)?.st_dev;
    Ok((stat.f_bavail.saturating_mul(stat.f_frsize), dev))
}

/// The directory SQLite writes temporary files to on unix: the first usable
/// of `SQLITE_TMPDIR`, `TMPDIR`, `/var/tmp`, `/usr/tmp` and `/tmp`, else the
/// working directory. The same order SQLite's unix VFS searches
/// (`unixTempFileDir`), so the space check covers the volume the copy lands on.
fn sqlite_temp_dir() -> std::path::PathBuf {
    let env = ["SQLITE_TMPDIR", "TMPDIR"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(std::path::PathBuf::from);
    let fixed = ["/var/tmp", "/usr/tmp", "/tmp"]
        .into_iter()
        .map(std::path::PathBuf::from);
    env.chain(fixed)
        .find(|dir| {
            dir.is_dir()
                && rustix::fs::access(
                    dir.as_path(),
                    rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
                )
                .is_ok()
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Outcome of [`compact`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactReport {
    pub dedupe: DedupeReport,
    pub vacuum: VacuumOutcome,
}

/// Convert inline attachments, then switch the file to incremental mode once.
/// The desktop worker runs this after its startup scan, at the lowest
/// priority the worker has: nothing else is queued by then.
///
/// Before the first batch, the archive is backed up unless a backup under an
/// hour old exists ([`crate::backup::ensure_recent_backup`]); without one,
/// nothing is converted.
///
/// The outcome is recorded under [`ERROR_SETTING`], so a failure is visible
/// to a caller that runs this in the background and cannot report it.
pub fn compact(conn: &Connection, stop: &AtomicBool) -> Result<CompactReport> {
    let result = compact_stages(conn, stop);
    let recorded = match &result {
        Ok(_) => "null".to_owned(),
        Err((stage, error)) => {
            let code = match error {
                StorageError::Sqlite(rusqlite::Error::SqliteFailure(e, _)) => {
                    e.extended_code.to_string()
                }
                _ => "null".to_owned(),
            };
            format!("{{\"stage\":\"{stage}\",\"sqlite_code\":{code}}}")
        }
    };
    // Best-effort: failing to record must not replace the error itself.
    let _ = crate::settings::set(conn, ERROR_SETTING, &recorded);
    result.map_err(|(_, error)| error)
}

/// [`compact`]'s stages, each error tagged with the stage it came from.
fn compact_stages(
    conn: &Connection,
    stop: &AtomicBool,
) -> std::result::Result<CompactReport, (&'static str, StorageError)> {
    back_up_before_converting(conn).map_err(|e| ("backup", e))?;
    let dedupe = dedupe_attachments(conn, DEDUPE_BATCH, stop).map_err(|e| ("dedupe", e))?;
    let vacuum = if stop.load(Ordering::Relaxed) {
        VacuumOutcome::DedupePending
    } else {
        vacuum_once(conn).map_err(|e| ("vacuum", e))?
    };
    Ok(CompactReport { dedupe, vacuum })
}

/// Back up a file archive that still has rows to convert.
fn back_up_before_converting(conn: &Connection) -> Result<()> {
    if inline_attachments(conn)? == 0 {
        return Ok(());
    }
    let Some(dir) = crate::backup::backup_dir_for(conn) else {
        return Ok(());
    };
    crate::backup::ensure_recent_backup(
        conn,
        &dir,
        crate::now_ms(),
        crate::backup::RECENT_BACKUP_MS,
    )
    .map_err(|_| {
        StorageError::Migration(
            "could not back up the archive before compacting it; nothing was converted".into(),
        )
    })?;
    Ok(())
}

/// The stored body of a part, read through the one read path. For tests and
/// tools that check a single row; transcript reads select from the view.
pub fn part_content(conn: &Connection, part_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT content_json FROM message_part_full WHERE id = ?1",
            [part_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_volume_needs_twice_the_size() {
        let space = VacuumSpace::one_volume(1999);
        assert_eq!(space.shortfall(1000), Some((2000, 1999)));
        assert_eq!(VacuumSpace::one_volume(2000).shortfall(1000), None);
    }

    #[test]
    fn two_volumes_each_need_the_size() {
        let split = |archive, temp| VacuumSpace {
            archive,
            temp,
            same_volume: false,
        };
        assert_eq!(split(1000, 1000).shortfall(1000), None);
        assert_eq!(split(1000, 999).shortfall(1000), Some((1000, 999)), "temp");
        assert_eq!(
            split(999, 5000).shortfall(1000),
            Some((1000, 999)),
            "archive"
        );
    }

    #[test]
    fn the_temp_dir_is_a_writable_directory() {
        assert!(sqlite_temp_dir().is_dir());
    }
}
