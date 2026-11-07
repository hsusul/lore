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
//! - **One copy per tool output.** A tool result keeps its output in `text`
//!   and again inside its raw payload. Where the payload holds that output
//!   exactly once, it is stored with the output replaced by a marker
//!   ([`slim_tool_output`]) and `message_part_full` (migration 0020) puts it
//!   back from `text`. [`slim_tool_outputs`] converts older rows.
//! - **Bodies compressed.** Once the conversions above are done,
//!   [`compress_bodies`] trains one zstd dictionary per compressed column
//!   ([`crate::codec::COLUMNS`]) from a sample of the archive's own rows and
//!   rewrites the rows compressed. The views of migration 0021 decode them,
//!   and later ingests write compressed values directly.
//! - **Free pages leave the file.** A forget or a re-ingest frees pages that
//!   SQLite otherwise keeps inside the file for reuse. With `auto_vacuum =
//!   INCREMENTAL`, [`reclaim_free_pages`] hands them back. New archives get the
//!   mode at creation; an existing one needs one full `VACUUM`
//!   ([`vacuum_once`]), which runs only when the volume has room for it.
//!
//! Lossless is checked, not assumed: a row's old column is cleared only after
//! its body reads back byte-equal through the view. A row that does not is
//! left exactly as it was, counted, and skipped, so one bad row neither loses
//! data nor stalls the rest of the archive.

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

/// The setting that records how many rows the last [`compact`] could not
/// verify and left as they were: attachments left inline
/// ([`DedupeReport::failed`]) and values left uncompressed
/// ([`ZstdReport::failed`]). A count only.
pub const FAILED_ROWS_SETTING: &str = "compact.failed_rows";

/// The attachment part of [`FAILED_ROWS_SETTING`]: rows the backup check
/// tolerates as inline.
pub const DEDUPE_FAILED_SETTING: &str = "compact.dedupe_failed_rows";

/// The setting that records why the last [`compact`] failed, content-free:
/// `{"stage": "backup" | "dedupe" | "slim" | "zstd" | "vacuum" | "reclaim",
/// "sqlite_code": n | null}`,
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
/// Stored compressed once `part_body.body` has a dictionary. The row held
/// under the hash is decoded here rather than through `part_body_full`, so a
/// damaged one also reads as "not the same" instead of failing the ingest.
///
/// Runs in the caller's transaction.
pub(crate) fn store_body(
    tx: &Connection,
    packer: &crate::codec::Packer,
    body: &str,
) -> Result<Option<String>> {
    let hash = body_hash(body);
    tx.prepare_cached(
        "INSERT INTO part_body (hash, body) VALUES (?1, ?2) ON CONFLICT(hash) DO NOTHING",
    )?
    .execute(params![hash, packer.pack(&crate::codec::PART_BODY, body)])?;
    let same: bool = tx
        .prepare_cached(
            "SELECT body, (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_id(body))
             FROM part_body WHERE hash = ?1",
        )?
        .query_row([&hash], |row| {
            let dict = row.get_ref(1)?.as_blob_or_null()?;
            Ok(crate::codec::decode(row.get_ref(0)?, dict)
                .is_ok_and(|stored| stored.as_deref() == Some(body)))
        })?;
    Ok(same.then_some(hash))
}

/// The part kind whose output is stored once. A tool result keeps its output
/// in `text` and again inside its raw payload: 778 MB for 389 MB of output on
/// one archive.
pub const TOOL_RESULT: &str = "tool_result";

/// Stands for `json_quote(text)` in a slim tool result payload: the output as
/// a JSON string value. A raw `\x01` never appears in JSON a parser accepts.
pub const TEXT_QUOTED_MARKER: &str = "\u{1}lore:text:quoted\u{1}";

/// Stands for `text` verbatim in a slim tool result payload: the output held
/// as raw JSON, as Codex records an array output.
pub const TEXT_RAW_MARKER: &str = "\u{1}lore:text:raw\u{1}";

/// The setting holding the last `message_part` rowid ingested before migration
/// 0020. Rows after it were ingested in the slim form already.
pub const TOOL_OUTPUT_UNTIL_SETTING: &str = "compact.tool_output_until";

/// The setting holding the last rowid [`slim_tool_outputs`] has examined.
pub const TOOL_OUTPUT_AFTER_SETTING: &str = "compact.tool_output_after";

/// The setting counting tool result rows whose slim form did not read back
/// byte-equal and were left as they were. A count only.
pub const TOOL_OUTPUT_FAILED_SETTING: &str = "compact.tool_output_failed_rows";

/// The slim form of a tool result payload, or `None` when the row keeps both
/// columns as they are.
///
/// The rule: `content` holds no `\x01` byte, and holds the output exactly
/// once, either as a JSON string value (`json_quote(text)`, tried first) or,
/// when the output is itself a JSON array or object (a Codex array output),
/// verbatim. That one occurrence is
/// replaced by [`TEXT_QUOTED_MARKER`] or [`TEXT_RAW_MARKER`], and only when
/// the marker is shorter than what it replaces. `message_part_full` reverses
/// it; callers still compare the rebuilt bytes before relying on it.
#[must_use]
pub fn slim_tool_output(text: &str, content: &str) -> Option<String> {
    if content.contains('\u{1}') {
        return None;
    }
    let quoted = serde_json::to_string(text).ok()?;
    replace_once(content, &quoted, TEXT_QUOTED_MARKER).or_else(|| {
        let structured =
            text.starts_with(['[', '{']) && serde_json::from_str::<serde_json::Value>(text).is_ok();
        structured
            .then(|| replace_once(content, text, TEXT_RAW_MARKER))
            .flatten()
    })
}

/// `hay` with its one occurrence of `needle` replaced by `marker`; `None` when
/// `needle` occurs other than exactly once or is no longer than `marker`.
fn replace_once(hay: &str, needle: &str, marker: &str) -> Option<String> {
    if needle.len() <= marker.len() {
        return None;
    }
    let at = hay.find(needle)?;
    let rest = &hay[at + needle.len()..];
    if rest.contains(needle) {
        return None;
    }
    let mut out = String::with_capacity(hay.len() - needle.len() + marker.len());
    out.push_str(&hay[..at]);
    out.push_str(marker);
    out.push_str(rest);
    Some(out)
}

/// Whether `message_part_full` serves `expected` for part `part_id`, byte for
/// byte. Runs in the caller's transaction.
pub(crate) fn serves_exactly(tx: &Connection, part_id: &str, expected: &str) -> Result<bool> {
    Ok(tx
        .prepare_cached("SELECT content_json FROM message_part_full WHERE id = ?1")?
        .query_row([part_id], |row| {
            Ok(row.get_ref(0)?.as_bytes_or_null()? == Some(expected.as_bytes()))
        })
        .optional()?
        .unwrap_or(false))
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
    /// conversion is complete, nonzero if it was stopped or rows failed.
    pub remaining: u64,
    /// Attachment rows this call left inline because their body did not read
    /// back byte-equal. Each is exactly as it was before the call.
    pub failed: u64,
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
/// with the old column. Only then is the old column cleared. A row that does
/// not compare equal is rolled back to exactly what it was, counted in
/// [`DedupeReport::failed`], and skipped; the conversion carries on with the
/// next row. A rerun tries the failed rows again.
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
                   AND body_hash IS NULL AND typeof(content_json) = 'text'
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
                    report.failed += 1;
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

/// Outcome of [`slim_tool_outputs`]. Counts only, never content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlimReport {
    /// Tool result rows this call stored in the slim form.
    pub converted: u64,
    /// Tool result rows this call left as they were because the rule did not
    /// hold ([`slim_tool_output`]). Not a failure.
    pub kept: u64,
    /// Tool result rows whose slim form did not read back byte-equal. Each is
    /// exactly as it was before the call.
    pub failed: u64,
    /// Bytes this call removed from `content_json`.
    pub bytes_saved: u64,
    /// Whether rows ingested before migration 0020 are still unexamined: false
    /// once the conversion is complete, true if it was stopped.
    pub pending: bool,
}

fn setting_i64(conn: &Connection, key: &str) -> Result<i64> {
    Ok(conn
        .prepare_cached("SELECT value_json FROM setting WHERE key = ?1")?
        .query_row([key], |row| row.get::<_, String>(0))
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

fn put_setting_i64(tx: &Connection, key: &str, value: i64) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO setting (key, value_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                                        updated_at = excluded.updated_at",
    )?
    .execute(params![key, value.to_string(), crate::now_ms()])?;
    Ok(())
}

/// Whether tool result rows ingested before migration 0020 are still
/// unexamined by [`slim_tool_outputs`].
pub fn tool_outputs_pending(conn: &Connection) -> Result<bool> {
    let after = setting_i64(conn, TOOL_OUTPUT_AFTER_SETTING)?;
    let until = setting_i64(conn, TOOL_OUTPUT_UNTIL_SETTING)?;
    if after >= until {
        return Ok(false);
    }
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM message_part
                WHERE rowid > ?1 AND rowid <= ?2 AND kind = ?3)",
        )?
        .query_row(params![after, until, TOOL_RESULT], |row| row.get(0))?)
}

/// Store every tool result ingested before migration 0020 in the slim form
/// where [`slim_tool_output`]'s rule holds.
///
/// The same shape as [`dedupe_attachments`]: batches of `batch` rows, each one
/// IMMEDIATE transaction under the write lock that also advances the
/// [`TOOL_OUTPUT_AFTER_SETTING`] cursor, so a crash, an error or a stop loses
/// at most the batch in flight and a rerun continues after the last committed
/// one. Rerunning a finished conversion is a no-op.
///
/// Verified: each row is rewritten, read back through `message_part_full` and
/// compared byte for byte with the original payload before the batch commits.
/// A row that does not compare equal is rolled back to exactly what it was and
/// counted in [`SlimReport::failed`]. `stop` is checked between batches.
pub fn slim_tool_outputs(conn: &Connection, batch: usize, stop: &AtomicBool) -> Result<SlimReport> {
    let mut report = SlimReport::default();
    let limit = i64::try_from(batch.max(1)).unwrap_or(i64::MAX);
    while !stop.load(Ordering::Relaxed) {
        let _write = crate::storage::write_lock();
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let after = setting_i64(&tx, TOOL_OUTPUT_AFTER_SETTING)?;
        let until = setting_i64(&tx, TOOL_OUTPUT_UNTIL_SETTING)?;
        if after >= until {
            break;
        }
        // A value that is not UTF-8 text reads as `None` and is left alone,
        // not an error that would stall the conversion on every launch.
        let text_of = |v: rusqlite::types::ValueRef<'_>| v.as_str().ok().map(str::to_owned);
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = tx
            .prepare_cached(
                "SELECT rowid, id, text, content_json FROM message_part
                 WHERE rowid > ?1 AND rowid <= ?2 AND kind = ?3
                   AND text IS NOT NULL AND content_json IS NOT NULL
                 ORDER BY rowid LIMIT ?4",
            )?
            .query_map(params![after, until, TOOL_RESULT, limit], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    text_of(row.get_ref(2)?),
                    text_of(row.get_ref(3)?),
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let last = rows.last().map(|r| r.0);
        let mut failed: i64 = 0;
        for (_, id, text, content) in &rows {
            let (Some(text), Some(content)) = (text, content) else {
                report.kept += 1;
                continue;
            };
            let Some(slim) = slim_tool_output(text, content) else {
                report.kept += 1;
                continue;
            };
            tx.execute_batch("SAVEPOINT slim_part")?;
            tx.prepare_cached("UPDATE message_part SET content_json = ?2 WHERE id = ?1")?
                .execute(params![id, slim])?;
            if serves_exactly(&tx, id, content)? {
                tx.execute_batch("RELEASE slim_part")?;
                report.converted += 1;
                report.bytes_saved += (content.len() - slim.len()) as u64;
            } else {
                tx.execute_batch("ROLLBACK TO slim_part; RELEASE slim_part")?;
                failed += 1;
            }
        }
        // No rows left in range: the conversion is complete.
        put_setting_i64(&tx, TOOL_OUTPUT_AFTER_SETTING, last.unwrap_or(until))?;
        if failed > 0 {
            let total = setting_i64(&tx, TOOL_OUTPUT_FAILED_SETTING)? + failed;
            put_setting_i64(&tx, TOOL_OUTPUT_FAILED_SETTING, total)?;
        }
        tx.commit()?;
        report.failed += u64::try_from(failed).unwrap_or(0);
        if last.is_none() {
            break;
        }
    }
    report.pending = tool_outputs_pending(conn)?;
    Ok(report)
}

/// Attachment rows still stored inline and uncompressed, the rows
/// [`dedupe_attachments`] would try. A compressed one was left inline by an
/// earlier failure and then compressed in place.
pub fn inline_attachments(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM message_part
         WHERE kind = ?1 AND body_hash IS NULL AND typeof(content_json) = 'text'",
        [ATTACHMENT],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Rows rewritten per transaction by [`compress_bodies`].
pub const ZSTD_BATCH: usize = 1000;

/// How a column's dictionary is trained ([`compress_bodies`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Training {
    /// Largest dictionary, in bytes.
    pub dict_size: usize,
    /// Fewest distinct sample rows a dictionary is trained from. A column with
    /// fewer rows gets no dictionary and stays plain; a later run tries again.
    pub min_samples: usize,
    /// Rows probed for samples, spread evenly over the table's rowids.
    pub probes: usize,
    /// Most bytes taken from one row.
    pub sample_cap: usize,
    /// Most sample bytes in all, about a hundred times the dictionary, as
    /// zstd recommends.
    pub sample_budget: usize,
}

impl Default for Training {
    /// A 112 KiB dictionary: 2.6 to 4.9 times smaller bodies on a 2.66 GB
    /// archive at level 3.
    fn default() -> Self {
        Self {
            dict_size: 112 * 1024,
            min_samples: 1000,
            probes: 20_000,
            sample_cap: 16 * 1024,
            sample_budget: 100 * 112 * 1024,
        }
    }
}

/// Outcome of [`compress_bodies`]. Counts only, never content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZstdReport {
    /// Dictionaries this call trained and stored.
    pub dicts_trained: u64,
    /// Values this call stored compressed.
    pub converted: u64,
    /// Values that did not compress back to their exact bytes, or did not
    /// read back exactly through SQL. Each is exactly as it was.
    pub failed: u64,
    /// Bytes of the converted values before and after.
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Whether rows remain to convert: false once complete, true if stopped.
    pub pending: bool,
}

fn zstd_cursor_key(column: &crate::codec::Column) -> String {
    format!("compact.zstd_after.{}", column.key)
}

/// The column's current dictionary: id, bytes and the last rowid it covers.
fn column_dict(
    conn: &Connection,
    column: &crate::codec::Column,
) -> Result<Option<(u32, Vec<u8>, i64)>> {
    Ok(conn
        .prepare_cached(
            "SELECT id, dict, until_rowid FROM zstd_dict WHERE \"column\" = ?1
             ORDER BY id DESC LIMIT 1",
        )?
        .query_row([column.key], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?)
}

/// Whether any column has a dictionary and rows up to its `until_rowid` that
/// [`compress_bodies`] has not reached yet.
pub fn zstd_pending(conn: &Connection) -> Result<bool> {
    for column in crate::codec::COLUMNS {
        let Some((_, _, until)) = column_dict(conn, &column)? else {
            continue;
        };
        let after = setting_i64(conn, &zstd_cursor_key(&column))?;
        if after >= until {
            continue;
        }
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE rowid > ?1 AND rowid <= ?2)",
            column.table
        );
        if conn.query_row(&sql, params![after, until], |row| row.get::<_, bool>(0))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sample `column`'s plain values, spread evenly over the table's rowids.
fn sample_column(
    conn: &Connection,
    column: &crate::codec::Column,
    training: &Training,
) -> Result<Vec<Vec<u8>>> {
    let (lo, hi): (Option<i64>, Option<i64>) = conn.query_row(
        &format!("SELECT min(rowid), max(rowid) FROM {}", column.table),
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Ok(Vec::new());
    };
    let probes = i64::try_from(training.probes.max(1)).unwrap_or(i64::MAX);
    let step = ((hi - lo) / probes).max(1);
    let mut stmt = conn.prepare(&format!(
        "SELECT rowid, {c} FROM {t} WHERE rowid >= ?1 AND typeof({c}) = 'text'
         ORDER BY rowid LIMIT 1",
        c = column.column,
        t = column.table
    ))?;
    let mut samples = Vec::new();
    let mut total = 0;
    let mut last = None;
    let mut at = lo;
    while at <= hi && total < training.sample_budget {
        let row: Option<(i64, Vec<u8>)> = stmt
            .query_row([at], |row| {
                Ok((row.get(0)?, row.get_ref(1)?.as_bytes()?.to_vec()))
            })
            .optional()?;
        at = at.saturating_add(step);
        let Some((rowid, mut bytes)) = row else {
            break;
        };
        if last == Some(rowid) || bytes.len() < crate::codec::MIN_LEN {
            continue;
        }
        last = Some(rowid);
        bytes.truncate(training.sample_cap);
        total += bytes.len();
        samples.push(bytes);
    }
    Ok(samples)
}

/// Train and store a dictionary for `column` unless it has one or too few
/// rows. Whether one was stored.
fn train_column(
    conn: &Connection,
    column: &crate::codec::Column,
    training: &Training,
) -> Result<bool> {
    if column_dict(conn, column)?.is_some() {
        return Ok(false);
    }
    let samples = sample_column(conn, column, training)?;
    if samples.len() < training.min_samples {
        return Ok(false);
    }
    let Some(dict) = crate::codec::train(&samples, training.dict_size) else {
        return Ok(false);
    };
    let _write = crate::storage::write_lock();
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    // Every row up to the table's last rowid now is converted by compaction;
    // every later one is written by an ingest that sees this dictionary.
    tx.execute(
        &format!(
            "INSERT INTO zstd_dict (\"column\", dict, until_rowid, trained_at)
             SELECT ?1, ?2, COALESCE(max(rowid), 0), ?3 FROM {}",
            column.table
        ),
        params![column.key, dict, crate::now_ms()],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Rewrite `column`'s plain rows up to its dictionary's `until_rowid`
/// compressed, in verified batches.
fn convert_column(
    conn: &Connection,
    column: &crate::codec::Column,
    batch: usize,
    stop: &AtomicBool,
    report: &mut ZstdReport,
) -> Result<()> {
    let Some((id, dict, until)) = column_dict(conn, column)? else {
        return Ok(());
    };
    let dict = crate::codec::Dict::new(id, dict);
    let key = zstd_cursor_key(column);
    let limit = i64::try_from(batch.max(1)).unwrap_or(i64::MAX);
    let (t, c) = (column.table, column.column);
    let select = format!(
        "SELECT rowid, {c} FROM {t} WHERE rowid > ?1 AND rowid <= ?2
           AND typeof({c}) = 'text' ORDER BY rowid LIMIT ?3"
    );
    let update = format!("UPDATE {t} SET {c} = ?2 WHERE rowid = ?1");
    let read_back = format!(
        "SELECT {} FROM {t} WHERE rowid = ?1",
        crate::codec::unpack_sql(c)
    );
    while !stop.load(Ordering::Relaxed) {
        let _write = crate::storage::write_lock();
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let after = setting_i64(&tx, &key)?;
        if after >= until {
            break;
        }
        // A value that is not UTF-8 text reads as `None` and is left alone.
        let rows: Vec<(i64, Option<String>)> = tx
            .prepare_cached(&select)?
            .query_map(params![after, until, limit], |row| {
                Ok((
                    row.get(0)?,
                    row.get_ref(1)?.as_str().ok().map(str::to_owned),
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let last = rows.last().map(|r| r.0);
        let mut failed: u64 = 0;
        for (rowid, plain) in &rows {
            let Some(plain) = plain else {
                continue;
            };
            // Decompressed and compared before anything is written.
            let packed = match crate::codec::pack_checked(&dict, plain) {
                crate::codec::Packed::Value(v) => v,
                crate::codec::Packed::Plain => continue,
                crate::codec::Packed::Failed => {
                    failed += 1;
                    continue;
                }
            };
            tx.execute_batch("SAVEPOINT zstd_value")?;
            tx.prepare_cached(&update)?
                .execute(params![rowid, packed])?;
            // And read back through SQL's decoder after: a decode error is a
            // mismatch like any other.
            let same = tx
                .prepare_cached(&read_back)?
                .query_row([rowid], |row| {
                    Ok(row.get_ref(0)?.as_bytes_or_null()? == Some(plain.as_bytes()))
                })
                .unwrap_or(false);
            if same {
                tx.execute_batch("RELEASE zstd_value")?;
                report.converted += 1;
                report.bytes_before += plain.len() as u64;
                report.bytes_after += packed.len() as u64;
            } else {
                tx.execute_batch("ROLLBACK TO zstd_value; RELEASE zstd_value")?;
                failed += 1;
            }
        }
        put_setting_i64(&tx, &key, last.unwrap_or(until))?;
        tx.commit()?;
        report.failed += failed;
        if last.is_none() {
            break;
        }
    }
    Ok(())
}

/// Compress the archive's bodies: for each column of
/// [`crate::codec::COLUMNS`], train a dictionary from a sample of its rows if
/// it has none and enough rows ([`Training`]), then rewrite its rows up to
/// the dictionary's `until_rowid` compressed.
///
/// The same shape as [`slim_tool_outputs`]: batches of `batch` rows, each one
/// IMMEDIATE transaction under the write lock that advances the column's
/// cursor, so a crash, an error or a stop loses at most the batch in flight
/// and a rerun continues after the last committed one.
///
/// Verified twice: each value is compressed, decompressed and compared byte
/// for byte before it is written, then read back through SQL's decoder and
/// compared again before the batch commits. A value that fails either is left
/// exactly as it was and counted in [`ZstdReport::failed`]. A dictionary is
/// never replaced. `stop` is checked between batches.
///
/// `before_writing` runs once dictionaries exist and before any row is
/// rewritten; [`compact`] backs the archive up there.
pub fn compress_bodies(
    conn: &Connection,
    training: &Training,
    batch: usize,
    stop: &AtomicBool,
    before_writing: impl FnOnce(&Connection) -> Result<()>,
) -> Result<ZstdReport> {
    let mut report = ZstdReport::default();
    for column in crate::codec::COLUMNS {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        report.dicts_trained += u64::from(train_column(conn, &column, training)?);
    }
    if zstd_pending(conn)? && !stop.load(Ordering::Relaxed) {
        before_writing(conn)?;
        for column in crate::codec::COLUMNS {
            convert_column(conn, &column, batch, stop, &mut report)?;
        }
    }
    report.pending = zstd_pending(conn)?;
    Ok(report)
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
    /// Attachment rows are still inline, or tool results still unexamined.
    /// The `VACUUM` waits for the conversions, so the file is rewritten once,
    /// at its smallest.
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
/// later launch tries again. `tolerated_inline` is how many attachment rows
/// may still be inline: the rows the conversion could not verify and left as
/// they were ([`DedupeReport::failed`]), which would otherwise block it
/// forever.
///
/// The outcome is recorded under [`VACUUM_SETTING`] so a skip is visible
/// afterwards. The `VACUUM` holds the write lock for its duration (seconds on
/// a multi-gigabyte archive), once.
pub fn vacuum_once(conn: &Connection, tolerated_inline: u64) -> Result<VacuumOutcome> {
    let space = match conn.path().filter(|p| !p.is_empty()) {
        Some(path) => vacuum_space(std::path::Path::new(path))?,
        None => VacuumSpace::one_volume(0),
    };
    vacuum_once_with(conn, space, tolerated_inline)
}

/// [`vacuum_once`] with the free space given, so the space rule can be tested
/// without filling a disk.
pub fn vacuum_once_with(
    conn: &Connection,
    space: VacuumSpace,
    tolerated_inline: u64,
) -> Result<VacuumOutcome> {
    let outcome = decide_vacuum(conn, space, tolerated_inline)?;
    let outcome = match outcome {
        Some(outcome) => outcome,
        None => full_vacuum(conn)?,
    };
    if outcome != VacuumOutcome::NoFile {
        // `settings::set` takes the write lock itself.
        crate::settings::set(conn, VACUUM_SETTING, &format!("\"{}\"", outcome.as_str()))?;
    }
    Ok(outcome)
}

/// Rewrite the archive with one `VACUUM`, in incremental mode.
fn full_vacuum(conn: &Connection) -> Result<VacuumOutcome> {
    let pages_before = page_count(conn)?;
    {
        let _write = crate::storage::write_lock();
        crate::storage::checkpoint(conn);
        // The rewritten copy goes to a temporary file, not memory: with the
        // connection's usual `temp_store = MEMORY` it would hold the whole
        // archive in RAM. Restored whatever happens.
        conn.execute_batch("PRAGMA temp_store = FILE;")?;
        let vacuumed = conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;");
        conn.execute_batch("PRAGMA temp_store = MEMORY;")?;
        vacuumed?;
    }
    crate::storage::checkpoint(conn);
    Ok(VacuumOutcome::Vacuumed {
        pages_before,
        pages_after: page_count(conn)?,
    })
}

/// The setting that records the outcome of the one `VACUUM` after
/// compression ([`vacuum_after_compression`]), content-free.
pub const ZSTD_VACUUM_SETTING: &str = "compact.zstd_vacuum";

/// Rewrite the archive once after [`compress_bodies`] has converted every
/// column with a dictionary. Compressing shrinks rows in place and leaves
/// pages part-filled, which returning free pages cannot recover: on a 2.66 GB
/// archive about 220 MB. The same space rule as [`vacuum_once`] applies; a
/// skip is recorded under [`ZSTD_VACUUM_SETTING`] and retried by a later
/// run. `None` when there is nothing to do: no dictionary yet, rows still to
/// convert, or already done.
pub fn vacuum_after_compression(conn: &Connection) -> Result<Option<VacuumOutcome>> {
    let done =
        crate::settings::get(conn, ZSTD_VACUUM_SETTING)?.is_some_and(|v| v == "\"vacuumed\"");
    let has_dict: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM zstd_dict)", [], |r| r.get(0))?;
    if done || !has_dict || zstd_pending(conn)? {
        return Ok(None);
    }
    let Some(path) = conn.path().filter(|p| !p.is_empty()) else {
        return Ok(Some(VacuumOutcome::NoFile));
    };
    let space = vacuum_space(std::path::Path::new(path))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let size = u64::try_from(page_count(conn)?.saturating_mul(page_size)).unwrap_or(u64::MAX);
    let outcome = match space.shortfall(size) {
        Some((needed, available)) => VacuumOutcome::LowSpace { needed, available },
        None => full_vacuum(conn)?,
    };
    crate::settings::set(
        conn,
        ZSTD_VACUUM_SETTING,
        &format!("\"{}\"", outcome.as_str()),
    )?;
    Ok(Some(outcome))
}

/// `Some(outcome)` when the `VACUUM` should not run, `None` when it should.
fn decide_vacuum(
    conn: &Connection,
    space: VacuumSpace,
    tolerated_inline: u64,
) -> Result<Option<VacuumOutcome>> {
    if is_incremental(conn)? {
        return Ok(Some(VacuumOutcome::AlreadyIncremental));
    }
    if conn.path().filter(|p| !p.is_empty()).is_none() {
        return Ok(Some(VacuumOutcome::NoFile));
    }
    if inline_attachments(conn)? > tolerated_inline
        || tool_outputs_pending(conn)?
        || zstd_pending(conn)?
    {
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
pub(crate) fn free_bytes(dir: &std::path::Path) -> Result<(u64, rustix::fs::Dev)> {
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
    pub slim: SlimReport,
    pub zstd: ZstdReport,
    pub vacuum: VacuumOutcome,
    /// The one `VACUUM` after compression, when it was due this run.
    pub zstd_vacuum: Option<VacuumOutcome>,
    /// Free pages returned to the file system after the conversions, on an
    /// archive already in incremental mode.
    pub reclaimed_pages: i64,
}

/// Convert inline attachments and tool outputs, then switch the file to
/// incremental mode once, or return the freed pages when it already is.
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
    crate::settings::set(conn, DEDUPE_FAILED_SETTING, &dedupe.failed.to_string())
        .map_err(|e| ("dedupe", e))?;
    crate::settings::set(conn, FAILED_ROWS_SETTING, &dedupe.failed.to_string())
        .map_err(|e| ("dedupe", e))?;
    let slim = slim_tool_outputs(conn, DEDUPE_BATCH, stop).map_err(|e| ("slim", e))?;
    // Compression reads the stored form the conversions above produce, so it
    // waits for them.
    let ready = !stop.load(Ordering::Relaxed) && !slim.pending && dedupe.remaining <= dedupe.failed;
    let zstd = if ready {
        let zstd = compress_bodies(
            conn,
            &Training::default(),
            ZSTD_BATCH,
            stop,
            back_up_before_converting,
        )
        .map_err(|e| ("zstd", e))?;
        crate::settings::set(
            conn,
            FAILED_ROWS_SETTING,
            &(dedupe.failed + zstd.failed).to_string(),
        )
        .map_err(|e| ("zstd", e))?;
        zstd
    } else {
        ZstdReport {
            pending: true,
            ..ZstdReport::default()
        }
    };
    let vacuum = if stop.load(Ordering::Relaxed) {
        VacuumOutcome::DedupePending
    } else {
        vacuum_once(conn, dedupe.failed).map_err(|e| ("vacuum", e))?
    };
    // A first-time VACUUM after a finished compression already did the work.
    if matches!(vacuum, VacuumOutcome::Vacuumed { .. }) && !zstd.pending && zstd.converted > 0 {
        crate::settings::set(conn, ZSTD_VACUUM_SETTING, "\"vacuumed\"")
            .map_err(|e| ("vacuum", e))?;
    }
    let zstd_vacuum =
        if vacuum == VacuumOutcome::AlreadyIncremental && !stop.load(Ordering::Relaxed) {
            vacuum_after_compression(conn).map_err(|e| ("vacuum", e))?
        } else {
            None
        };
    let mut reclaimed_pages = 0;
    if vacuum == VacuumOutcome::AlreadyIncremental {
        let reclaim = || -> Result<i64> {
            let before = freelist_count(conn)?;
            let _write = crate::storage::write_lock();
            reclaim_free_pages(conn)?;
            Ok(before - freelist_count(conn)?)
        };
        reclaimed_pages = reclaim().map_err(|e| ("reclaim", e))?;
    }
    Ok(CompactReport {
        dedupe,
        slim,
        zstd,
        vacuum,
        zstd_vacuum,
        reclaimed_pages,
    })
}

/// Back up a file archive that still has rows to convert. Rows the last run
/// could not verify do not count: they are left as they were, and backing up
/// on every launch for them would only fill the disk.
fn back_up_before_converting(conn: &Connection) -> Result<()> {
    let known_failed = crate::settings::get(conn, DEDUPE_FAILED_SETTING)?
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if inline_attachments(conn)? <= known_failed
        && !tool_outputs_pending(conn)?
        && !zstd_pending(conn)?
    {
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
    fn the_rule_slims_one_occurrence_and_keeps_everything_else() {
        let q = TEXT_QUOTED_MARKER;
        let r = TEXT_RAW_MARKER;
        let out = "a tool output long enough to slim";
        assert_eq!(
            slim_tool_output(out, &format!("\"{out}\"")),
            Some(q.to_owned())
        );
        assert_eq!(
            slim_tool_output(out, &format!("[{{\"type\":\"text\",\"text\":\"{out}\"}}]")),
            Some(format!("[{{\"type\":\"text\",\"text\":{q}}}]"))
        );
        let raw = "[{\"text\":\"Script completed\"}]";
        assert_eq!(
            slim_tool_output(raw, &format!("{{\"output\":{raw}}}")),
            Some(format!("{{\"output\":{r}}}"))
        );
        // Twice, absent, too short, or a payload already holding a 0x01 byte.
        assert_eq!(
            slim_tool_output(out, &format!("[\"{out}\",\"{out}\"]")),
            None
        );
        assert_eq!(slim_tool_output(out, "\"something else entirely\""), None);
        assert_eq!(slim_tool_output("ok", "\"ok\""), None);
        // Verbatim only for an output that is a JSON array or object.
        assert_eq!(slim_tool_output(out, &format!("\"{out}x\"")), None);
        assert_eq!(
            slim_tool_output("[1, 2, 3 and more", "{\"o\":[1, 2, 3 and more}"),
            None
        );
        assert_eq!(slim_tool_output(out, &format!("\"{out}\"\u{1}")), None);
    }

    /// The view rebuilds a quoted output with SQLite's `json_quote`, and the
    /// rule finds it with serde_json's quoting: they agree on every character,
    /// so rows slim instead of failing their read-back.
    #[test]
    fn sqlite_and_serde_quote_every_character_alike() {
        let conn = Connection::open_in_memory().unwrap();
        let all: String = (0..=0x10_FFFF_u32).filter_map(char::from_u32).collect();
        for chunk in all.chars().collect::<Vec<_>>().chunks(4096) {
            let s: String = chunk.iter().collect();
            let sqlite: String = conn
                .query_row("SELECT json_quote(?1)", [&s], |r| r.get(0))
                .unwrap();
            assert_eq!(sqlite, serde_json::to_string(&s).unwrap());
        }
    }

    #[test]
    fn the_temp_dir_is_a_writable_directory() {
        assert!(sqlite_temp_dir().is_dir());
    }
}
