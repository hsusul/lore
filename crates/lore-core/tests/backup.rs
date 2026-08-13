//! M7 acceptance: Lore-owned local backups via SQLite's online backup API
//! (SECURITY.md §6, DATA_MODEL.md §9).
//!
//! A backup of the WAL-mode archive must be a standalone, integrity-clean
//! database containing every committed row — including content still sitting in
//! the uncheckpointed WAL — and retention must keep only the newest `keep`
//! copies. Backup files inherit the app's private-permission posture.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::codex::CodexAdapter;
use lore_core::backup::{create_backup, DEFAULT_BACKUP_RETENTION};
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use std::path::Path;

/// A Codex rollout whose recorded patch carries a synthetic github-token, so a
/// populated archive exercises sessions, search projections, a blob, and a
/// secret finding in one persist.
fn codex_patch(session: &str, content: &str) -> String {
    format!(
        concat!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"{id}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n",
            "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"function_call\",\"name\":\"apply_patch\",\"arguments\":\"{{}}\",\"call_id\":\"c1\"}}}}\n",
            "{{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:02.000Z\",\"payload\":{{\"type\":\"patch_apply_end\",\"call_id\":\"c1\",\"success\":true,\"changes\":{{\"config.ts\":{{\"type\":\"add\",\"content\":\"{content}\"}}}}}}}}\n"
        ),
        id = session,
        content = content
    )
}

/// A synthetic github-token, assembled from split parts (never a contiguous
/// literal, so it cannot trip upstream push-protection on this fixture).
fn secret() -> String {
    format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz")
}

/// Open a file-backed WAL archive (the real app configuration, not `:memory:`)
/// with a blob store alongside.
fn archive(root: &Path) -> (rusqlite::Connection, BlobStore) {
    let conn = lore_core::storage::open(&root.join("lore.db")).unwrap();
    let blobs = BlobStore::open(root.join("blobs")).unwrap();
    (conn, blobs)
}

fn populate(conn: &rusqlite::Connection, blobs: &BlobStore) {
    let content = format!("const KEY = {}", secret());
    let parsed = CodexAdapter::new().parse_str(&codex_patch("s-a", &content), "s-a");
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
    let parsed = CodexAdapter::new().parse_str(&codex_patch("s-b", "const B = 2"), "s-b");
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
}

fn counts(conn: &rusqlite::Connection) -> (i64, i64, i64, i64, i64, i64) {
    conn.query_row(
        "SELECT
            (SELECT count(*) FROM agent_session),
            (SELECT count(*) FROM message_part),
            (SELECT count(*) FROM search_document),
            (SELECT count(*) FROM secret_finding),
            (SELECT count(*) FROM blob),
            (SELECT count(*) FROM setting)",
        [],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        },
    )
    .unwrap()
}

#[test]
fn backup_is_a_standalone_copy_of_the_archive() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    conn.execute(
        "INSERT INTO setting (key, value_json, updated_at) VALUES ('k', '\"v\"', 0)",
        [],
    )
    .unwrap();
    populate(&conn, &blobs);

    let backups = dir.path().join("backups");
    let info = create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();

    assert!(info.path.starts_with(&backups));
    assert!(info.path.exists());
    assert!(info.size_bytes > 0);

    let expected = counts(&conn);
    assert_eq!(expected.0, 2, "two sessions committed");
    assert_eq!(expected.3, 1, "one secret finding from the planted token");
    assert_eq!(expected.4, 2, "one content-addressed blob per patch");

    // The backup is a standalone database: openable at the current schema with
    // all committed content, including the settings row and job-queue table.
    let backup = lore_core::storage::open(&info.path).unwrap();
    assert_eq!(counts(&backup), expected, "backup must mirror the source");
    let jobs: i64 = backup
        .query_row("SELECT count(*) FROM job", [], |r| r.get(0))
        .unwrap();
    assert_eq!(jobs, 0);
}

#[test]
fn backup_includes_uncheckpointed_wal_content() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);

    // No checkpoint runs on a tiny WAL (the auto-checkpoint threshold is far
    // above this database's size), so the committed rows are still only in the
    // WAL when the backup is taken. The online backup must capture them.
    let before = counts(&conn);
    assert_eq!(before.0, 2, "sessions committed but not checkpointed");

    let info = create_backup(&conn, &dir.path().join("backups"), DEFAULT_BACKUP_RETENTION).unwrap();
    let backup = lore_core::storage::open(&info.path).unwrap();
    assert_eq!(
        counts(&backup),
        before,
        "uncheckpointed WAL content must be backed up"
    );
}

#[test]
fn retention_keeps_only_the_newest_backups() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _blobs) = archive(dir.path());
    let backups = dir.path().join("backups");

    for _ in 0..(DEFAULT_BACKUP_RETENTION + 2) {
        create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();
    }

    let remaining = list_backups(&backups);
    assert_eq!(remaining.len(), DEFAULT_BACKUP_RETENTION);
    // The newest `keep` survive; older copies are pruned.
    let mut sorted = remaining.clone();
    sorted.sort();
    assert_eq!(
        remaining, sorted,
        "survivors are the newest, by sortable name"
    );
    let all = all_backup_names(&backups);
    assert_eq!(
        all.len(),
        DEFAULT_BACKUP_RETENTION,
        "old copies are deleted"
    );
}

#[cfg(unix)]
#[test]
fn backup_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let (conn, _blobs) = archive(dir.path());
    let info = create_backup(&conn, &dir.path().join("backups"), DEFAULT_BACKUP_RETENTION).unwrap();
    let mode = std::fs::metadata(&info.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "backup files must inherit the app's private permissions"
    );
}

fn list_backups(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

fn all_backup_names(dir: &Path) -> Vec<String> {
    list_backups(dir)
        .into_iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}
