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
        let _write = crate::storage::write_lock();
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
        let _write = crate::storage::write_lock();
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

/// The one user-owned subtree under the archive directory. Everything else
/// there is Lore's (`DATA_MODEL.md` §9) and is swept by `forget_everything`.
const EXPORTS_DIR: &str = "exports";

const REMAINING_NOTE: &str =
    "Original agent logs and any exports you kept are outside Lore's ownership \
     and were not deleted. Secure block-level erasure is not guaranteed on SSDs.";

/// Outcome of forgetting the whole archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetEverythingReport {
    /// Lore-owned entries removed under the archive directory (top-level names).
    pub removed: Vec<String>,
    /// User-owned exports left in place (top-level names under `exports/`), named
    /// so the UI can disclose exactly what still exists after "forget everything".
    pub preserved_exports: Vec<String>,
    /// A truthful note about copies Lore does not own and cannot delete.
    pub remaining_note: &'static str,
}

/// Remove **all** Lore-owned data under `archive_dir` — the database and its
/// WAL/SHM/journal sidecars, blobs, backups, cache, content-bearing logs,
/// quarantine artifacts, and anything else Lore wrote there. The user-owned
/// `exports/` subtree and original agent logs are left untouched. The caller
/// must close DB connections first.
///
/// The sweep is a **whitelist**, not a name list: it preserves `exports/` and
/// removes every other top-level entry. This is exhaustive by construction, so a
/// sidecar Lore adds later (or a stray `lore.db-journal`, a temp file, a
/// `.DS_Store`) cannot silently survive the way a hardcoded blocklist would.
///
/// Note: secure physical erasure cannot be guaranteed on SSD/copy-on-write
/// filesystems; this removes the files, not necessarily every underlying block.
pub fn forget_everything(archive_dir: &Path) -> Result<ForgetEverythingReport> {
    let mut removed = Vec::new();
    let entries = match std::fs::read_dir(archive_dir) {
        Ok(entries) => entries,
        // No archive directory means there is nothing Lore-owned to remove.
        Err(_) => {
            return Ok(ForgetEverythingReport {
                removed,
                preserved_exports: Vec::new(),
                remaining_note: REMAINING_NOTE,
            })
        }
    };

    for entry in entries {
        let entry = entry.map_err(|_| StorageError::Io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == EXPORTS_DIR {
            continue; // user-owned; never deleted
        }
        let path = entry.path();
        if entry.file_type().map_err(|_| StorageError::Io)?.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|_| StorageError::Io)?;
        } else {
            std::fs::remove_file(&path).map_err(|_| StorageError::Io)?;
        }
        removed.push(name);
    }
    removed.sort();

    Ok(ForgetEverythingReport {
        removed,
        preserved_exports: list_exports(archive_dir),
        remaining_note: REMAINING_NOTE,
    })
}

/// Content-bearing, recoverable Lore-owned stores that a full in-app "forget
/// everything" must also clear so nothing survives to be restored: `backups/`
/// hold whole-database copies, `cache/` holds rendered/search content, and
/// `quarantine/` holds preserved corrupt archives. `logs/` is intentionally
/// excluded — it is content-free by design (`DATA_MODEL.md` §9) and may be held
/// open by the running process.
const RECOVERABLE_DIRS: &[&str] = &["backups", "cache", "quarantine"];

/// Remove the on-disk stores from which just-forgotten data could otherwise be
/// recovered. This complements [`forget_all`], which wipes only the live database
/// rows and blobs: without this, a whole-database copy under `backups/` still
/// holds everything the user asked to forget (and `restore_backup` would bring it
/// back). Returns the directory names actually removed. The live `lore.db` and
/// `blobs/` are left to `forget_all`; user-owned `exports/` is never touched.
pub fn purge_recoverable_copies(archive_dir: &Path) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for name in RECOVERABLE_DIRS {
        let path = archive_dir.join(name);
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|_| StorageError::Io)?;
            removed.push((*name).to_string());
        }
    }
    Ok(removed)
}

/// Top-level entry names under `archive_dir/exports`, sorted; empty if absent.
fn list_exports(archive_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(archive_dir.join(EXPORTS_DIR)) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
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
        // User-owned exports are never deleted, and are disclosed by name.
        assert!(root.join("exports/keep.md").exists());
        assert_eq!(report.preserved_exports, vec!["keep.md".to_string()]);
        assert!(report.remaining_note.contains("outside Lore's ownership"));
    }

    /// The deletion-sweep audit: entries a hardcoded name list would miss — a
    /// stray `lore.db-journal`, a future/unknown sidecar dir, an OS `.DS_Store` —
    /// must still be swept. Only the user-owned `exports/` subtree survives.
    #[test]
    fn forget_everything_sweeps_unlisted_owned_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The documented owned set…
        for f in ["lore.db", "lore.db-wal", "lore.db-shm", "lore.db-journal"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        for d in ["blobs", "backups", "cache", "logs", "quarantine"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        // …plus entries no blocklist enumerates.
        std::fs::write(root.join(".DS_Store"), b"os").unwrap();
        std::fs::write(root.join("lore.db.tmp-write"), b"partial").unwrap();
        std::fs::create_dir(root.join("index-next")).unwrap();
        std::fs::write(root.join("index-next/segments"), b"future").unwrap();
        // …and a user export that must survive.
        std::fs::create_dir(root.join("exports")).unwrap();
        std::fs::write(root.join("exports/session.md"), b"kept").unwrap();

        let report = forget_everything(root).unwrap();

        // Everything Lore-owned is gone; only exports/ remains under the archive.
        let remaining: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            remaining,
            vec!["exports".to_string()],
            "only exports survives"
        );
        for unlisted in [
            ".DS_Store",
            "lore.db-journal",
            "lore.db.tmp-write",
            "index-next",
        ] {
            assert!(
                report.removed.contains(&unlisted.to_string()),
                "unlisted owned entry {unlisted} must be swept and reported"
            );
        }
        assert!(root.join("exports/session.md").exists());
        assert_eq!(report.preserved_exports, vec!["session.md".to_string()]);
    }

    #[test]
    fn purge_recoverable_copies_clears_backups_but_keeps_db_and_exports() {
        // Regression: an in-app "forget everything" wiped DB rows and blobs but
        // left whole-DB copies under backups/, so the forgotten data stayed
        // recoverable. purge_recoverable_copies closes that hole.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("lore.db"), b"live-empty-db").unwrap();
        std::fs::create_dir(root.join("blobs")).unwrap();
        for d in ["backups", "cache", "quarantine", "logs"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        std::fs::write(root.join("backups/lore-20260813.db"), b"old data").unwrap();
        std::fs::create_dir(root.join("exports")).unwrap();
        std::fs::write(root.join("exports/keep.md"), b"user export").unwrap();

        let mut removed = purge_recoverable_copies(root).unwrap();
        removed.sort();
        assert_eq!(removed, vec!["backups", "cache", "quarantine"]);
        // Recoverable copies are gone…
        assert!(!root.join("backups").exists());
        assert!(!root.join("cache").exists());
        assert!(!root.join("quarantine").exists());
        // …but the live DB, blobs, content-free logs, and user exports remain.
        assert!(root.join("lore.db").exists());
        assert!(root.join("blobs").exists());
        assert!(root.join("logs").exists());
        assert!(root.join("exports/keep.md").exists());
    }

    #[test]
    fn forget_everything_is_a_noop_on_a_missing_archive_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let report = forget_everything(&missing).unwrap();
        assert!(report.removed.is_empty());
        assert!(report.preserved_exports.is_empty());
    }

    #[test]
    fn forget_session_with_nonexistent_id_returns_empty_report() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let report = forget_session(&conn, &blobs, "nonexistent-session-id").unwrap();
        assert_eq!(report.blobs_removed, 0);
        assert!(report.source_paths.is_empty());
    }

    #[test]
    fn forget_all_on_empty_database_and_report_clones() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let report = forget_all(&conn, &blobs).unwrap();
        assert_eq!(report.blobs_removed, 0);
        assert!(report.source_paths.is_empty());

        let report_clone = report.clone();
        assert_eq!(report, report_clone);

        let everything = ForgetEverythingReport {
            removed: vec!["blobs".to_string()],
            preserved_exports: vec!["summary.md".to_string()],
            remaining_note: "note",
        };
        assert_eq!(everything.clone(), everything);
    }

    #[test]
    fn purge_recoverable_copies_on_nonexistent_and_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist");
        let removed1 = purge_recoverable_copies(&nonexistent).unwrap();
        assert!(removed1.is_empty());

        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let removed2 = purge_recoverable_copies(&empty).unwrap();
        assert!(removed2.is_empty());
    }

    #[test]
    fn forget_everything_on_nonexistent_and_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist");
        let rep1 = forget_everything(&nonexistent).unwrap();
        assert!(rep1.removed.is_empty());
        assert!(rep1.preserved_exports.is_empty());

        let empty = dir.path().join("empty_dir");
        std::fs::create_dir_all(&empty).unwrap();
        let rep2 = forget_everything(&empty).unwrap();
        assert!(rep2.removed.is_empty());
        assert!(rep2.preserved_exports.is_empty());
    }
}
