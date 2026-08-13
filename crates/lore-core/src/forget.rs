//! Deletion: "forget a session" and "forget everything" (`SECURITY.md` §6).
//!
//! Forgetting a session transactionally removes its canonical rows, search
//! projections (so the FTS index is cleared), findings, and any blob that is no
//! longer referenced by anything — then runs `secure_delete` + `VACUUM`
//! maintenance to zero freed pages. Original agent logs and user-chosen exports
//! are **outside Lore's ownership** and are never deleted; the report names the
//! source copies that remain so the UI can disclose them.

use std::path::Path;

use rusqlite::Connection;

use crate::storage::blob::BlobStore;
use crate::storage::{Result, StorageError};

/// Outcome of forgetting a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgetReport {
    /// Unreferenced blobs whose rows and files were removed.
    pub blobs_removed: usize,
    /// Original agent source paths (not owned by Lore) that still exist on disk.
    pub source_paths: Vec<String>,
}

/// Forget one session: remove its rows, projections, findings, and now-orphaned
/// blobs, then run secure-delete maintenance. A blob still referenced by another
/// session is retained.
pub fn forget_session(
    conn: &Connection,
    blobs: &BlobStore,
    session_id: &str,
) -> Result<ForgetReport> {
    let source_paths = session_source_paths(conn, session_id)?;

    // Zero freed pages for subsequent deletes in this connection.
    conn.execute_batch("PRAGMA secure_delete = ON;")?;

    let orphans = {
        let tx = conn.unchecked_transaction()?;
        // Delete projections first so the FTS external-content delete trigger
        // fires (an FK cascade would not), then cascade the rest.
        tx.execute(
            "DELETE FROM search_document WHERE session_id = ?1",
            [session_id],
        )?;
        tx.execute("DELETE FROM agent_session WHERE id = ?1", [session_id])?;

        let orphans = orphan_blobs(&tx)?;
        for (id, _) in &orphans {
            tx.execute("DELETE FROM blob WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        orphans
    };

    for (_, relpath) in &orphans {
        blobs.remove(relpath)?;
    }

    // Best-effort maintenance: checkpoint the WAL and vacuum to reclaim + zero
    // freed space. Failure here does not undo the deletion.
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;");

    Ok(ForgetReport {
        blobs_removed: orphans.len(),
        source_paths,
    })
}

/// Forget **all** archive content, keeping the database file/connection open
/// (the connection-safe form of "forget everything" for a running app): remove
/// every session, repository, source, projection, finding, and blob, then run
/// secure-delete maintenance. Settings and the job queue are preserved. The
/// file-level [`forget_everything`] is for a full uninstall instead.
pub fn forget_all(conn: &Connection, blobs: &BlobStore) -> Result<ForgetReport> {
    conn.execute_batch("PRAGMA secure_delete = ON;")?;
    let orphans = {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
        // Delete projections first so the FTS external-content trigger fires.
        tx.execute("DELETE FROM search_document", [])?;
        tx.execute("DELETE FROM agent_session", [])?;
        tx.execute("DELETE FROM repository", [])?;
        tx.execute("DELETE FROM source_artifact", [])?;
        tx.execute("DELETE FROM agent", [])?;
        // Every blob is now unreferenced.
        let orphans = orphan_blobs(&tx)?;
        tx.execute("DELETE FROM blob", [])?;
        tx.commit()?;
        orphans
    };

    for (_, relpath) in &orphans {
        blobs.remove(relpath)?;
    }
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;");

    Ok(ForgetReport {
        blobs_removed: orphans.len(),
        source_paths: Vec::new(),
    })
}

/// Blobs no longer referenced by any content row.
fn orphan_blobs(tx: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = tx.prepare(
        "SELECT id, storage_relpath FROM blob WHERE id NOT IN (
            SELECT patch_blob_id FROM file_event   WHERE patch_blob_id  IS NOT NULL
            UNION SELECT blob_id  FROM message_part WHERE blob_id        IS NOT NULL
            UNION SELECT input_blob_id  FROM tool_call WHERE input_blob_id  IS NOT NULL
            UNION SELECT output_blob_id FROM tool_call WHERE output_blob_id IS NOT NULL
            UNION SELECT diff_blob_id FROM git_observation WHERE diff_blob_id IS NOT NULL
         )",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn session_source_paths(conn: &Connection, session_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT sa.current_path FROM session_source ss
         JOIN source_artifact sa ON sa.id = ss.source_artifact_id
         WHERE ss.session_id = ?1",
    )?;
    let rows = stmt
        .query_map([session_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Outcome of forgetting the whole archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetEverythingReport {
    /// Lore-owned entries removed under the archive directory.
    pub removed: Vec<String>,
    /// A truthful note about copies Lore does not own and cannot delete.
    pub remaining_note: &'static str,
}

/// Remove all Lore-owned data under `archive_dir`: the database and its WAL/SHM
/// sidecars, blobs, backups, cache, content-bearing logs, and quarantine
/// artifacts. Exports (which the user chose to keep) and original agent logs
/// are left untouched. The caller must close DB connections first.
///
/// Note: secure physical erasure cannot be guaranteed on SSD/copy-on-write
/// filesystems; this removes the files, not necessarily every underlying block.
pub fn forget_everything(archive_dir: &Path) -> Result<ForgetEverythingReport> {
    const FILES: &[&str] = &["lore.db", "lore.db-wal", "lore.db-shm"];
    const DIRS: &[&str] = &["blobs", "backups", "cache", "logs", "quarantine"];
    let mut removed = Vec::new();

    for name in FILES {
        let path = archive_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|_| StorageError::Io)?;
            removed.push((*name).to_string());
        }
    }
    for name in DIRS {
        let path = archive_dir.join(name);
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|_| StorageError::Io)?;
            removed.push((*name).to_string());
        }
    }

    Ok(ForgetEverythingReport {
        removed,
        remaining_note:
            "Original agent logs and any exports you kept are outside Lore's ownership \
             and were not deleted. Secure block-level erasure is not guaranteed on SSDs.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::codex::CodexAdapter;
    use crate::ingest::persist_session;

    fn store() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = BlobStore::open(dir.path()).unwrap();
        (dir, s)
    }

    fn codex_patch(session: &str, content: &str) -> String {
        format!(
            concat!(
                "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"{id}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n",
                "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"function_call\",\"name\":\"apply_patch\",\"arguments\":\"{{}}\",\"call_id\":\"c1\"}}}}\n",
                "{{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:02.000Z\",\"payload\":{{\"type\":\"patch_apply_end\",\"call_id\":\"c1\",\"success\":true,\"changes\":{{\"f.ts\":{{\"type\":\"add\",\"content\":\"{content}\"}}}}}}}}\n"
            ),
            id = session,
            content = content
        )
    }

    fn persist(conn: &Connection, blobs: &BlobStore, session: &str, content: &str) -> String {
        let parsed = CodexAdapter::new().parse_str(&codex_patch(session, content), session);
        persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap()
    }

    #[test]
    fn forget_session_removes_rows_projections_and_orphan_blobs() {
        let conn = crate::storage::open_in_memory().unwrap();
        let (_bd, blobs) = store();
        let sid = persist(&conn, &blobs, "a", "const A = 1");

        // Find the blob path before forgetting.
        let relpath: String = conn
            .query_row("SELECT storage_relpath FROM blob", [], |r| r.get(0))
            .unwrap();
        assert!(blobs.read(&relpath).is_ok());

        let report = forget_session(&conn, &blobs, &sid).unwrap();
        assert_eq!(report.blobs_removed, 1);

        let counts: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT count(*) FROM agent_session),
                    (SELECT count(*) FROM search_document),
                    (SELECT count(*) FROM secret_finding),
                    (SELECT count(*) FROM blob)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0), "all owned rows gone");
        assert!(blobs.read(&relpath).is_err(), "blob file deleted");
    }

    #[test]
    fn forget_session_keeps_a_blob_shared_with_another_session() {
        let conn = crate::storage::open_in_memory().unwrap();
        let (_bd, blobs) = store();
        // Same patch content in two sessions → one content-addressed blob.
        let a = persist(&conn, &blobs, "a", "shared body");
        persist(&conn, &blobs, "b", "shared body");
        assert_eq!(
            conn.query_row("SELECT count(*) FROM blob", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );

        let report = forget_session(&conn, &blobs, &a).unwrap();
        assert_eq!(report.blobs_removed, 0, "shared blob is retained");
        assert_eq!(
            conn.query_row("SELECT count(*) FROM blob", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn forget_all_wipes_content_but_keeps_the_connection() {
        let conn = crate::storage::open_in_memory().unwrap();
        let (_bd, blobs) = store();
        persist(&conn, &blobs, "a", "const A = 1");
        persist(&conn, &blobs, "b", "const B = 2");

        let report = forget_all(&conn, &blobs).unwrap();
        assert_eq!(report.blobs_removed, 2);

        // Every content table is empty; the connection is still usable.
        for table in [
            "agent_session",
            "message",
            "search_document",
            "secret_finding",
            "blob",
            "repository",
            "source_artifact",
            "agent",
        ] {
            let n: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} must be empty after forget_all");
        }
    }

    #[test]
    fn forget_everything_removes_owned_data_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("lore.db"), b"db").unwrap();
        std::fs::write(root.join("lore.db-wal"), b"wal").unwrap();
        std::fs::create_dir(root.join("blobs")).unwrap();
        std::fs::write(root.join("blobs/x"), b"blob").unwrap();
        std::fs::create_dir(root.join("backups")).unwrap();
        std::fs::create_dir(root.join("quarantine")).unwrap();
        std::fs::write(root.join("quarantine/lore-corrupt.db"), b"preserved").unwrap();
        std::fs::create_dir(root.join("exports")).unwrap();
        std::fs::write(root.join("exports/keep.md"), b"user export").unwrap();

        let report = forget_everything(root).unwrap();
        assert!(report.removed.contains(&"lore.db".to_string()));
        assert!(report.removed.contains(&"blobs".to_string()));
        assert!(report.removed.contains(&"quarantine".to_string()));
        assert!(!root.join("lore.db").exists());
        assert!(!root.join("blobs").exists());
        assert!(!root.join("quarantine").exists());
        // User-owned exports are never deleted.
        assert!(root.join("exports/keep.md").exists());
        assert!(report.remaining_note.contains("outside Lore's ownership"));
    }
}
