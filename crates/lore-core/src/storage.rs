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
    #[error("io error reading source")]
    Io,
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
fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;",
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
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
        assert_eq!(applied, 3, "all migrations should be recorded");
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
    fn migrations_are_idempotent() {
        let conn = open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        migrations::run(&conn).unwrap();
        let applied: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 3, "re-running migrations must not duplicate rows");
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
}
