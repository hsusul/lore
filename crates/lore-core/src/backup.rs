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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{backup::Backup, Connection};

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

    set_private(&path)?;
    verify(&path)?;
    prune(backup_dir, keep)?;

    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(BackupInfo { path, size_bytes })
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
    let mut backups: Vec<PathBuf> = std::fs::read_dir(backup_dir)
        .map_err(|_| BackupError::Io)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_backup_file(path))
        .collect();
    if backups.len() <= keep {
        return Ok(());
    }
    let drop_count = backups.len() - keep;
    backups.sort();
    for old in backups.into_iter().take(drop_count) {
        std::fs::remove_file(&old).map_err(|_| BackupError::Io)?;
    }
    Ok(())
}

fn is_backup_file(path: &Path) -> bool {
    path.file_name()
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
