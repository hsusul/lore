//! User-defined folders for organizing threads (sessions).
//!
//! Folders are Lore-owned organizational metadata in `lore.db`: created by the
//! user, preserved when archived content is cleared, and never derived from
//! agent files. Membership is mutually exclusive — one folder per thread — so
//! filing a thread into a folder replaces any prior membership.
//!
//! The paged listing of a folder's threads lives in [`crate::query`] so it can
//! reuse the shared session-summary projection and keyset cursor.

use lore_ipc::FolderSummary;
use rusqlite::{Connection, OptionalExtension};

use crate::storage::Result;

/// Longest accepted folder name; longer input is truncated on a char boundary.
const MAX_NAME_LEN: usize = 100;

/// Normalize a user-supplied folder name: trim surrounding whitespace, cap the
/// length, and fall back to a sensible default when empty. Never fails so the
/// UI can stay optimistic.
fn clean_name(name: &str) -> String {
    let words = name
        .split(|c: char| c.is_whitespace() || c.is_control() || crate::is_zero_width(c))
        .filter(|w| !w.is_empty());
    let mut single_line = String::with_capacity(name.len().min(MAX_NAME_LEN));
    let mut count = 0;
    for word in words {
        if count > 0 {
            if count >= MAX_NAME_LEN {
                break;
            }
            single_line.push(' ');
            count += 1;
        }
        for c in word.chars().filter(|c| !crate::is_zero_width(*c)) {
            if count >= MAX_NAME_LEN {
                break;
            }
            single_line.push(c);
            count += 1;
        }
    }
    let trimmed = single_line.trim();
    if trimmed.is_empty() {
        "New folder".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Every folder with its thread count, ordered by the user position then name
/// for a stable list.
pub fn list_folders(conn: &Connection) -> Result<Vec<FolderSummary>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.name, f.position,
                (SELECT count(*) FROM session_folder sf WHERE sf.folder_id = f.id)
         FROM folder f
         ORDER BY f.position, f.name, f.id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(FolderSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                position: row.get(2)?,
                session_count: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Create a folder, appended after the current last one. The id is a random
/// 128-bit hex string minted by SQLite; the cleaned name is returned so the UI
/// reflects any normalization.
pub fn create_folder(conn: &Connection, name: &str) -> Result<FolderSummary> {
    let name = clean_name(name);
    let _write = crate::storage::write_lock();
    let position: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM folder",
        [],
        |r| r.get(0),
    )?;
    let id: String = conn.query_row(
        "INSERT INTO folder (id, name, position, created_at, updated_at)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, unixepoch('now') * 1000,
                 unixepoch('now') * 1000)
         RETURNING id",
        rusqlite::params![name, position],
        |r| r.get(0),
    )?;
    Ok(FolderSummary {
        id,
        name,
        session_count: 0,
        position,
    })
}

/// Rename a folder. Unknown ids are a no-op.
pub fn rename_folder(conn: &Connection, id: &str, name: &str) -> Result<()> {
    let _write = crate::storage::write_lock();
    conn.execute(
        "UPDATE folder SET name = ?2, updated_at = unixepoch('now') * 1000 WHERE id = ?1",
        rusqlite::params![id, clean_name(name)],
    )?;
    Ok(())
}

/// Delete a folder. Its memberships cascade away; the threads themselves are
/// untouched and simply become unfiled. Unknown ids are a no-op.
pub fn delete_folder(conn: &Connection, id: &str) -> Result<()> {
    let _write = crate::storage::write_lock();
    conn.execute("DELETE FROM folder WHERE id = ?1", [id])?;
    Ok(())
}

/// File a thread into a folder, replacing any prior membership. `folder_id` of
/// `None` unfiles the thread. Errors if the folder or session id is unknown
/// (foreign-key enforced).
pub fn set_session_folder(
    conn: &Connection,
    session_id: &str,
    folder_id: Option<&str>,
) -> Result<()> {
    let _write = crate::storage::write_lock();
    match folder_id {
        Some(folder_id) => {
            conn.execute(
                "INSERT INTO session_folder (session_id, folder_id, added_at)
                 VALUES (?1, ?2, unixepoch('now') * 1000)
                 ON CONFLICT(session_id) DO UPDATE SET
                     folder_id = excluded.folder_id,
                     added_at  = excluded.added_at",
                rusqlite::params![session_id, folder_id],
            )?;
        }
        None => {
            conn.execute(
                "DELETE FROM session_folder WHERE session_id = ?1",
                [session_id],
            )?;
        }
    }
    Ok(())
}

/// The folder a thread is filed in, or `None` when it is unfiled.
pub fn folder_of_session(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT folder_id FROM session_folder WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::claude_code::ClaudeCodeAdapter;
    use crate::ingest::persist_session;
    use crate::storage::blob::BlobStore;

    /// Persist a single Claude session and return its id.
    fn seed_session(conn: &Connection, native: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let content = format!(
            "{{\"type\":\"user\",\"uuid\":\"u-{native}\",\"sessionId\":\"{native}\",\"cwd\":\"/p\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n"
        );
        let parsed = ClaudeCodeAdapter::new().parse_str(&content, native);
        persist_session(conn, "claude-code", "Claude Code", &parsed, &blobs).unwrap()
    }

    #[test]
    fn create_list_and_rename_round_trip() {
        let conn = crate::storage::open_in_memory().unwrap();
        assert!(list_folders(&conn).unwrap().is_empty());

        let a = create_folder(&conn, "  Inbox  ").unwrap();
        assert_eq!(a.name, "Inbox", "name is trimmed");
        assert_eq!(a.position, 0);
        let b = create_folder(&conn, "Later").unwrap();
        assert_eq!(b.position, 1, "folders append after the last position");

        rename_folder(&conn, &a.id, "  Triage  ").unwrap();
        let folders = list_folders(&conn).unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].name, "Triage");
        assert_eq!(folders[0].session_count, 0);
    }

    #[test]
    fn an_empty_name_falls_back_to_a_default() {
        let conn = crate::storage::open_in_memory().unwrap();
        let f = create_folder(&conn, "   \n\t  ").unwrap();
        assert_eq!(f.name, "New folder");

        let multiline = create_folder(&conn, "  Project   Alpha  \n  V2  ").unwrap();
        assert_eq!(multiline.name, "Project Alpha V2");

        let with_controls = create_folder(&conn, "  Secret\0\x07Folder  ").unwrap();
        assert_eq!(with_controls.name, "Secret Folder");

        let with_bom = create_folder(&conn, "\u{feff}   \u{feff}").unwrap();
        assert_eq!(with_bom.name, "New folder");

        let with_zero_width = create_folder(&conn, "\u{200b}\u{200c}\u{200d}\u{2060}").unwrap();
        assert_eq!(with_zero_width.name, "New folder");

        let multiline = create_folder(&conn, "  Folder\r\n\tWith\nSpaces  ").unwrap();
        assert_eq!(multiline.name, "Folder With Spaces");

        let long = create_folder(&conn, &"word ".repeat(30)).unwrap();
        assert!(long.name.chars().count() <= MAX_NAME_LEN);
        assert!(!long.name.ends_with(' '));
    }

    #[test]
    fn filing_a_thread_is_mutually_exclusive() {
        let conn = crate::storage::open_in_memory().unwrap();
        let sid = seed_session(&conn, "s1");
        let a = create_folder(&conn, "A").unwrap();
        let b = create_folder(&conn, "B").unwrap();

        set_session_folder(&conn, &sid, Some(&a.id)).unwrap();
        assert_eq!(
            folder_of_session(&conn, &sid).unwrap().as_deref(),
            Some(a.id.as_str())
        );
        assert_eq!(list_folders(&conn).unwrap()[0].session_count, 1);

        // Re-filing replaces the prior membership rather than duplicating it.
        set_session_folder(&conn, &sid, Some(&b.id)).unwrap();
        assert_eq!(
            folder_of_session(&conn, &sid).unwrap().as_deref(),
            Some(b.id.as_str())
        );
        let counts: Vec<i64> = list_folders(&conn)
            .unwrap()
            .iter()
            .map(|f| f.session_count)
            .collect();
        assert_eq!(counts, vec![0, 1], "the thread moved, it was not copied");

        // Unfiling removes membership entirely.
        set_session_folder(&conn, &sid, None).unwrap();
        assert!(folder_of_session(&conn, &sid).unwrap().is_none());
    }

    #[test]
    fn deleting_a_folder_unfiles_its_threads_without_removing_them() {
        let conn = crate::storage::open_in_memory().unwrap();
        let sid = seed_session(&conn, "s1");
        let a = create_folder(&conn, "A").unwrap();
        set_session_folder(&conn, &sid, Some(&a.id)).unwrap();

        delete_folder(&conn, &a.id).unwrap();
        assert!(list_folders(&conn).unwrap().is_empty());
        assert!(folder_of_session(&conn, &sid).unwrap().is_none());
        // The session itself survives.
        assert_eq!(crate::query::list_sessions(&conn, 10).unwrap().len(), 1);
    }

    #[test]
    fn forgetting_a_thread_removes_its_membership() {
        let conn = crate::storage::open_in_memory().unwrap();
        let sid = seed_session(&conn, "s1");
        let a = create_folder(&conn, "A").unwrap();
        set_session_folder(&conn, &sid, Some(&a.id)).unwrap();

        conn.execute("DELETE FROM agent_session WHERE id = ?1", [&sid])
            .unwrap();
        assert_eq!(list_folders(&conn).unwrap()[0].session_count, 0);
    }

    #[test]
    fn filing_into_an_unknown_folder_is_rejected() {
        let conn = crate::storage::open_in_memory().unwrap();
        let sid = seed_session(&conn, "s1");
        assert!(set_session_folder(&conn, &sid, Some("nope")).is_err());
    }

    #[test]
    fn filing_an_unknown_session_is_rejected() {
        let conn = crate::storage::open_in_memory().unwrap();
        let a = create_folder(&conn, "A").unwrap();
        assert!(set_session_folder(&conn, "nonexistent-session", Some(&a.id)).is_err());
    }

    #[test]
    fn rename_and_delete_nonexistent_folder_are_safe_noops() {
        let conn = crate::storage::open_in_memory().unwrap();
        assert!(rename_folder(&conn, "nonexistent", "New Name").is_ok());
        assert!(delete_folder(&conn, "nonexistent").is_ok());
    }

    #[test]
    fn unfiling_an_already_unfiled_or_unknown_session_is_safe_noop() {
        let conn = crate::storage::open_in_memory().unwrap();
        let sid = seed_session(&conn, "s1");
        // Unfiling a session that was never in a folder succeeds
        assert!(set_session_folder(&conn, &sid, None).is_ok());
        // Unfiling a nonexistent session ID also succeeds safely
        assert!(set_session_folder(&conn, "nonexistent-session", None).is_ok());
    }

    #[test]
    fn rename_folder_with_empty_or_whitespace_name_falls_back_to_default() {
        let conn = crate::storage::open_in_memory().unwrap();
        let f = create_folder(&conn, "Original Name").unwrap();
        assert_eq!(f.name, "Original Name");

        // Rename with whitespace falls back to "New folder"
        rename_folder(&conn, &f.id, "   \t\n  ").unwrap();
        let folders = list_folders(&conn).unwrap();
        assert_eq!(folders[0].name, "New folder");

        // Rename with control characters strips them
        rename_folder(&conn, &f.id, "\0Renamed\x07Folder\0").unwrap();
        let folders = list_folders(&conn).unwrap();
        assert_eq!(folders[0].name, "Renamed Folder");
    }

    #[test]
    fn create_and_rename_folder_with_unicode_and_punctuation() {
        let conn = crate::storage::open_in_memory().unwrap();
        let f = create_folder(&conn, "📁 Projects · 2026 🚀").unwrap();
        assert_eq!(f.name, "📁 Projects · 2026 🚀");

        rename_folder(&conn, &f.id, "★ Starred (ACME / 開発) ★").unwrap();
        let folders = list_folders(&conn).unwrap();
        assert_eq!(folders[0].name, "★ Starred (ACME / 開発) ★");
    }

    #[test]
    fn set_session_folder_is_idempotent_and_updates_membership() {
        let conn = crate::storage::open_in_memory().unwrap();
        let sid = seed_session(&conn, "s1");
        let f1 = create_folder(&conn, "Folder 1").unwrap();
        let f2 = create_folder(&conn, "Folder 2").unwrap();

        // Add to folder 1
        set_session_folder(&conn, &sid, Some(&f1.id)).unwrap();
        assert_eq!(
            folder_of_session(&conn, &sid).unwrap().as_deref(),
            Some(f1.id.as_str())
        );

        // Repeated addition to folder 1 is idempotent
        set_session_folder(&conn, &sid, Some(&f1.id)).unwrap();
        assert_eq!(
            folder_of_session(&conn, &sid).unwrap().as_deref(),
            Some(f1.id.as_str())
        );

        // Move to folder 2 updates membership without duplicate rows
        set_session_folder(&conn, &sid, Some(&f2.id)).unwrap();
        assert_eq!(
            folder_of_session(&conn, &sid).unwrap().as_deref(),
            Some(f2.id.as_str())
        );

        let rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM session_folder WHERE session_id = ?1",
                [&sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
    }
}
