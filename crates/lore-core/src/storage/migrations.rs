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
];

/// How many migrations exist. Tests assert against this rather than a literal
/// so adding a migration does not require editing an unrelated assertion — the
/// interesting property is "every migration is recorded exactly once", not the
/// number itself.
pub const COUNT: i64 = MIGRATIONS.len() as i64;

/// Apply all pending migrations. Idempotent.
pub fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT    NOT NULL,
            checksum   TEXT    NOT NULL,
            applied_at INTEGER NOT NULL
        );",
    )?;

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
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = run(&conn).unwrap_err();
        assert!(matches!(err, StorageError::Migration(_)));
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
}
