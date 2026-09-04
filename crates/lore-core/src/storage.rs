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
///
/// The read-only variants are deliberately distinct rather than one opaque
/// string, because a query surface has to tell a user *which* thing went wrong:
/// "you have no archive yet" and "your archive is newer than this binary" call
/// for opposite actions.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("io error")]
    Io,
    /// No file at the requested path. A read-only open never creates one.
    #[error("no archive at that path")]
    ArchiveMissing,
    /// The file opened as SQLite but carries no Lore migration ledger.
    #[error("not a Lore archive")]
    NotAnArchive,
    /// The archive was written by a newer build. Read-only callers cannot
    /// migrate *down*, and a newer schema may hold tables this build cannot
    /// interpret, so reading is refused rather than half-understood.
    #[error(
        "archive schema version {archive} is newer than this build supports \
         (up to {supported}); upgrade Lore to open it"
    )]
    SchemaTooNew { archive: i64, supported: i64 },
    /// The archive predates this build. A read-only connection cannot run the
    /// pending migrations, and querying an older schema would fail later with a
    /// confusing "no such table", so it is refused here with the fix named.
    #[error(
        "archive schema version {archive} predates this build (up to {supported}); \
         open it once with Lore, or run a scan, to upgrade it"
    )]
    SchemaNeedsUpgrade { archive: i64, supported: i64 },
    /// The migration ledger exists but violates an invariant the write path
    /// maintains (contiguous versions, checksums matching the embedded SQL).
    #[error("archive migration ledger is inconsistent: {0}")]
    SchemaInconsistent(&'static str),
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

/// Open the archive at `path` **read-only**: incapable of creating, migrating,
/// or modifying anything.
///
/// This is the connection every query surface takes — a CLI, and later an MCP
/// server answering an agent. Read-only is enforced by SQLite through
/// `SQLITE_OPEN_READ_ONLY` rather than by convention, so "an agent can query the
/// archive but can never change it" is a property of the handle, not a promise
/// about the code above it.
///
/// The schema is validated before the connection is returned, so a caller never
/// discovers an incompatible archive halfway through a query as "no such table".
///
/// ## What this does still touch
///
/// SQLite needs the `-shm` shared-memory index to read a WAL database, and
/// creates it (plus an empty `-wal`) if it is absent. Those are sidecars, not
/// archive content: no page of the database is written, and nothing Lore stores
/// is altered. It does mean the *directory* must be writable — on a genuinely
/// read-only filesystem with no pre-existing `-shm`, SQLite reports
/// `SQLITE_READONLY` at the first query rather than at open. Lore has no way to
/// avoid that short of copying the archive, which would be worse.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    // Opened first, then the failure is classified — rather than pre-checking
    // `is_file()`, which both opens a TOCTOU window and reports a directory or a
    // permission problem as a missing archive. `SQLITE_CANTOPEN` covers "no such
    // file" and "cannot read it", so only the former becomes `ArchiveMissing`;
    // anything else keeps SQLite's own diagnosis.
    let conn = match Connection::open_with_flags(path, flags) {
        Ok(conn) => conn,
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::CannotOpen && !path.exists() =>
        {
            return Err(StorageError::ArchiveMissing);
        }
        Err(error) => return Err(error.into()),
    };

    // `open` itself does not read the file header — SQLite defers that to the
    // first statement, which here is a pragma. So a file that is not a database
    // surfaces as `SQLITE_NOTADB` out of `configure_reader`, before the ledger
    // check ever runs, and has to be classified in both places.
    if let Err(error) = configure_reader(&conn) {
        return Err(reclassify_not_a_database(error));
    }
    check_schema_compatibility(&conn)?;
    Ok(conn)
}

/// `SQLITE_NOTADB` means the bytes are not a database at all, which is "not a
/// Lore archive" in every sense a caller acts on.
fn reclassify_not_a_database(error: StorageError) -> StorageError {
    match error {
        StorageError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::NotADatabase =>
        {
            StorageError::NotAnArchive
        }
        other => other,
    }
}

/// Validate that this build can read the archive on `conn`, without writing.
///
/// Separate from [`open_read_only`] so the rule can be exercised against an
/// arbitrary connection in tests.
fn check_schema_compatibility(conn: &Connection) -> Result<()> {
    match migrations::compatibility(conn)? {
        migrations::Compatibility::Ok => Ok(()),
        migrations::Compatibility::NotAnArchive => Err(StorageError::NotAnArchive),
        migrations::Compatibility::TooNew { archive, supported } => {
            Err(StorageError::SchemaTooNew { archive, supported })
        }
        migrations::Compatibility::NeedsUpgrade { archive, supported } => {
            Err(StorageError::SchemaNeedsUpgrade { archive, supported })
        }
        migrations::Compatibility::Inconsistent(why) => Err(StorageError::SchemaInconsistent(why)),
    }
}

/// Timeout a writer waits on SQLite's single write lock before giving up.
/// Generous: two writers legitimately take turns, and losing an ingest batch is
/// worse than a pause nobody is watching.
const WRITER_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Timeout a reader waits. Deliberately short: a WAL reader is not blocked by a
/// writer at all, so the only legitimate waits are brief WAL-index recovery.
/// Anything longer is a bug, and a query surface that stalls — a CLI, or a hook
/// that runs before every agent session — reads as a hang rather than as a wait.
const READER_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Pragmas shared by both connection kinds: connection-local, none writes a
/// database page.
///
/// These are pure speed/memory tuning with no durability or correctness effect.
/// A larger page cache and memory-backed temporaries keep the write-heavy
/// initial scan's btree and FTS pages resident instead of spilling, and give
/// readers room for FTS sorts. They return rows, so `execute_batch` is used; it
/// ignores results.
fn configure_common(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA cache_size = -65536;\n\
         PRAGMA temp_store = MEMORY;\n\
         PRAGMA mmap_size = 268435456;",
    )?;
    Ok(())
}

/// Apply the connection pragmas Lore relies on for a writable archive. Run once
/// at open, outside any transaction.
///
/// Everything here beyond [`configure_common`] is writer-only and deliberately
/// *not* applied to a reader:
///
/// - `foreign_keys` governs only statements that modify data, so on a reader it
///   provably does nothing. An inert pragma documented as inert is more
///   confusing than an absent one.
/// - `journal_mode = WAL` records the mode in the database header, which is a
///   write. On a read-only connection SQLite silently declines it and reports
///   the existing mode, so including it would look harmless while asserting
///   something the connection cannot do.
/// - `synchronous = NORMAL` controls fsync at commit and is meaningless without
///   commits. It is durable under WAL across app crashes (only an OS/power loss
///   can drop the last transaction), which a re-scan recovers.
fn configure(conn: &Connection) -> Result<()> {
    configure_common(conn)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;\n\
         PRAGMA journal_mode = WAL;\n\
         PRAGMA synchronous = NORMAL;",
    )?;
    conn.busy_timeout(WRITER_BUSY_TIMEOUT)?;
    Ok(())
}

/// Apply the read-safe subset. Nothing here can modify the database.
///
/// On the lock timeout: in WAL mode a reader is never blocked by a writer. A
/// reader can meet `SQLITE_BUSY` while the WAL index is being recovered after a
/// crash, against a connection in `EXCLUSIVE` locking mode, or across a
/// `journal_mode` change — all brief or pathological. Note the direction that is
/// often stated backwards: readers do not wait for checkpoints, they *hold up*
/// `RESTART` and `TRUNCATE` checkpoints.
fn configure_reader(conn: &Connection) -> Result<()> {
    configure_common(conn)?;
    conn.busy_timeout(READER_BUSY_TIMEOUT)?;
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
        assert_eq!(
            applied,
            migrations::COUNT,
            "all migrations should be recorded"
        );
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
        assert_eq!(
            applied,
            migrations::COUNT,
            "re-running migrations must not duplicate rows"
        );
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

    /// A migrated archive on disk, plus its directory guard.
    fn migrated_archive() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.db");
        let conn = open(&path).unwrap();
        drop(conn);
        (dir, path)
    }

    #[test]
    fn read_only_open_reads_a_migrated_archive() {
        let (_dir, path) = migrated_archive();
        let conn = open_read_only(&path).unwrap();
        let applied: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, migrations::COUNT);
    }

    /// The realistic case: the writer has fully closed, so SQLite must rebuild
    /// the `-shm` index before it can read a WAL database. This is the open that
    /// fails on a read-only *directory*, so it is worth pinning explicitly.
    #[test]
    fn read_only_open_works_after_the_writer_has_closed() {
        let (_dir, path) = migrated_archive();
        let conn = open_read_only(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "reader sees the archive's mode");
    }

    #[test]
    fn read_only_connection_cannot_write_in_any_form() {
        let (_dir, path) = migrated_archive();
        let conn = open_read_only(&path).unwrap();

        // One representative statement per mutation class. Each must fail with
        // SQLITE_READONLY specifically — not merely "some error", which a typo
        // in the SQL would also produce.
        let statements = [
            "INSERT INTO agent (id, display_name) VALUES ('x', 'X')",
            "UPDATE agent SET display_name = 'Y'",
            "DELETE FROM agent",
            "CREATE TABLE intruder (x INTEGER)",
            "DROP TABLE agent",
            "ALTER TABLE agent ADD COLUMN sneaky TEXT",
        ];
        for sql in statements {
            let error = conn.execute(sql, []).unwrap_err();
            match error {
                rusqlite::Error::SqliteFailure(e, _) => assert_eq!(
                    e.code,
                    rusqlite::ErrorCode::ReadOnly,
                    "`{sql}` should fail as read-only, got {e:?}"
                ),
                other => panic!("`{sql}` should fail as read-only, got {other:?}"),
            }
        }
    }

    #[test]
    fn read_only_open_never_creates_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("lore.db");
        let error = open_read_only(&path).unwrap_err();
        assert!(matches!(error, StorageError::ArchiveMissing));
        assert!(!path.exists(), "must not create the file");
        assert!(!path.parent().unwrap().exists(), "must not create the dir");
    }

    #[test]
    fn read_only_open_rejects_a_sqlite_file_that_is_not_a_lore_archive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foreign.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE unrelated (x INTEGER);")
                .unwrap();
        }
        let error = open_read_only(&path).unwrap_err();
        assert!(matches!(error, StorageError::NotAnArchive));
    }

    #[test]
    fn read_only_open_refuses_an_archive_from_a_newer_build() {
        let (_dir, path) = migrated_archive();
        {
            let writer = Connection::open(&path).unwrap();
            writer
                .execute(
                    "INSERT INTO schema_migrations (version, name, checksum, applied_at)
                     VALUES (?1, 'future', 'ffffffffffffffff', 0)",
                    [migrations::COUNT + 1],
                )
                .unwrap();
        }
        let error = open_read_only(&path).unwrap_err();
        match error {
            StorageError::SchemaTooNew { archive, supported } => {
                assert_eq!(archive, migrations::COUNT + 1);
                assert_eq!(supported, migrations::COUNT);
                // The message has to tell the user what to do about it.
                let text = StorageError::SchemaTooNew { archive, supported }.to_string();
                assert!(text.contains("upgrade Lore"), "actionable: {text}");
            }
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
    }

    #[test]
    fn read_only_open_refuses_an_archive_awaiting_migration() {
        let (_dir, path) = migrated_archive();
        {
            let writer = Connection::open(&path).unwrap();
            writer
                .execute(
                    "DELETE FROM schema_migrations WHERE version = ?1",
                    [migrations::COUNT],
                )
                .unwrap();
        }
        let error = open_read_only(&path).unwrap_err();
        assert!(matches!(error, StorageError::SchemaNeedsUpgrade { .. }));
    }

    #[test]
    fn read_only_open_does_not_change_the_database_contents() {
        let (_dir, path) = migrated_archive();
        let before = std::fs::read(&path).unwrap();
        {
            let conn = open_read_only(&path).unwrap();
            let _: i64 = conn
                .query_row("SELECT count(*) FROM agent", [], |r| r.get(0))
                .unwrap();
        }
        // Byte-for-byte: the reader may create `-shm`/`-wal` sidecars, but the
        // archive itself must be untouched.
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn reader_and_writer_pragmas_differ_only_where_intended() {
        let (_dir, path) = migrated_archive();
        let writer = open(&path).unwrap();
        let reader = open_read_only(&path).unwrap();

        let query = |c: &Connection, p: &str| -> i64 {
            c.query_row(&format!("PRAGMA {p}"), [], |r| r.get(0))
                .unwrap()
        };
        for pragma in ["cache_size", "temp_store", "mmap_size"] {
            assert_eq!(
                query(&writer, pragma),
                query(&reader, pragma),
                "{pragma} is shared tuning"
            );
        }
        // `synchronous` is writer-only and observably different.
        assert_eq!(query(&writer, "synchronous"), 1, "writer is NORMAL");

        // Foreign keys are ON for both, and NOT because the reader sets them:
        // rusqlite's bundled amalgamation is compiled with
        // `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, so every connection starts with them
        // enabled. The writer still sets the pragma explicitly, as a statement of
        // intent that survives a change to that build flag; on a reader — which
        // can execute no statement the pragma governs — it would be pure noise.
        assert_eq!(query(&writer, "foreign_keys"), 1);
        assert_eq!(query(&reader, "foreign_keys"), 1);
    }

    #[test]
    fn the_reader_waits_far_less_than_the_writer_for_a_lock() {
        // A query surface that stalls reads as a hang. The exact values matter
        // less than the relationship and the reader's ceiling.
        assert!(READER_BUSY_TIMEOUT < WRITER_BUSY_TIMEOUT);
        assert!(READER_BUSY_TIMEOUT <= std::time::Duration::from_secs(1));
    }

    #[test]
    fn read_only_open_rejects_a_file_that_is_not_sqlite_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("garbage.db");
        std::fs::write(&path, b"definitely not a database").unwrap();
        assert!(matches!(
            open_read_only(&path).unwrap_err(),
            StorageError::NotAnArchive
        ));
    }

    #[test]
    fn read_only_open_rejects_an_empty_file() {
        // A zero-byte file is a valid, empty SQLite database — so it opens, and
        // is classified by the ledger check rather than by SQLite.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.db");
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(
            open_read_only(&path).unwrap_err(),
            StorageError::NotAnArchive
        ));
    }

    #[test]
    fn read_only_open_rejects_another_tools_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rails.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_migrations (version VARCHAR PRIMARY KEY);
                 INSERT INTO schema_migrations VALUES ('20240101120000');",
            )
            .unwrap();
        }
        assert!(matches!(
            open_read_only(&path).unwrap_err(),
            StorageError::NotAnArchive
        ));
    }

    #[test]
    fn read_only_open_reports_a_directory_as_sqlite_cannot_open_not_as_missing() {
        // The old `is_file()` pre-check called this "no archive", which is wrong:
        // something *is* there. SQLite's own diagnosis is the honest one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a-directory");
        std::fs::create_dir(&path).unwrap();
        let error = open_read_only(&path).unwrap_err();
        assert!(
            !matches!(error, StorageError::ArchiveMissing),
            "a directory is not a missing archive, got {error:?}"
        );
    }

    #[test]
    fn the_writer_refuses_an_archive_migrated_by_a_newer_build() {
        // The invariant that makes two independently-upgraded binaries safe on
        // one archive: an older build must not ingest under a newer schema.
        let (_dir, path) = migrated_archive();
        {
            let writer = Connection::open(&path).unwrap();
            writer
                .execute(
                    "INSERT INTO schema_migrations (version, name, checksum, applied_at)
                     VALUES (?1, 'future', 'ffffffffffffffff', 0)",
                    [migrations::COUNT + 1],
                )
                .unwrap();
        }
        assert!(matches!(
            open(&path).unwrap_err(),
            StorageError::SchemaTooNew { .. }
        ));
    }

    #[test]
    fn storage_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let storage_err: StorageError = io_err.into();
        assert!(matches!(storage_err, StorageError::Io));
        assert_eq!(storage_err.to_string(), "io error");
    }

    #[test]
    fn file_backed_database_enforces_wal_journal_mode_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore_wal.db");
        let conn = open(&path).unwrap();

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let sync_mode: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync_mode, 1); // 1 == NORMAL

        assert!(foreign_keys_on(&conn));
    }
}
