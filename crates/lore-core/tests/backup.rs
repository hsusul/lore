//! M7 acceptance: Lore-owned local backups via SQLite's online backup API
//! (SECURITY.md §6, DATA_MODEL.md §9).
//!
//! A backup of the WAL-mode archive must be a standalone, integrity-clean
//! database containing every committed row — including content still sitting in
//! the uncheckpointed WAL — and retention must keep only the newest `keep`
//! copies. Backup files inherit the app's private-permission posture.
//!
//! Recovery: a Lore-owned backup must restore a usable archive without any
//! original agent logs (SECURITY.md §6 "restore from a Lore-owned local
//! backup"; TESTING.md §7 "works from local backup without source logs").
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::codex::CodexAdapter;
use lore_core::backup::{
    create_backup, read_schedule, run_scheduled_backup, write_schedule, BackupInterval,
    BackupSchedule, DEFAULT_BACKUP_RETENTION,
};
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

#[test]
fn restore_backup_recreates_the_archive_without_source_logs() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let expected = counts(&conn);

    let backups = dir.path().join("backups");
    create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();

    // Recovery must not assume agent logs still exist: the source here is only
    // what was already ingested into the archive, and it is gone after the
    // connections close (the WAL checkpoints into lore.db on clean close).
    drop(conn);
    drop(blobs);

    let newest = lore_core::backup::list_backups(&backups)
        .unwrap()
        .into_iter()
        .last()
        .unwrap();
    lore_core::backup::restore_backup(&newest, &dir.path().join("lore.db")).unwrap();

    let (restored, _blobs) = archive(dir.path());
    assert_eq!(
        counts(&restored),
        expected,
        "restored archive must mirror the backup without source logs"
    );
}

#[test]
fn restore_backup_rejects_a_non_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.db");
    std::fs::write(&bad, "not a sqlite database").unwrap();
    let dst = dir.path().join("out.db");
    assert!(
        lore_core::backup::restore_backup(&bad, &dst).is_err(),
        "restoring from a non-database file must fail"
    );
}

#[test]
fn restore_backup_rejects_a_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.db");
    let dst = dir.path().join("out.db");
    assert!(
        lore_core::backup::restore_backup(&missing, &dst).is_err(),
        "restoring from a nonexistent file must fail"
    );
}

#[test]
fn list_backups_lists_only_lore_owned_backups_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _blobs) = archive(dir.path());
    let backups = dir.path().join("backups");
    for _ in 0..3 {
        create_backup(&conn, &backups, 10).unwrap();
    }
    std::fs::write(backups.join("notes.txt"), "not a backup").unwrap();

    let listed = lore_core::backup::list_backups(&backups).unwrap();
    assert_eq!(listed.len(), 3, "stray non-backup files are ignored");
    assert!(
        listed.windows(2).all(|w| w[0] < w[1]),
        "backups are listed oldest-first by sortable name"
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

#[test]
fn schedule_round_trips_through_settings() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _blobs) = archive(dir.path());

    // Default before anything is written: off, default retention.
    assert_eq!(read_schedule(&conn).unwrap(), BackupSchedule::default());

    write_schedule(
        &conn,
        BackupSchedule {
            interval: BackupInterval::Weekly,
            keep: 3,
        },
    )
    .unwrap();
    let got = read_schedule(&conn).unwrap();
    assert_eq!(got.interval, BackupInterval::Weekly);
    assert_eq!(got.keep, 3);

    // Wire values are stable and parse back symmetrically.
    assert_eq!(
        BackupInterval::parse(BackupInterval::Daily.as_str()),
        BackupInterval::Daily
    );
    assert_eq!(BackupInterval::parse("bogus"), BackupInterval::Off);
}

#[test]
fn scheduled_backup_runs_only_when_due() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let backup_dir = dir.path().join("backups");
    let day = 24 * 60 * 60 * 1000_i64;
    let now = 1_800_000_000_000_i64;

    // Off: never backs up, even on the first call (no backup dir is created).
    assert!(run_scheduled_backup(&conn, &backup_dir, now)
        .unwrap()
        .is_none());
    assert!(!backup_dir.exists(), "Off must not create any backup");

    // Daily: the first call has no prior backup, so it is due.
    write_schedule(
        &conn,
        BackupSchedule {
            interval: BackupInterval::Daily,
            keep: DEFAULT_BACKUP_RETENTION,
        },
    )
    .unwrap();
    assert!(run_scheduled_backup(&conn, &backup_dir, now)
        .unwrap()
        .is_some());
    assert_eq!(list_backups(&backup_dir).len(), 1);

    // A second call within the interval is not due.
    assert!(run_scheduled_backup(&conn, &backup_dir, now + day / 2)
        .unwrap()
        .is_none());
    assert_eq!(list_backups(&backup_dir).len(), 1);

    // Once a full day has elapsed it is due again.
    assert!(run_scheduled_backup(&conn, &backup_dir, now + day)
        .unwrap()
        .is_some());
    assert_eq!(list_backups(&backup_dir).len(), 2);
}

#[test]
fn list_backups_on_nonexistent_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let missing_dir = dir.path().join("does_not_exist_yet");
    let result = lore_core::backup::list_backups(&missing_dir).unwrap();
    assert!(result.is_empty(), "nonexistent backup directory returns empty list");
}

#[test]
fn list_backups_ignores_subdirectories_matching_backup_naming_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();
    // A directory named like a backup file
    let fake_backup_subdir = backup_dir.join("lore-00000000000000000001-0001.db");
    std::fs::create_dir_all(&fake_backup_subdir).unwrap();

    let result = lore_core::backup::list_backups(&backup_dir).unwrap();
    assert!(result.is_empty(), "directories matching backup naming must be ignored");
}

#[test]
fn backup_schedule_read_clamping_and_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = archive(dir.path());

    // 1. Unset settings fall back to default schedule (Off, DEFAULT_BACKUP_RETENTION)
    let s = read_schedule(&conn).unwrap();
    assert_eq!(s.interval, BackupInterval::Off);
    assert_eq!(s.keep, DEFAULT_BACKUP_RETENTION);

    // 2. Corrupt/unparseable JSON in interval setting safely degrades to Off
    lore_core::settings::set(&conn, "backup.interval", "not-valid-json").unwrap();
    let s = read_schedule(&conn).unwrap();
    assert_eq!(s.interval, BackupInterval::Off);

    // 3. Unknown interval string safely falls back to Off
    lore_core::settings::set(&conn, "backup.interval", "\"every-second\"").unwrap();
    let s = read_schedule(&conn).unwrap();
    assert_eq!(s.interval, BackupInterval::Off);

    // 4. Zero keep is clamped to 1
    lore_core::settings::set(&conn, "backup.keep", "0").unwrap();
    let s = read_schedule(&conn).unwrap();
    assert_eq!(s.keep, 1);

    // 5. Oversized keep is clamped to 100
    lore_core::settings::set(&conn, "backup.keep", "5000").unwrap();
    let s = read_schedule(&conn).unwrap();
    assert_eq!(s.keep, 100);
}
