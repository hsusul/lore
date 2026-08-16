//! Recovery wiring for a corrupted archive (`docs/architecture/SECURITY.md` §6,
//! `TESTING.md` §7).
//!
//! On an integrity failure the flow preserves the corrupt archive as a
//! quarantine artifact (never discarded automatically — "never discard the only
//! archive"), then restores the newest Lore-owned local backup in its place.
//! Recovery works from a local backup **without original agent logs**; if no
//! usable backup exists the outcome reports the preserved quarantine path so
//! the caller can offer best-effort salvage or a re-scan of sources that still
//! exist. A healthy archive is never touched; an absent archive is a fresh
//! start.
//!
//! The caller must close all connections to the archive before recovering —
//! restore replaces the archive file wholesale. Content-free on failure: errors
//! never echo archive data or SQLite diagnostics.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::backup::{self, BackupError};

/// Per-process counter so quarantined artifacts never collide; names remain
/// lexicographically sortable.
static QUARANTINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Content-free outcome of [`recover_archive`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// No archive database exists yet (fresh start; the caller creates one).
    Absent,
    /// The archive passed `PRAGMA integrity_check`; nothing was touched.
    Healthy,
    /// The corrupt archive was preserved under `quarantine_path` and the newest
    /// Lore-owned backup was restored in its place.
    Restored {
        quarantine_path: PathBuf,
        backup_path: PathBuf,
    },
    /// The corrupt archive was preserved under `quarantine_path`, but no usable
    /// Lore-owned backup was available to restore.
    QuarantinedOnly { quarantine_path: PathBuf },
}

/// Errors from recovery. Content-free.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("sqlite error during recovery")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error during recovery")]
    Io,
    #[error("backup error during recovery")]
    Backup(#[from] BackupError),
}

/// Convenience result alias for the recovery layer.
pub type Result<T> = std::result::Result<T, RecoveryError>;

/// Recover the archive under `archive_dir` using Lore-owned backups under
/// `backup_dir`. See [`RecoveryOutcome`] for the possible results.
///
/// The caller must close every connection to `archive_dir/lore.db` first.
pub fn recover_archive(archive_dir: &Path, backup_dir: &Path) -> Result<RecoveryOutcome> {
    let db_path = archive_dir.join("lore.db");
    if !db_path.exists() {
        return Ok(RecoveryOutcome::Absent);
    }
    if integrity_ok(&db_path) {
        return Ok(RecoveryOutcome::Healthy);
    }

    let quarantine_path = quarantine(&db_path, archive_dir)?;

    // Restore the newest usable Lore-owned backup, if any. After quarantine the
    // archive location is gone, so the outcome must never be an error: the caller
    // has to know the archive was preserved, not that a restore "failed".
    let mut restored = None;
    if let Ok(backups) = backup::list_backups(backup_dir) {
        for backup_path in backups.into_iter().rev() {
            if backup::restore_backup(&backup_path, &db_path).is_ok() {
                restored = Some(backup_path);
                break;
            }
        }
    }
    match restored {
        Some(backup_path) => Ok(RecoveryOutcome::Restored {
            quarantine_path,
            backup_path,
        }),
        None => Ok(RecoveryOutcome::QuarantinedOnly { quarantine_path }),
    }
}

/// Does the database at `db_path` open cleanly and pass `PRAGMA
/// integrity_check`? A missing/corrupt file or a non-"ok" result is not intact.
fn integrity_ok(db_path: &Path) -> bool {
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .map(|result| result == "ok")
        .unwrap_or(false)
}

/// Preserve the corrupt archive (and any WAL/SHM sidecars) under a fresh
/// `quarantine/` artifact, returning its path. The archive location is emptied
/// so a restore can replace it wholesale.
fn quarantine(db_path: &Path, archive_dir: &Path) -> Result<PathBuf> {
    let quarantine_dir = archive_dir.join("quarantine");
    std::fs::create_dir_all(&quarantine_dir).map_err(|_| RecoveryError::Io)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = QUARANTINE_SEQ.fetch_add(1, Ordering::Relaxed);
    let db_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lore.db");

    let mut main_dst = None;
    for suffix in ["", "-wal", "-shm"] {
        let src = db_path.with_file_name(format!("{db_name}{suffix}"));
        if !src.exists() {
            continue;
        }
        let dst = quarantine_dir.join(format!("lore-{stamp:020}-{seq:04}{suffix}"));
        std::fs::rename(&src, &dst).map_err(|_| RecoveryError::Io)?;
        if suffix.is_empty() {
            main_dst = Some(dst);
        }
    }
    main_dst.ok_or(RecoveryError::Io)
}
