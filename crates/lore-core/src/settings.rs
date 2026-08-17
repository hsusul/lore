//! Persistent key/value settings backed by the `setting` table.
//!
//! Values are stored as JSON text (`value_json`) so a setting can hold any
//! shape; the helpers here cover the common raw-string and boolean cases.
//! Settings are Lore-owned application state in `lore.db`; clearing archived
//! content preserves them so preferences and configured source roots remain.

use rusqlite::{Connection, OptionalExtension};

use crate::storage::Result;

/// Read a setting's raw JSON value, or `None` when it has never been set.
pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value_json FROM setting WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?)
}

/// Upsert a setting to a raw JSON value, stamping `updated_at` (epoch millis).
pub fn set(conn: &Connection, key: &str, value_json: &str) -> Result<()> {
    let _write = crate::storage::write_lock();
    conn.execute(
        "INSERT INTO setting (key, value_json, updated_at)
         VALUES (?1, ?2, unixepoch('now') * 1000)
         ON CONFLICT(key) DO UPDATE SET
             value_json = excluded.value_json,
             updated_at = excluded.updated_at",
        (key, value_json),
    )?;
    Ok(())
}

/// Read a boolean setting, falling back to `default` when it is unset or its
/// stored value does not parse as a JSON boolean.
pub fn get_bool(conn: &Connection, key: &str, default: bool) -> Result<bool> {
    Ok(get(conn, key)?
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(default))
}

/// Persist a boolean setting.
pub fn set_bool(conn: &Connection, key: &str, value: bool) -> Result<()> {
    set(conn, key, if value { "true" } else { "false" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        crate::storage::open_in_memory().unwrap()
    }

    #[test]
    fn get_returns_none_for_an_unset_key() {
        let conn = db();
        assert_eq!(get(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn set_then_get_round_trips_and_upserts() {
        let conn = db();
        set(&conn, "theme", "\"dark\"").unwrap();
        assert_eq!(get(&conn, "theme").unwrap().as_deref(), Some("\"dark\""));

        // A second set for the same key overwrites rather than erroring on the
        // primary key, and stamps a non-zero updated_at.
        set(&conn, "theme", "\"light\"").unwrap();
        assert_eq!(get(&conn, "theme").unwrap().as_deref(), Some("\"light\""));
        let n: i64 = conn
            .query_row("SELECT count(*) FROM setting WHERE key='theme'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1, "upsert keeps a single row per key");
        let updated: i64 = conn
            .query_row(
                "SELECT updated_at FROM setting WHERE key='theme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(updated > 0, "updated_at is stamped");
    }

    #[test]
    fn bool_helpers_round_trip_and_default() {
        let conn = db();
        assert!(get_bool(&conn, "flag", true).unwrap(), "default when unset");
        assert!(!get_bool(&conn, "flag", false).unwrap());

        set_bool(&conn, "flag", true).unwrap();
        assert!(get_bool(&conn, "flag", false).unwrap());
        set_bool(&conn, "flag", false).unwrap();
        assert!(!get_bool(&conn, "flag", true).unwrap());

        // A non-boolean stored value falls back to the default.
        set(&conn, "flag", "\"nonsense\"").unwrap();
        assert!(get_bool(&conn, "flag", true).unwrap());
    }

    #[test]
    fn structured_json_payloads_round_trip() {
        let conn = db();
        let payload = r#"{"roots":["/custom/path"],"enabled":true,"limit":100}"#;
        set(&conn, "agent_roots.claude_code", payload).unwrap();
        let retrieved = get(&conn, "agent_roots.claude_code").unwrap().unwrap();
        assert_eq!(retrieved, payload);

        let parsed: serde_json::Value = serde_json::from_str(&retrieved).unwrap();
        assert_eq!(parsed["roots"][0], "/custom/path");
        assert_eq!(parsed["enabled"], true);
        assert_eq!(parsed["limit"], 100);
    }
}
