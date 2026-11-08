//! Lore-owned local backups via SQLite's online backup API
//! (`docs/architecture/SECURITY.md` §6, `DATA_MODEL.md` §9).
//!
//! A backup is a standalone snapshot of the live WAL-mode archive copied page
//! by page while the source connection stays usable, so a concurrent worker
//! connection can keep ingesting during the copy (the copy retries on SQLite's
//! transient BUSY/LOCKED results — "Example 2: Online Backup of a Running
//! Database" from the SQLite docs). Backups land under the archive's `backups/`
//! directory, inherit the app's private-file posture (`0600` on unix), are
//! verified with `PRAGMA integrity_check` before being kept, and retention
//! bounds the number of copies on disk. Original agent logs and user-chosen
//! exports are never part of a backup — only Lore-owned archive data.
//!
//! A backup is stored zstd-compressed (`lore-<ts>-<n>.db.zst`, level 3, with
//! a content checksum): the online copy lands in a hidden temporary file, is
//! integrity-checked, then stream-compressed to a second temporary file that
//! is fsynced and renamed into place, and the uncompressed copy is deleted.
//! Backups taken by earlier builds are plain `.db` files; listing, retention
//! and restore accept both, so those age out under the same retention.
//!
//! Naming is intentionally lexicographically sortable (zero-padded timestamp +
//! per-process counter) so retention can prune "the newest N" without reading
//! file metadata, and names carry no source content.
//!
//! Recovery (SECURITY.md §6): a Lore-owned backup can be restored wholesale
//! into a destination database file via SQLite's restore API (the reverse of
//! the online copy), then re-verified. Restore never needs the original agent
//! logs — a backup contains the entire archive. The caller closes connections
//! to the destination first (the recovery flow "closes the active DB"), because
//! restore replaces the destination's content in place. A compressed backup
//! is first decompressed to a temporary file beside the destination and
//! integrity-checked; a damaged or truncated one is reported as
//! [`BackupError::Broken`].

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{backup::Backup, Connection, DatabaseName};

/// Name prefix for Lore-owned backup files.
const BACKUP_PREFIX: &str = "lore-";
/// Extension of uncompressed Lore-owned backup files (earlier builds).
const BACKUP_EXT: &str = "db";
/// Extension of compressed Lore-owned backup files.
const COMPRESSED_EXT: &str = "db.zst";
/// Suffix of the hidden temporary files a backup or restore writes. Their
/// names start with `.`, so they never look like a backup.
const PARTIAL_EXT: &str = "partial";
/// Temporary files older than this are left over from an interrupted backup
/// and are deleted by the next one. Long enough that a concurrent backup by
/// another process is never cut short.
const STALE_PARTIAL_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// zstd level for backups: 2.2x on a real 2 GB archive in about 6 s on one
/// core, where level 19 gains another 16% for 25 times the time.
const ZSTD_LEVEL: i32 = 3;
/// Upper bound on zstd compression threads, so a backup taken while the app
/// is in use does not take every core.
const MAX_ZSTD_WORKERS: usize = 4;
/// Upper bound on pages copied per backup step and the pause between steps.
/// 4 MiB per step at the default page size: the pause still lets a concurrent
/// writer in between steps, and a 4 GB archive spends about 10 s paused
/// rather than the 100 s 100-page steps took. That matters now that the
/// archive is backed up while the app opens it (`storage::migrations`).
const PAGES_PER_STEP: i32 = 1024;
const PAUSE_BETWEEN_STEPS: Duration = Duration::from_millis(10);

/// Default number of Lore-owned backups to keep on disk. Bounded and
/// user-visible (SECURITY.md §6); the exact cadence/retention remains a
/// settings decision (DATA_MODEL.md §11) — this is only the mechanism's bound.
pub const DEFAULT_BACKUP_RETENTION: usize = 7;

/// Per-process counter so backups created within the same millisecond never
/// collide; names remain lexicographically sortable (== chronological).
static BACKUP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Content-free outcome of creating a backup. The path is a Lore-owned file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Errors from creating/pruning a backup. Content-free: never echoes archive
/// data or SQLite diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("sqlite error during backup")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error while creating or pruning backup")]
    Io,
    /// Too little free space beside the backups for the temporary
    /// uncompressed copy plus the compressed backup.
    #[error("not enough free space for a backup: {needed} bytes needed, {available} available")]
    LowSpace { needed: u64, available: u64 },
    /// The backup file is damaged: it does not decompress, or what it
    /// decompresses to fails `PRAGMA integrity_check`.
    #[error("backup file is damaged")]
    Broken,
    #[error(transparent)]
    Settings(#[from] crate::storage::StorageError),
}

impl From<std::io::Error> for BackupError {
    fn from(_: std::io::Error) -> Self {
        BackupError::Io
    }
}

/// Convenience result alias for the backup layer.
pub type Result<T> = std::result::Result<T, BackupError>;

/// Snapshot the live archive `conn` into a new Lore-owned backup under
/// `backup_dir`, keeping only the newest `keep` backups, and return the
/// content-free outcome.
///
/// The source connection stays open and usable during the copy; the backup file
/// is verified (openable + `PRAGMA integrity_check`) and made private before
/// the function returns. Creating the backup never takes a write lock that
/// blocks the worker or UI connections beyond SQLite's own short read-lock
/// window.
///
/// The backup is refused with [`BackupError::LowSpace`] unless the volume
/// holding `backup_dir` has room for the temporary uncompressed copy and the
/// compressed backup together ([`backup_space_needed`]).
pub fn create_backup(conn: &Connection, backup_dir: &Path, keep: usize) -> Result<BackupInfo> {
    create_backup_with(conn, backup_dir, keep, &|dir| {
        crate::compact::free_bytes(dir)
            .map(|(bytes, _)| bytes)
            .map_err(|_| BackupError::Io)
    })
}

/// [`create_backup`] with the free-space probe supplied, so the refusal is
/// testable.
fn create_backup_with(
    conn: &Connection,
    backup_dir: &Path,
    keep: usize,
    free_bytes: &dyn Fn(&Path) -> Result<u64>,
) -> Result<BackupInfo> {
    std::fs::create_dir_all(backup_dir)?;
    remove_stale_partials(backup_dir);

    let needed = backup_space_needed(conn)?;
    let available = free_bytes(backup_dir)?;
    if available < needed {
        return Err(BackupError::LowSpace { needed, available });
    }

    let path = backup_path(backup_dir);
    let raw = partial_path(&path, "raw");
    let packed = partial_path(&path, "zst");
    let res = (|| -> Result<()> {
        copy_online(conn, &raw)?;
        set_private(&raw)?;
        verify(&raw)?;
        compress(&raw, &packed)?;
        std::fs::rename(&packed, &path)?;
        sync_dir(backup_dir);
        Ok(())
    })();
    let _ = std::fs::remove_file(&raw);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&packed);
        return Err(e);
    }
    prune(backup_dir, keep)?;

    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(BackupInfo { path, size_bytes })
}

/// Bytes a backup of `conn` needs free beside the backups at its peak: the
/// whole uncompressed copy, plus the compressed backup written while that
/// copy still exists, allowed half its size (backups compress 2.2x on a real
/// archive). A backup that still runs out fails cleanly and leaves nothing.
pub fn backup_space_needed(conn: &Connection) -> Result<u64> {
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let size = u64::try_from(page_size.saturating_mul(page_count)).unwrap_or(u64::MAX);
    Ok(size.saturating_add(size / 2))
}

/// Copy the live archive `conn` into a new database file at `dst_path`.
fn copy_online(conn: &Connection, dst_path: &Path) -> Result<()> {
    let mut dst = Connection::open(dst_path).map_err(|_| BackupError::Io)?;
    let backup = Backup::new(conn, &mut dst)?;
    // Online copy with retry on BUSY/LOCKED so a concurrent writer (e.g. the
    // background worker) never aborts the backup.
    backup.run_to_completion(PAGES_PER_STEP, PAUSE_BETWEEN_STEPS, None)?;
    Ok(())
}

/// Stream-compress `src` into a new private file at `dst`, fsynced.
fn compress(src: &Path, dst: &Path) -> Result<()> {
    let input = File::open(src)?;
    let len = input.metadata()?.len();
    let out = File::options().write(true).create_new(true).open(dst)?;
    set_private(dst)?;
    let mut encoder = zstd::stream::write::Encoder::new(out, ZSTD_LEVEL)?;
    encoder.include_checksum(true)?;
    encoder.set_pledged_src_size(Some(len))?;
    encoder.multithread(zstd_workers())?;
    std::io::copy(&mut BufReader::with_capacity(1 << 20, input), &mut encoder)?;
    encoder.finish()?.sync_all()?;
    Ok(())
}

/// Stream-decompress the backup `src` into a new private file at `dst`,
/// fsynced. A stream that does not decode — corrupt, truncated, or failing
/// its checksum — is [`BackupError::Broken`]; a failed write is `Io`.
fn decompress(src: &Path, dst: &Path) -> Result<()> {
    let input = File::open(src)?;
    let out = File::options().write(true).create_new(true).open(dst)?;
    set_private(dst)?;
    let mut decoder = zstd::stream::read::Decoder::new(input).map_err(|_| BackupError::Broken)?;
    let mut writer = BufWriter::with_capacity(1 << 20, out);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = decoder.read(&mut buf).map_err(|_| BackupError::Broken)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
    }
    writer
        .into_inner()
        .map_err(|_| BackupError::Io)?
        .sync_all()?;
    Ok(())
}

fn zstd_workers() -> u32 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    u32::try_from(cores.min(MAX_ZSTD_WORKERS)).unwrap_or(1)
}

/// The hidden temporary file `tag` for the backup at `path`, in its directory.
fn partial_path(path: &Path, tag: &str) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("lore");
    path.with_file_name(format!(".{name}.{tag}.{PARTIAL_EXT}"))
}

/// Delete temporary files an interrupted backup left in `backup_dir`.
/// Best-effort: a file that cannot be read or removed is left alone.
fn remove_stale_partials(backup_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(backup_dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let ours = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.starts_with(&format!(".{BACKUP_PREFIX}")) && n.ends_with(&format!(".{PARTIAL_EXT}"))
        });
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= STALE_PARTIAL_AGE);
        if ours && stale && path.is_file() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Make a rename in `dir` durable. Best-effort: the backup is already
/// complete and fsynced.
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

// ── Automatic-backup schedule ──────────────────────────────────────────────
// The cadence and retention are user-configurable and persisted in the settings
// store (Lore-owned, cleared by "forget everything"). Defaults are conservative:
// automatic backups are Off until the user opts into an interval.

const KEY_INTERVAL: &str = "backup.interval";
const KEY_KEEP: &str = "backup.keep";
const KEY_LAST_AT: &str = "backup.last_at";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How often automatic Lore-owned backups run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackupInterval {
    /// No automatic backups (the default); the user can still back up on demand.
    #[default]
    Off,
    Daily,
    Weekly,
}

impl BackupInterval {
    /// The stable wire value persisted in settings and crossed over IPC.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BackupInterval::Off => "off",
            BackupInterval::Daily => "daily",
            BackupInterval::Weekly => "weekly",
        }
    }

    /// Parse the wire value; anything unrecognized is `Off`.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "daily" => BackupInterval::Daily,
            "weekly" => BackupInterval::Weekly,
            _ => BackupInterval::Off,
        }
    }

    /// The period between automatic backups, or `None` when off.
    fn period_ms(self) -> Option<i64> {
        match self {
            BackupInterval::Off => None,
            BackupInterval::Daily => Some(DAY_MS),
            BackupInterval::Weekly => Some(7 * DAY_MS),
        }
    }
}

impl std::fmt::Display for BackupInterval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The user-configurable automatic-backup schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupSchedule {
    pub interval: BackupInterval,
    /// Number of newest backups to retain (clamped to a sane bound).
    pub keep: usize,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            interval: BackupInterval::Off,
            keep: DEFAULT_BACKUP_RETENTION,
        }
    }
}

/// Read the automatic-backup schedule from settings, falling back to defaults
/// for any unset or malformed value.
pub fn read_schedule(conn: &Connection) -> Result<BackupSchedule> {
    let interval = crate::settings::get(conn, KEY_INTERVAL)?
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .map_or(BackupInterval::Off, |s| BackupInterval::parse(&s));
    let keep = crate::settings::get(conn, KEY_KEEP)?
        .and_then(|v| serde_json::from_str::<usize>(&v).ok())
        .unwrap_or(DEFAULT_BACKUP_RETENTION)
        .clamp(1, 100);
    Ok(BackupSchedule { interval, keep })
}

/// Persist the automatic-backup schedule to settings.
pub fn write_schedule(conn: &Connection, schedule: BackupSchedule) -> Result<()> {
    crate::settings::set(
        conn,
        KEY_INTERVAL,
        &format!("\"{}\"", schedule.interval.as_str()),
    )?;
    crate::settings::set(conn, KEY_KEEP, &schedule.keep.clamp(1, 100).to_string())?;
    Ok(())
}

/// Create an automatic backup if one is due per the stored schedule: stamp the
/// last-backup time and return the new backup. Returns `None` when backups are
/// off or the interval has not elapsed since the last one. `now_ms` is the
/// current epoch-millis clock, supplied by the caller so the decision is
/// deterministic and testable.
pub fn run_scheduled_backup(
    conn: &Connection,
    backup_dir: &Path,
    now_ms: i64,
) -> Result<Option<BackupInfo>> {
    let schedule = read_schedule(conn)?;
    let Some(period) = schedule.interval.period_ms() else {
        return Ok(None);
    };
    let last_at =
        crate::settings::get(conn, KEY_LAST_AT)?.and_then(|v| serde_json::from_str::<i64>(&v).ok());
    let due = last_at.is_none_or(|t| now_ms.saturating_sub(t) >= period);
    if !due {
        return Ok(None);
    }
    let info = create_backup(conn, backup_dir, schedule.keep)?;
    crate::settings::set(conn, KEY_LAST_AT, &now_ms.to_string())?;
    Ok(Some(info))
}

/// How recent a backup must be for [`ensure_recent_backup`] to rely on it
/// instead of taking another: one hour.
pub const RECENT_BACKUP_MS: i64 = 60 * 60 * 1000;

/// The Lore-owned backup directory beside the archive `conn` has open, or
/// `None` for an in-memory archive.
#[must_use]
pub fn backup_dir_for(conn: &Connection) -> Option<PathBuf> {
    let db = Path::new(conn.path().filter(|p| !p.is_empty())?);
    Some(db.parent()?.join(crate::paths::BACKUPS_DIRNAME))
}

/// Back up the archive before a change an older build cannot read back,
/// unless a Lore-owned backup taken less than `max_age_ms` before `now_ms`
/// already exists. Returns the new backup, or `None` when a recent one made it
/// unnecessary. Uses [`create_backup`] and the user's retention.
///
/// The caller makes no change when this fails: a one-way step without a way
/// back is exactly what this exists to prevent.
pub fn ensure_recent_backup(
    conn: &Connection,
    backup_dir: &Path,
    now_ms: i64,
    max_age_ms: i64,
) -> Result<Option<BackupInfo>> {
    let recent = list_backups(backup_dir)?
        .iter()
        .filter_map(|path| backup_taken_at_ms(path))
        .any(|taken| (0..max_age_ms).contains(&now_ms.saturating_sub(taken)));
    if recent {
        return Ok(None);
    }
    let keep = read_schedule(conn).map_or(DEFAULT_BACKUP_RETENTION, |s| s.keep);
    create_backup(conn, backup_dir, keep).map(Some)
}

/// When a Lore-owned backup was taken, in epoch millis, read from its name
/// ([`backup_path`]). `None` for a name that does not carry one.
fn backup_taken_at_ms(path: &Path) -> Option<i64> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix(BACKUP_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// A collision-free, lexicographically-chronological backup path.
fn backup_path(backup_dir: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = BACKUP_SEQ.fetch_add(1, Ordering::Relaxed);
    backup_dir.join(format!(
        "{BACKUP_PREFIX}{stamp:020}-{seq:04}.{COMPRESSED_EXT}"
    ))
}

/// Refuse to keep a backup that does not open cleanly or fails an integrity
/// check. Content-free: a failure is reported as `Io`, never the diagnostic.
fn verify(path: &Path) -> Result<()> {
    let conn = Connection::open(path).map_err(|_| BackupError::Io)?;
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| BackupError::Io)?;
    if result != "ok" {
        return Err(BackupError::Io);
    }
    Ok(())
}

/// Keep only the newest `keep` Lore-owned backups, compressed or not,
/// deleting older copies. Names are lexicographically sorted, which is
/// chronological by construction.
fn prune(backup_dir: &Path, keep: usize) -> Result<()> {
    let backups = list_backups(backup_dir)?;
    if backups.len() <= keep {
        return Ok(());
    }
    let drop_count = backups.len() - keep;
    for old in backups.into_iter().take(drop_count) {
        std::fs::remove_file(&old)?;
    }
    Ok(())
}

/// List the Lore-owned backups under `backup_dir`, oldest first (names are
/// lexicographically chronological), both compressed `.db.zst` and the plain
/// `.db` earlier builds wrote. Content-free: only Lore-owned paths are
/// returned, never archive data. Enables the recovery flow to offer a restore
/// from the newest local backup (SECURITY.md §6).
pub fn list_backups(backup_dir: &Path) -> Result<Vec<PathBuf>> {
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }
    let mut backups: Vec<PathBuf> = std::fs::read_dir(backup_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_backup_file(path))
        .collect();
    backups.sort();
    Ok(backups)
}

/// Restore a Lore-owned backup into the database at `dst_db_path`, replacing
/// the destination's content wholesale, and verify the result opens cleanly.
///
/// This is the reverse of the online copy: the backup file (a standalone
/// database) is copied page by page into the destination. The caller must
/// ensure no other connection to `dst_db_path` is open — the recovery flow
/// closes the active DB first (SECURITY.md §6). Original agent logs are never
/// needed: a backup contains the entire archive. Content-free on failure.
///
/// A compressed backup is decompressed to a hidden temporary file beside
/// `dst_db_path` and integrity-checked first; one that fails either is
/// [`BackupError::Broken`] and the destination is left untouched.
pub fn restore_backup(backup_path: &Path, dst_db_path: &Path) -> Result<()> {
    if !backup_path.is_file() {
        return Err(BackupError::Io);
    }
    if !is_compressed(backup_path) {
        return restore_db(backup_path, dst_db_path);
    }
    let tmp = partial_path(
        dst_db_path,
        &format!("restore-{}", BACKUP_SEQ.fetch_add(1, Ordering::Relaxed)),
    );
    let res = decompress(backup_path, &tmp)
        .and_then(|()| {
            // An empty or headerless file opens as an empty database and
            // passes `integrity_check`; a backup is never that.
            let mut header = [0u8; 16];
            File::open(&tmp)?
                .read_exact(&mut header)
                .map_err(|_| BackupError::Broken)?;
            if &header != b"SQLite format 3\0" {
                return Err(BackupError::Broken);
            }
            verify(&tmp).map_err(|_| BackupError::Broken)
        })
        .and_then(|()| restore_db(&tmp, dst_db_path));
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Restore the uncompressed database `src` into `dst_db_path`.
fn restore_db(src: &Path, dst_db_path: &Path) -> Result<()> {
    let restore_res = (|| -> Result<()> {
        let mut dst = Connection::open(dst_db_path).map_err(|_| BackupError::Io)?;
        dst.restore(
            DatabaseName::Main,
            src,
            None::<fn(rusqlite::backup::Progress)>,
        )?;
        drop(dst);
        verify(dst_db_path)
    })();

    if let Err(e) = restore_res {
        let _ = std::fs::remove_file(dst_db_path);
        return Err(e);
    }
    Ok(())
}

fn is_backup_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(BACKUP_PREFIX)
                    && (name.ends_with(&format!(".{BACKUP_EXT}"))
                        || name.ends_with(&format!(".{COMPRESSED_EXT}")))
            })
}

fn is_compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!(".{COMPRESSED_EXT}")))
}

/// Backup files inherit the app's private-file posture (SECURITY.md §2).
#[cfg(unix)]
fn set_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_recent_backup_skips_only_when_a_backup_is_under_an_hour_old() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::storage::open(&dir.path().join("lore.db")).unwrap();
        let backup_dir = backup_dir_for(&conn).unwrap();
        assert_eq!(backup_dir, dir.path().join(crate::paths::BACKUPS_DIRNAME));

        let now = crate::now_ms();
        let first = ensure_recent_backup(&conn, &backup_dir, now, RECENT_BACKUP_MS).unwrap();
        assert!(first.is_some(), "no backup yet, so one is taken");
        let taken = backup_taken_at_ms(&first.unwrap().path).unwrap();
        assert!((now - taken).abs() < 60_000, "the name carries the time");

        let soon = taken + RECENT_BACKUP_MS - 1;
        assert_eq!(
            ensure_recent_backup(&conn, &backup_dir, soon, RECENT_BACKUP_MS).unwrap(),
            None,
            "under an hour old: relied on"
        );
        let later = taken + RECENT_BACKUP_MS;
        assert!(
            ensure_recent_backup(&conn, &backup_dir, later, RECENT_BACKUP_MS)
                .unwrap()
                .is_some(),
            "an hour old: another is taken"
        );
        assert_eq!(list_backups(&backup_dir).unwrap().len(), 2);
        assert_eq!(
            backup_dir_for(&crate::storage::open_in_memory().unwrap()),
            None
        );
    }

    #[test]
    fn a_backup_is_refused_without_room_for_the_uncompressed_copy() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::storage::open(&dir.path().join("lore.db")).unwrap();
        let backup_dir = dir.path().join("backups");
        let size = std::fs::metadata(dir.path().join("lore.db")).unwrap().len();
        let needed = backup_space_needed(&conn).unwrap();
        assert!(needed >= size, "the temporary copy is the whole archive");

        let seen = std::cell::Cell::new(None);
        let short = |d: &Path| {
            seen.set(Some(d.to_path_buf()));
            Ok(needed - 1)
        };
        match create_backup_with(&conn, &backup_dir, 7, &short) {
            Err(BackupError::LowSpace {
                needed: n,
                available,
            }) => {
                assert_eq!((n, available), (needed, needed - 1));
            }
            other => panic!("expected LowSpace, got {other:?}"),
        }
        assert_eq!(seen.take().as_deref(), Some(backup_dir.as_path()));
        assert_eq!(std::fs::read_dir(&backup_dir).unwrap().count(), 0);

        create_backup_with(&conn, &backup_dir, 7, &|_| Ok(needed)).unwrap();
        assert_eq!(list_backups(&backup_dir).unwrap().len(), 1);
    }

    #[test]
    fn stale_temporary_files_are_removed_and_fresh_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".lore-1-0000.db.zst.raw.partial");
        let fresh = dir.path().join(".lore-2-0000.db.zst.raw.partial");
        let other = dir.path().join(".other.partial");
        for p in [&stale, &fresh, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        let old = SystemTime::now() - STALE_PARTIAL_AGE - Duration::from_secs(60);
        for p in [&stale, &other] {
            File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        remove_stale_partials(dir.path());
        assert!(!stale.exists());
        assert!(fresh.exists(), "maybe another process's backup in progress");
        assert!(other.exists(), "not Lore's");
    }

    #[test]
    fn backup_interval_display_and_parse_roundtrip() {
        for (interval, expected) in [
            (BackupInterval::Off, "off"),
            (BackupInterval::Daily, "daily"),
            (BackupInterval::Weekly, "weekly"),
        ] {
            assert_eq!(interval.to_string(), expected);
            assert_eq!(interval.as_str(), expected);
            assert_eq!(BackupInterval::parse(expected), interval);
        }
        assert_eq!(BackupInterval::parse("unknown"), BackupInterval::Off);
    }

    #[test]
    fn prune_keeps_exact_number_of_newest_backups_and_ignores_non_backup_files() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Create mock backup files with chronological names
        let b1 = backup_dir.join("lore-00000000000000000001-0000.db");
        let b2 = backup_dir.join("lore-00000000000000000002-0000.db");
        let b3 = backup_dir.join("lore-00000000000000000003-0000.db");
        let extra = backup_dir.join("other_file.txt");
        let foreign_db = backup_dir.join("lore-not-backup.wal");

        std::fs::write(&b1, b"b1").unwrap();
        std::fs::write(&b2, b"b2").unwrap();
        std::fs::write(&b3, b"b3").unwrap();
        std::fs::write(&extra, b"txt").unwrap();
        std::fs::write(&foreign_db, b"wal").unwrap();

        let list = list_backups(&backup_dir).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list, vec![b1.clone(), b2.clone(), b3.clone()]);

        // Prune down to 2 newest
        prune(&backup_dir, 2).unwrap();

        let list_after = list_backups(&backup_dir).unwrap();
        assert_eq!(list_after.len(), 2);
        assert_eq!(list_after, vec![b2, b3]);
        assert!(!b1.exists());
        assert!(extra.exists());
        assert!(foreign_db.exists());
    }

    #[test]
    fn backup_schedule_default_and_interval_display_and_period() {
        let default_sched = BackupSchedule::default();
        assert_eq!(default_sched.interval, BackupInterval::Off);
        assert_eq!(default_sched.keep, DEFAULT_BACKUP_RETENTION);

        assert_eq!(format!("{}", BackupInterval::Off), "off");
        assert_eq!(format!("{}", BackupInterval::Daily), "daily");
        assert_eq!(format!("{}", BackupInterval::Weekly), "weekly");

        assert_eq!(BackupInterval::Off.period_ms(), None);
        assert_eq!(BackupInterval::Daily.period_ms(), Some(DAY_MS));
        assert_eq!(BackupInterval::Weekly.period_ms(), Some(7 * DAY_MS));
    }

    #[test]
    fn prune_edge_cases_keep_zero_and_keep_greater_than_total() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let b1 = backup_dir.join("lore-00000000000000000001-0000.db");
        let b2 = backup_dir.join("lore-00000000000000000002-0000.db");
        let extra = backup_dir.join("other_file.txt");
        std::fs::write(&b1, b"b1").unwrap();
        std::fs::write(&b2, b"b2").unwrap();
        std::fs::write(&extra, b"extra").unwrap();

        // keep > total count (keep = 10, total = 2) -> retains all backups, deletes nothing
        prune(&backup_dir, 10).unwrap();
        assert_eq!(list_backups(&backup_dir).unwrap().len(), 2);
        assert!(b1.exists());
        assert!(b2.exists());
        assert!(extra.exists());

        // keep = 0 -> drops all backups, ignores non-backup file
        prune(&backup_dir, 0).unwrap();
        assert_eq!(list_backups(&backup_dir).unwrap().len(), 0);
        assert!(!b1.exists());
        assert!(!b2.exists());
        assert!(extra.exists());

        // keep = 0 on empty backup dir -> Ok(())
        prune(&backup_dir, 0).unwrap();

        // prune on nonexistent directory -> Ok(())
        let nonexistent = dir.path().join("nonexistent");
        prune(&nonexistent, 5).unwrap();
    }

    #[test]
    fn run_scheduled_backup_respects_interval_timing_and_due_dates() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::storage::open(&dir.path().join("lore.db")).unwrap();
        let backup_dir = dir.path().join("backups");

        // When interval is Off -> returns None
        write_schedule(
            &conn,
            BackupSchedule {
                interval: BackupInterval::Off,
                keep: 5,
            },
        )
        .unwrap();
        let res = run_scheduled_backup(&conn, &backup_dir, 1_000_000).unwrap();
        assert_eq!(res, None);

        // When interval is Daily and never run before -> runs backup
        write_schedule(
            &conn,
            BackupSchedule {
                interval: BackupInterval::Daily,
                keep: 5,
            },
        )
        .unwrap();
        let now_ms = 100_000_000;
        let res1 = run_scheduled_backup(&conn, &backup_dir, now_ms).unwrap();
        assert!(res1.is_some());

        // Calling again within 1 hour -> not due -> returns None
        let not_due_ms = now_ms + (60 * 60 * 1000);
        let res2 = run_scheduled_backup(&conn, &backup_dir, not_due_ms).unwrap();
        assert_eq!(res2, None);

        // Calling after 25 hours -> due -> runs backup
        let due_ms = now_ms + (25 * 60 * 60 * 1000);
        let res3 = run_scheduled_backup(&conn, &backup_dir, due_ms).unwrap();
        assert!(res3.is_some());
    }

    #[test]
    fn backup_interval_variants_and_error_formatting() {
        assert_eq!(BackupInterval::Off.as_str(), "off");
        assert_eq!(BackupInterval::Daily.as_str(), "daily");
        assert_eq!(BackupInterval::Weekly.as_str(), "weekly");

        assert_eq!(BackupInterval::parse("off"), BackupInterval::Off);
        assert_eq!(BackupInterval::parse("daily"), BackupInterval::Daily);
        assert_eq!(BackupInterval::parse("weekly"), BackupInterval::Weekly);
        assert_eq!(BackupInterval::parse("unknown"), BackupInterval::Off);

        let err_io = BackupError::Io;
        assert_eq!(
            err_io.to_string(),
            "io error while creating or pruning backup"
        );
    }

    #[test]
    fn backup_info_clones_and_list_nonexistent() {
        let info = BackupInfo {
            path: PathBuf::from("/path/to/backup.db"),
            size_bytes: 1024,
        };
        let info_clone = info.clone();
        assert_eq!(info, info_clone);

        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist");
        let list = list_backups(&nonexistent).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn backup_schedule_clones_and_read_defaults() {
        let sched = BackupSchedule {
            interval: BackupInterval::Weekly,
            keep: 14,
        };
        let sched2 = sched;
        assert_eq!(sched.interval, sched2.interval);
        assert_eq!(sched.keep, sched2.keep);

        let conn = crate::storage::open_in_memory().unwrap();
        let read = read_schedule(&conn).unwrap();
        assert_eq!(read.interval, BackupInterval::Off);
        assert_eq!(read.keep, DEFAULT_BACKUP_RETENTION);
    }

    #[test]
    fn backup_prune_keep_larger_than_count() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // 2 mock backup files
        let b1 = backup_dir.join("lore-00000000000000000001-0001.db");
        let b2 = backup_dir.join("lore-00000000000000000002-0002.db");
        std::fs::write(&b1, b"mock backup 1").unwrap();
        std::fs::write(&b2, b"mock backup 2").unwrap();

        // Pruning with keep = 10 -> keep all
        assert!(prune(&backup_dir, 10).is_ok());
        assert!(b1.exists());
        assert!(b2.exists());
    }
}
