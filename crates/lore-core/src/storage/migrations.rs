//! Ordered, checksummed SQL migrations.
//!
//! Each migration runs inside a single transaction together with the row that
//! records it in `schema_migrations`, so a partially-applied migration can
//! never be observed. Recorded checksums are re-verified on every startup to
//! catch an accidentally edited, already-applied migration.

use rusqlite::{Connection, OptionalExtension};

use super::{Result, StorageError};

/// One embedded migration. `sql` may contain multiple statements.
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// The ordered migration set. Append-only; never edit an applied migration.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "schema",
        sql: include_str!("../../migrations/0002_schema.sql"),
    },
    Migration {
        version: 3,
        name: "job_redo",
        sql: include_str!("../../migrations/0003_job_redo.sql"),
    },
    Migration {
        version: 4,
        name: "identity_indexes",
        sql: include_str!("../../migrations/0004_identity_indexes.sql"),
    },
    Migration {
        version: 5,
        name: "source_artifact_indexes",
        sql: include_str!("../../migrations/0005_source_artifact_indexes.sql"),
    },
    Migration {
        version: 6,
        name: "job_error_kind",
        sql: include_str!("../../migrations/0006_job_error_kind.sql"),
    },
    Migration {
        version: 7,
        name: "search_document_sort_keys",
        sql: include_str!("../../migrations/0007_search_document_sort_keys.sql"),
    },
    Migration {
        version: 8,
        name: "folders",
        sql: include_str!("../../migrations/0008_folders.sql"),
    },
    Migration {
        version: 9,
        name: "query_path_indexes",
        sql: include_str!("../../migrations/0009_query_path_indexes.sql"),
    },
    Migration {
        version: 10,
        name: "blob_hash_algo",
        sql: include_str!("../../migrations/0010_blob_hash_algo.sql"),
    },
    Migration {
        version: 11,
        name: "search_git",
        sql: include_str!("../../migrations/0011_search_git.sql"),
    },
    Migration {
        version: 12,
        name: "search_git_filter_indexes",
        sql: include_str!("../../migrations/0012_search_git_filter_indexes.sql"),
    },
    Migration {
        version: 13,
        name: "file_content_identity",
        sql: include_str!("../../migrations/0013_file_content_identity.sql"),
    },
];

/// How many migrations exist. Tests assert against this rather than a literal
/// so adding a migration does not require editing an unrelated assertion — the
/// interesting property is "every migration is recorded exactly once", not the
/// number itself.
pub const COUNT: i64 = MIGRATIONS.len() as i64;

/// Verdict on whether this build can read an archive, decided without writing.
///
/// A read-only connection cannot run migrations, so it has to establish up front
/// that the schema it is about to query is the one it was compiled against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    /// Every migration this build knows is applied, and no others.
    Ok,
    /// No migration ledger: an empty or foreign SQLite file.
    NotAnArchive,
    /// Written by a newer build.
    TooNew { archive: i64, supported: i64 },
    /// Written by an older build; migrations are pending.
    NeedsUpgrade { archive: i64, supported: i64 },
    /// The ledger violates an invariant [`run`] maintains.
    Inconsistent(&'static str),
}

/// Read the migration ledger and judge it against this build. Never writes.
///
/// This reuses the write path's invariants rather than inventing a second,
/// weaker interpretation of migration state. `MAX(version)` alone would accept a
/// ledger with holes, or one whose recorded SQL differs from the SQL this binary
/// embeds — exactly the corruption [`apply`] refuses on the write path. So the
/// same three properties are checked here: versions are contiguous from 1, the
/// count matches [`COUNT`], and every recorded checksum equals the checksum of
/// the migration this build would have applied.
pub fn compatibility(conn: &Connection) -> Result<Compatibility> {
    // A file that is not SQLite at all fails here, on the first statement. That
    // is "not a Lore archive" in every sense a caller cares about, so it is
    // classified rather than surfaced as a raw SQLite error.
    let ledger_exists = match conn.query_row(
        "SELECT count(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'schema_migrations'",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(count) => count > 0,
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::NotADatabase =>
        {
            return Ok(Compatibility::NotAnArchive);
        }
        Err(error) => return Err(error.into()),
    };
    if !ledger_exists {
        return Ok(Compatibility::NotAnArchive);
    }

    // The table name is not enough: `schema_migrations` is a common name, and
    // other tools' versions of it have different columns and types (Rails uses a
    // single `version VARCHAR`). Selecting blindly would surface `no such
    // column` or `InvalidColumnType` as an opaque SQLite error on exactly the
    // input this function exists to classify.
    if !ledger_shape_matches(conn)? {
        return Ok(Compatibility::NotAnArchive);
    }

    let mut stmt =
        conn.prepare("SELECT version, checksum FROM schema_migrations ORDER BY version ASC")?;
    let recorded = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // The table can exist with no rows only if something created it and then
    // failed before the first migration committed — never a readable archive.
    if recorded.is_empty() {
        return Ok(Compatibility::NotAnArchive);
    }

    for (index, (version, _)) in recorded.iter().enumerate() {
        if *version != (index + 1) as i64 {
            return Ok(Compatibility::Inconsistent(
                "recorded versions are not contiguous from 1",
            ));
        }
    }

    let archive = recorded.len() as i64;
    if archive > COUNT {
        return Ok(Compatibility::TooNew {
            archive,
            supported: COUNT,
        });
    }

    // Checked over the applied prefix, which is all this build can vouch for.
    // A mismatch means the archive's schema is not the schema this binary's SQL
    // describes, so its queries would be reasoning about the wrong tables.
    for (recorded_version, recorded_checksum) in &recorded {
        let Some(migration) = MIGRATIONS.iter().find(|m| m.version == *recorded_version) else {
            return Ok(Compatibility::Inconsistent(
                "an applied version is unknown to this build",
            ));
        };
        if fnv1a_hex(migration.sql) != *recorded_checksum {
            return Ok(Compatibility::Inconsistent(
                "an applied migration's checksum does not match this build",
            ));
        }
    }

    if archive < COUNT {
        return Ok(Compatibility::NeedsUpgrade {
            archive,
            supported: COUNT,
        });
    }

    Ok(Compatibility::Ok)
}

/// Does `schema_migrations` have the columns this build reads, with the types it
/// reads them as? Checked via `PRAGMA table_info`, which never errors on an
/// unexpected shape the way a `SELECT` of missing columns would.
fn ledger_shape_matches(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(schema_migrations)")?;
    let columns = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let has = |name: &str, ty: &str| {
        columns
            .iter()
            .any(|(n, t)| n == name && t.eq_ignore_ascii_case(ty))
    };
    Ok(has("version", "INTEGER") && has("checksum", "TEXT"))
}

/// Apply all pending migrations. Idempotent.
///
/// Refuses an archive this build does not understand, for the same reason
/// [`compatibility`] does — and it matters more here, because a writer that
/// proceeds anyway *records new rows under the wrong schema*. From the moment
/// two binaries share one archive (the desktop app and a CLI, upgraded
/// independently), an older build could otherwise open an archive migrated by a
/// newer one, see only the versions it knows, report success, and ingest data
/// while skipping every column the newer schema added. The result is missing
/// derived data that a later read reports as *absent evidence* rather than as
/// absent computation — precisely the claim-outruns-evidence failure the
/// product cannot afford.
///
/// The rule both paths share: **one schema version per archive at a time.**
/// A fresh or half-created archive (`NotAnArchive`) and one with pending
/// migrations (`NeedsUpgrade`) are exactly what this function is for, so only
/// `TooNew` and `Inconsistent` are refused.
pub fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT    NOT NULL,
            checksum   TEXT    NOT NULL,
            applied_at INTEGER NOT NULL
        );",
    )?;

    match compatibility(conn)? {
        // Nothing applied yet, or every applied migration agrees with this
        // build: both are this function's normal input.
        Compatibility::Ok | Compatibility::NotAnArchive | Compatibility::NeedsUpgrade { .. } => {}
        Compatibility::TooNew { archive, supported } => {
            return Err(StorageError::SchemaTooNew { archive, supported });
        }
        Compatibility::Inconsistent(why) => return Err(StorageError::SchemaInconsistent(why)),
    }

    let mut previous = 0_i64;
    for m in MIGRATIONS {
        if m.version <= previous {
            return Err(StorageError::Migration(format!(
                "migrations out of order at version {}",
                m.version
            )));
        }
        previous = m.version;
        apply(conn, m)?;
    }
    Ok(())
}

fn apply(conn: &Connection, m: &Migration) -> Result<()> {
    let checksum = fnv1a_hex(m.sql);

    let recorded: Option<String> = conn
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version = ?1",
            [m.version],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(recorded) = recorded {
        if recorded != checksum {
            return Err(StorageError::Migration(format!(
                "migration {} was modified after being applied (checksum mismatch)",
                m.version
            )));
        }
        return Ok(()); // already applied
    }

    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(m.sql)?;
    tx.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (?1, ?2, ?3, unixepoch('now') * 1000)",
        rusqlite::params![m.version, m.name, checksum],
    )?;
    tx.commit()?;
    Ok(())
}

/// FNV-1a 64-bit hex digest — a small, dependency-free content fingerprint used
/// only to detect edited-after-apply migrations (not a security primitive).
fn fnv1a_hex(s: &str) -> String {
    crate::hash::fnv1a64_hex(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        /// `compatibility` is total over arbitrary ledger contents: it classifies
        /// rather than erroring, never panics, and returns `Ok` only for a ledger
        /// that is exactly this build's migration set.
        #[test]
        fn compatibility_is_total_over_arbitrary_ledgers(
            rows in proptest::collection::vec((-3i64..20, "[0-9a-f]{0,16}"), 0..20)
        ) {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_migrations (
                    version    INTEGER PRIMARY KEY,
                    name       TEXT    NOT NULL,
                    checksum   TEXT    NOT NULL,
                    applied_at INTEGER NOT NULL
                );",
            )
            .unwrap();
            for (version, checksum) in &rows {
                // Duplicate versions collide on the primary key; skip those.
                let _ = conn.execute(
                    "INSERT INTO schema_migrations (version, name, checksum, applied_at)
                     VALUES (?1, 'x', ?2, 0)",
                    rusqlite::params![version, checksum],
                );
            }

            let verdict = compatibility(&conn).unwrap();

            // Read back what actually landed and derive the expected verdict.
            let mut stmt = conn
                .prepare("SELECT version, checksum FROM schema_migrations ORDER BY version ASC")
                .unwrap();
            let stored: Vec<(i64, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(std::result::Result::unwrap)
                .collect();
            let exact = stored.len() as i64 == COUNT
                && stored.iter().enumerate().all(|(i, (v, c))| {
                    *v == (i + 1) as i64 && fnv1a_hex(MIGRATIONS[i].sql) == *c
                });
            proptest::prop_assert_eq!(verdict == Compatibility::Ok, exact);
        }
    }

    #[test]
    fn checksum_is_stable_and_content_sensitive() {
        assert_eq!(fnv1a_hex("abc"), fnv1a_hex("abc"));
        assert_ne!(fnv1a_hex("abc"), fnv1a_hex("abd"));
        assert_eq!(fnv1a_hex("abc").len(), 16);
    }

    #[test]
    fn detects_modified_applied_migration() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        // Simulate an edited migration by corrupting the recorded checksum.
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'deadbeefdeadbeef' WHERE version = 1",
            [],
        )
        .unwrap();
        // Caught by the up-front compatibility gate now, before any migration is
        // reconsidered — `apply`'s own per-migration check remains as a backstop.
        let err = run(&conn).unwrap_err();
        assert!(matches!(err, StorageError::SchemaInconsistent(_)));
    }

    /// The rule that makes two independently-upgraded binaries safe on one
    /// archive: a writer refuses a schema it does not understand instead of
    /// recording rows under it.
    #[test]
    fn run_refuses_an_archive_migrated_by_a_newer_build() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (?1, 'future', 'ffffffffffffffff', 0)",
            [COUNT + 1],
        )
        .unwrap();
        let err = run(&conn).unwrap_err();
        assert!(matches!(
            err,
            StorageError::SchemaTooNew {
                archive,
                supported
            } if archive == COUNT + 1 && supported == COUNT
        ));
    }

    #[test]
    fn run_refuses_a_ledger_with_a_hole() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .unwrap();
        assert!(matches!(
            run(&conn).unwrap_err(),
            StorageError::SchemaInconsistent(_)
        ));
    }

    /// A fresh database is `run`'s most common input and classifies as
    /// `NotAnArchive` before anything is applied, so the new gate must permit it.
    /// Getting this wrong would brick first run.
    #[test]
    fn run_migrates_a_fresh_database_the_gate_sees_as_not_an_archive() {
        let fresh = Connection::open_in_memory().unwrap();
        assert_eq!(compatibility(&fresh).unwrap(), Compatibility::NotAnArchive);
        run(&fresh).unwrap();
        assert_eq!(compatibility(&fresh).unwrap(), Compatibility::Ok);
    }

    /// An archive with migrations pending is exactly what `run` exists to fix,
    /// so the gate must not refuse it.
    ///
    /// The partial state is simulated by removing the last ledger row, which
    /// leaves that migration's DDL in place — so re-running it fails on its own
    /// objects. That artefact is not what is under test: the assertion is only
    /// that the failure is *not* the schema gate turning `NeedsUpgrade` away.
    #[test]
    fn the_gate_does_not_refuse_an_archive_awaiting_migration() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = ?1", [COUNT])
            .unwrap();
        assert!(matches!(
            compatibility(&conn).unwrap(),
            Compatibility::NeedsUpgrade { .. }
        ));
        if let Err(error) = run(&conn) {
            assert!(
                !matches!(error, StorageError::SchemaNeedsUpgrade { .. }),
                "the gate must let a pending migration through, got {error:?}"
            );
        }
    }

    #[test]
    fn compatibility_classifies_a_file_that_is_not_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("garbage.db");
        std::fs::write(&path, b"this is not a database, just some bytes").unwrap();
        let conn = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::NotAnArchive);
    }

    #[test]
    fn compatibility_classifies_another_tools_schema_migrations_table() {
        // Rails' ledger: same table name, one `version VARCHAR` column. Selecting
        // `checksum` blindly would raise `no such column`.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version VARCHAR PRIMARY KEY);
             INSERT INTO schema_migrations VALUES ('20240101120000');",
        )
        .unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::NotAnArchive);
    }

    #[test]
    fn compatibility_classifies_a_ledger_with_the_wrong_column_types() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                version TEXT PRIMARY KEY, name TEXT, checksum BLOB, applied_at TEXT
             );",
        )
        .unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::NotAnArchive);
    }

    #[test]
    fn compatibility_rejects_a_non_positive_version() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (0, 'bogus', 'ffffffffffffffff', 0)",
            [],
        )
        .unwrap();
        assert!(matches!(
            compatibility(&conn).unwrap(),
            Compatibility::Inconsistent(_)
        ));
    }

    /// Check ordering is deliberate: contiguity is judged before version count,
    /// so a ledger that is both corrupt and too new reports the corruption.
    #[test]
    fn a_corrupt_and_too_new_ledger_reports_the_corruption_first() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (?1, 'future', 'ffffffffffffffff', 0)",
            [COUNT + 1],
        )
        .unwrap();
        assert!(matches!(
            compatibility(&conn).unwrap(),
            Compatibility::Inconsistent(_)
        ));
    }

    #[test]
    fn run_migrations_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let count_1: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_1, COUNT);

        // Second run must be a successful no-op
        run(&conn).unwrap();

        let count_2: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_2, COUNT);
    }

    #[test]
    fn apply_rejects_sql_with_added_whitespace_or_comments() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version    INTEGER PRIMARY KEY,
                name       TEXT    NOT NULL,
                checksum   TEXT    NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let m1 = Migration {
            version: 1,
            name: "initial",
            sql: "CREATE TABLE t (id INT);",
        };
        apply(&conn, &m1).unwrap();

        // Modifying SQL with trailing space changes checksum and fails
        let m1_modified = Migration {
            version: 1,
            name: "initial",
            sql: "CREATE TABLE t (id INT); ",
        };
        let err = apply(&conn, &m1_modified).unwrap_err();
        assert!(matches!(err, StorageError::Migration(_)));
        assert!(err.to_string().contains("checksum mismatch"));
    }

    #[test]
    fn schema_migrations_table_has_expected_columns_and_valid_timestamps() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let mut stmt = conn
            .prepare("SELECT version, name, checksum, applied_at FROM schema_migrations ORDER BY version ASC")
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(rows.len() as i64, COUNT);
        for (i, (v, name, checksum, applied_at)) in rows.into_iter().enumerate() {
            assert_eq!(v, (i + 1) as i64);
            assert!(!name.trim().is_empty());
            assert_eq!(checksum.len(), 16);
            assert!(applied_at > 0);
        }
    }

    #[test]
    fn apply_atomic_rollback_on_syntax_error_in_migration() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version    INTEGER PRIMARY KEY,
                name       TEXT    NOT NULL,
                checksum   TEXT    NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        let bad_migration = Migration {
            version: 1,
            name: "bad_syntax",
            sql: "CREATE TABLE t (id INT); SYNTAX ERROR HERE;",
        };
        assert!(apply(&conn, &bad_migration).is_err());

        // Table 't' and schema_migrations record must not exist
        let t_exists: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='t'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            > 0;
        assert!(!t_exists);

        let mig_count: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mig_count, 0);
    }

    #[test]
    fn fnv1a_hex_determinism_and_length() {
        let digest1 = fnv1a_hex("CREATE TABLE test (id INTEGER);");
        let digest2 = fnv1a_hex("CREATE TABLE test (id INTEGER);");
        assert_eq!(digest1, digest2);
        assert_eq!(digest1.len(), 16);

        let digest3 = fnv1a_hex("CREATE TABLE test2 (id INTEGER);");
        assert_ne!(digest1, digest3);
    }

    #[test]
    fn migrations_run_idempotent_on_populated_database() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(run(&conn).is_ok());
        assert!(run(&conn).is_ok());

        let count: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, COUNT);
    }

    #[test]
    fn fnv1a_hex_empty_string_matches_offset_basis() {
        let digest = fnv1a_hex("");
        assert_eq!(digest, "cbf29ce484222325");
    }

    #[test]
    fn compatibility_accepts_a_fully_migrated_archive() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::Ok);
    }

    #[test]
    fn compatibility_rejects_a_database_with_no_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::NotAnArchive);
        // A foreign SQLite file with unrelated tables is equally not an archive.
        conn.execute_batch("CREATE TABLE unrelated (x INTEGER);")
            .unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::NotAnArchive);
    }

    #[test]
    fn compatibility_rejects_an_empty_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute("DELETE FROM schema_migrations", []).unwrap();
        assert_eq!(compatibility(&conn).unwrap(), Compatibility::NotAnArchive);
    }

    #[test]
    fn compatibility_refuses_an_archive_from_a_newer_build() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        // Stand in for a future migration this build knows nothing about.
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (?1, 'future', 'ffffffffffffffff', 0)",
            [COUNT + 1],
        )
        .unwrap();
        assert_eq!(
            compatibility(&conn).unwrap(),
            Compatibility::TooNew {
                archive: COUNT + 1,
                supported: COUNT,
            }
        );
    }

    #[test]
    fn compatibility_reports_an_archive_that_still_needs_migrating() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = ?1", [COUNT])
            .unwrap();
        assert_eq!(
            compatibility(&conn).unwrap(),
            Compatibility::NeedsUpgrade {
                archive: COUNT - 1,
                supported: COUNT,
            }
        );
    }

    #[test]
    fn compatibility_rejects_a_ledger_with_a_hole() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        // Remove a middle version: MAX(version) is unchanged, so a naive
        // version-only check would wrongly call this archive current.
        conn.execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .unwrap();
        assert!(matches!(
            compatibility(&conn).unwrap(),
            Compatibility::Inconsistent(_)
        ));
    }

    #[test]
    fn compatibility_rejects_a_checksum_that_does_not_match_this_build() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'deadbeefdeadbeef' WHERE version = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            compatibility(&conn).unwrap(),
            Compatibility::Inconsistent(_)
        ));
    }
}
