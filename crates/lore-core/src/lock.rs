//! Advisory lock serializing archive *writers*.
//!
//! ## Why this exists
//!
//! [`jobs::recover_running`](crate::jobs::recover_running) returns **every**
//! `running` job to `pending`. The `job` table has no owner column, so recovery
//! cannot tell "my own work, interrupted by a crash" from "another process's
//! work, in flight right now". That is the correct behaviour for the only writer
//! on a machine — a crash must not strand work — and it becomes a hijack the
//! moment there are two. A second writer calling `recover()` while the first is
//! mid-ingest reclaims the first's jobs, and both then run the same source.
//!
//! Adding an owner column would be the other fix, and a worse one: it makes
//! recovery depend on liveness detection (is that pid still alive? is it the
//! same process, or a reused pid?), which is exactly the problem an OS lock
//! already solves correctly.
//!
//! So writers take this lock for the whole time they may write, and recovery
//! stays simple. Readers never take it: reads open the archive
//! `SQLITE_OPEN_READ_ONLY` and cannot mutate it, so a query surface must not be
//! blocked by a running scan.
//!
//! ## Why `flock`, and not a lockfile with a pid in it
//!
//! The lock is released by the kernel when the file descriptor closes, including
//! on crash, `SIGKILL`, and power loss. A pid file has no such guarantee: it
//! survives the process that wrote it, so every reader of one needs stale-entry
//! heuristics that are wrong in the cases that matter (pid reuse, a debugger
//! stopped at a breakpoint, an NFS mount). Nothing here needs to decide whether
//! a holder is alive, because the kernel already knows.
//!
//! The lock file is created if absent and **never deleted**, not even on
//! release. Unlinking it would let a second process acquire a lock on an
//! unlinked inode while a third acquires one on its replacement, and both would
//! believe they held the archive. An empty file is a small price for that not
//! being possible.
//!
//! ## Scope, stated plainly
//!
//! This module provides the lock; a writer only benefits from it if it takes
//! it. Both do: `lorectl scan` holds it for a scan, and the desktop app takes
//! it at startup and holds it for its whole run (`src-tauri/src/lib.rs`), since
//! its worker may ingest at any moment. The visible consequence is deliberate —
//! `lorectl scan` exits 5 while the app is open.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::paths::SCAN_LOCK_FILENAME;

/// Why a writer could not take the archive lock.
///
/// Content-free, like [`StorageError`](crate::storage::StorageError): a variant
/// names a condition, never a path or archive contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LockError {
    /// Another process holds the lock. It is writing to this archive now.
    #[error("another Lore process is writing to this archive")]
    Held,
    /// The lock file could not be created or opened.
    #[error("could not open the archive lock file")]
    Io,
    /// This platform has no advisory file locking through `rustix::fs`, which is
    /// `cfg(not(windows))`. Refused rather than silently proceeding unlocked:
    /// a lock that quietly does nothing is worse than no lock, because callers
    /// would be entitled to believe they were serialized.
    #[error("advisory archive locking is not supported on this platform")]
    Unsupported,
}

/// An exclusive hold on one archive's writer lock.
///
/// Held for as long as the process may write. Dropping it — or exiting by any
/// means, including a crash — releases the lock, because the kernel releases it
/// when the descriptor closes.
///
/// The `File` is never read from or written to; only its descriptor matters.
#[derive(Debug)]
pub struct ScanLock {
    _file: File,
    path: PathBuf,
}

impl ScanLock {
    /// Take the writer lock for the archive at `archive_dir`, without waiting.
    ///
    /// Returns [`LockError::Held`] immediately if another process holds it,
    /// rather than blocking: a CLI that appears to hang is worse than one that
    /// says the archive is busy and lets the caller decide.
    ///
    /// `archive_dir` must exist. Creating it is the caller's business — this
    /// module does not decide where an archive lives, or that one should exist.
    pub fn acquire(archive_dir: &Path) -> Result<Self, LockError> {
        let path = archive_dir.join(SCAN_LOCK_FILENAME);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|_| LockError::Io)?;
        flock_exclusive_nonblocking(&file)?;
        Ok(Self { _file: file, path })
    }

    /// The lock file's path. Useful to a caller explaining which archive is
    /// busy; never required to release the lock.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn flock_exclusive_nonblocking(file: &File) -> Result<(), LockError> {
    use rustix::fs::{flock, FlockOperation};
    match flock(file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(()),
        // EWOULDBLOCK is the "someone else holds it" answer; every other errno
        // means the lock attempt itself failed and must not be read as success.
        Err(e) if e == rustix::io::Errno::WOULDBLOCK => Err(LockError::Held),
        Err(_) => Err(LockError::Io),
    }
}

#[cfg(not(unix))]
fn flock_exclusive_nonblocking(_file: &File) -> Result<(), LockError> {
    Err(LockError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_second_acquire_is_refused_while_the_first_is_held() {
        // flock is per open-file-description, not per process, so two opens in
        // one process contend exactly as two processes would.
        let dir = tempfile::tempdir().unwrap();
        let first = ScanLock::acquire(dir.path()).unwrap();
        assert_eq!(ScanLock::acquire(dir.path()).unwrap_err(), LockError::Held);
        drop(first);
    }

    #[cfg(unix)]
    #[test]
    fn releasing_lets_the_next_writer_in() {
        // The whole point: a finished scan must not lock out the next one.
        let dir = tempfile::tempdir().unwrap();
        drop(ScanLock::acquire(dir.path()).unwrap());
        let second = ScanLock::acquire(dir.path());
        assert!(second.is_ok(), "lock outlived its holder");
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_survives_release() {
        // Deleting it on release would allow two holders of two different
        // inodes; see the module docs.
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let lock = ScanLock::acquire(dir.path()).unwrap();
            lock.path().to_path_buf()
        };
        assert!(
            path.exists(),
            "the lock file must not be removed on release"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_lives_inside_the_archive_under_its_published_name() {
        let dir = tempfile::tempdir().unwrap();
        let lock = ScanLock::acquire(dir.path()).unwrap();
        assert_eq!(lock.path(), dir.path().join(SCAN_LOCK_FILENAME));
        assert_eq!(SCAN_LOCK_FILENAME, "scan.lock");
    }

    #[cfg(unix)]
    #[test]
    fn locks_on_different_archives_do_not_contend() {
        // The lock is per archive, not per machine: two archives are two
        // independent writers.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _first = ScanLock::acquire(a.path()).unwrap();
        assert!(ScanLock::acquire(b.path()).is_ok());
    }

    #[test]
    fn a_missing_archive_directory_is_an_io_error_not_a_panic() {
        // Creating the archive is the caller's job; this must degrade, not panic.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-archive");
        assert_eq!(ScanLock::acquire(&missing).unwrap_err(), LockError::Io);
    }

    #[test]
    fn every_error_names_a_distinct_condition() {
        // The contract is that a caller can tell these apart and act on them —
        // "another process is writing" and "this platform cannot lock" need
        // different responses. Content-freeness follows from the variants
        // carrying no data at all, which the type already guarantees.
        let texts: Vec<String> = [LockError::Held, LockError::Io, LockError::Unsupported]
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut unique = texts.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "two lock errors read the same");
        assert!(texts.iter().all(|t| !t.is_empty()));
    }
}
