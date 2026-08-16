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
//! Naming is intentionally lexicographically sortable (zero-padded timestamp +
//! per-process counter) so retention can prune "the newest N" without reading
//! file metadata, and names carry no source content.
//!
//! Recovery (SECURITY.md §6): a Lore-owned backup can be restored wholesale
//! into a destination database file via SQLite's restore API (the reverse of
//! the online copy), then re-verified. Restore never needs the original agent
//! logs — a backup contains the entire archive. The caller closes connections
//! to the destination first (the recovery flow "closes the active DB"), because
//! restore replaces the destination's content in place.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{backup::Backup, Connection, DatabaseName};

/// Name prefix for Lore-owned backup files.
const BACKUP_PREFIX: &str = "lore-";
/// Extension of Lore-owned backup files.
const BACKUP_EXT: &str = "db";
/// Upper bound on pages copied per backup step and the pause between steps.
const PAGES_PER_STEP: i32 = 100;
const PAUSE_BETWEEN_STEPS: Duration = Duration::from_millis(10);

/// Default number of Lore-owned backups to keep on disk. Bounded and
/// user-visible (SECURITY.md §6); the exact cadence/retention remains a
/// settings decision (DATA_MODEL.md §11) — this is only the mechanism's bound.
pub const DEFAULT_BACKUP_RETENTION: usize = 5;

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
    #[error(transparent)]
    Settings(#[from] crate::storage::StorageError),
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
pub fn create_backup(conn: &Connection, backup_dir: &Path, keep: usize) -> Result<BackupInfo> {
    std::fs::create_dir_all(backup_dir).map_err(|_| BackupError::Io)?;
    let path = backup_path(backup_dir);

    let mut dst = Connection::open(&path).map_err(|_| BackupError::Io)?;
    let backup = Backup::new(conn, &mut dst)?;
    // Online copy with retry on BUSY/LOCKED so a concurrent writer (e.g. the
    // background worker) never aborts the backup.
    backup.run_to_completion(PAGES_PER_STEP, PAUSE_BETWEEN_STEPS, None)?;
    drop(backup);
    drop(dst);

    if let Err(e) = set_private(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    if let Err(e) = verify(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    prune(backup_dir, keep)?;

    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(BackupInfo { path, size_bytes })
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

/// A collision-free, lexicographically-chronological backup path.
fn backup_path(backup_dir: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = BACKUP_SEQ.fetch_add(1, Ordering::Relaxed);
    backup_dir.join(format!("{BACKUP_PREFIX}{stamp:020}-{seq:04}.{BACKUP_EXT}"))
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

/// Keep only the newest `keep` Lore-owned backups, deleting older copies.
/// Names are lexicographically sorted, which is chronological by construction.
fn prune(backup_dir: &Path, keep: usize) -> Result<()> {
    let backups = list_backups(backup_dir)?;
    if backups.len() <= keep {
        return Ok(());
    }
    let drop_count = backups.len() - keep;
    for old in backups.into_iter().take(drop_count) {
        std::fs::remove_file(&old).map_err(|_| BackupError::Io)?;
    }
    Ok(())
}

/// List the Lore-owned backups under `backup_dir`, oldest first (names are
/// lexicographically chronological). Content-free: only Lore-owned paths are
/// returned, never archive data. Enables the recovery flow to offer a restore
/// from the newest local backup (SECURITY.md §6).
pub fn list_backups(backup_dir: &Path) -> Result<Vec<PathBuf>> {
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }
    let mut backups: Vec<PathBuf> = std::fs::read_dir(backup_dir)
        .map_err(|_| BackupError::Io)?
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
pub fn restore_backup(backup_path: &Path, dst_db_path: &Path) -> Result<()> {
    let mut dst = Connection::open(dst_db_path).map_err(|_| BackupError::Io)?;
    dst.restore(
        DatabaseName::Main,
        backup_path,
        None::<fn(rusqlite::backup::Progress)>,
    )?;
    drop(dst);
    verify(dst_db_path)
}

fn is_backup_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(BACKUP_PREFIX) && name.ends_with(&format!(".{BACKUP_EXT}"))
            })
}

/// Backup files inherit the app's private-file posture (SECURITY.md §2).
#[cfg(unix)]
fn set_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| BackupError::Io)
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<()> {
    Ok(())
}
