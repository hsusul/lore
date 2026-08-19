//! SQLite storage: connection configuration and the migration runner.
//!
//! The archive is SQLite (WAL) with foreign keys enforced. FTS5 ships in the
//! bundled amalgamation. The full V0 schema — including the blob store and the
//! search/FTS tables — is applied here by ordered, checksummed migrations;
//! this module owns opening a configured connection and running them
//! transactionally.

pub mod blob;
pub mod migrations;

use std::path::Path;

use rusqlite::Connection;

/// Errors from the storage layer. Content-free: never embeds archive data.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("io error")]
    Io,
}

impl From<std::io::Error> for StorageError {
    fn from(_: std::io::Error) -> Self {
        StorageError::Io
    }
}

/// Convenience result alias for the storage layer.
pub type Result<T> = std::result::Result<T, StorageError>;

/// Open (creating if absent) the archive database at `path`, configure it, and
/// apply all pending migrations.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrations::run(&conn)?;
    Ok(conn)
}

/// Open an in-memory database with identical configuration and migrations.
/// Used by tests; WAL is a no-op for `:memory:` but foreign keys are enforced.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::run(&conn)?;
    Ok(conn)
}

/// Apply the connection pragmas Lore relies on. Run once at open, outside any
/// transaction. `journal_mode`/`synchronous` return rows, so we use
/// `execute_batch`, which ignores results.
///
/// The performance pragmas (`cache_size`, `temp_store`, `mmap_size`) are pure
/// speed/memory tuning with no durability or correctness effect: a larger page
/// cache and memory-backed temporaries keep the write-heavy initial scan's btree
/// and FTS pages resident instead of spilling, materially cutting ingest time on
/// a cold archive. `synchronous = NORMAL` is durable under WAL across app
/// crashes (only an OS/power loss can drop the last transaction), which a
/// re-scan recovers.
fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;\n\
         PRAGMA cache_size = -65536;\n\
         PRAGMA temp_store = MEMORY;\n\
         PRAGMA mmap_size = 268435456;",
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(())
}

/// Process-wide write serialization for the archive database.
///
/// The UI and the background ingest worker each hold their own SQLite connection
/// to the same WAL database. Two independent writers otherwise collide on
/// SQLite's single write lock and, once the busy-timeout is exceeded, surface
/// `SQLITE_BUSY` ("database is locked"). Every archive write path takes this lock
/// first, so writers serialize in-process and never contend at the SQLite layer.
/// Readers are unaffected — WAL readers never block on a writer.
///
/// Hold the guard only around the write transaction itself (stage blobs and
/// parse first), and never take it re-entrantly within a single call chain.
pub fn write_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foreign_keys_on(conn: &Connection) -> bool {
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap()
            == 1
    }

    #[test]
    fn migrations_apply_in_memory() {
        let conn = open_in_memory().unwrap();
        let applied: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 10, "all migrations should be recorded");
        assert!(foreign_keys_on(&conn), "foreign_keys must be enforced");
        // Infra tables exist.
        for t in ["setting", "job"] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {t} should exist");
        }
    }

    #[test]
    fn job_redo_column_is_added_with_default_zero() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO job (id, kind, created_at, updated_at)
             VALUES ('j', 'ingest_source', 0, 0)",
            [],
        )
        .unwrap();
        let redo: i64 = conn
            .query_row("SELECT redo FROM job WHERE id = 'j'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(redo, 0, "migration 0003 adds redo defaulting to 0");
    }

    #[test]
    fn job_failure_category_column_is_available() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO job
                (id, kind, state, error_kind, created_at, updated_at)
             VALUES ('failed', 'ingest_source', 'failed', 'source_io', 0, 0)",
            [],
        )
        .unwrap();
        let category: String = conn
            .query_row(
                "SELECT error_kind FROM job WHERE id = 'failed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(category, "source_io");
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        migrations::run(&conn).unwrap();
        let applied: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 10, "re-running migrations must not duplicate rows");
    }

    #[test]
    fn fts5_is_available() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(body);\n\
             INSERT INTO t(body) VALUES ('stripe webhook signature');",
        )
        .unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM t WHERE t MATCH 'signature'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1, "FTS5 MATCH must work");
    }

    #[test]
    fn open_file_backed_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.db");
        let conn = open(&path).unwrap();
        assert!(path.exists());
        assert!(foreign_keys_on(&conn));
    }

    #[test]
    fn storage_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let storage_err: StorageError = io_err.into();
        assert!(matches!(storage_err, StorageError::Io));
        assert_eq!(storage_err.to_string(), "io error");
    }
}
