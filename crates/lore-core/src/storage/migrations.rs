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
];

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
}
