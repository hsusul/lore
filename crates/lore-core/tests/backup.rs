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

    // The backup is a zstd stream that decompresses to a standalone database:
    // openable at the current schema with all committed content, including
    // the settings row and job-queue table.
    assert!(info.path.to_string_lossy().ends_with(".db.zst"));
    let head = std::fs::read(&info.path).unwrap();
    assert_eq!(&head[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd magic");
    let scratch = tempfile::tempdir().unwrap();
    let backup = open_backup(&info.path, scratch.path());
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
    let scratch = tempfile::tempdir().unwrap();
    let backup = open_backup(&info.path, scratch.path());
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

/// Restore the backup at `backup` into a fresh file under `dir`, the way
/// recovery does, run `integrity_check` on it and open it.
fn open_backup(backup: &Path, dir: &Path) -> rusqlite::Connection {
    let copy = dir.join("restored.db");
    lore_core::backup::restore_backup(backup, &copy).unwrap();
    let conn = lore_core::storage::open(&copy).unwrap();
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    conn
}

#[test]
fn backup_leaves_only_the_compressed_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let backups = dir.path().join("backups");
    let info = create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();
    assert_eq!(
        list_backups(&backups),
        vec![info.path.clone()],
        "no temporary uncompressed copy is left"
    );
    let db_size: u64 = conn
        .query_row(
            "SELECT page_count * page_size FROM pragma_page_count, pragma_page_size",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(info.size_bytes < db_size, "compressed is smaller");
}

#[test]
fn retention_keeps_the_newest_across_plain_and_compressed_backups() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _blobs) = archive(dir.path());
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    // Plain backups an earlier build wrote, older than anything taken now.
    let old: Vec<_> = (1..=3)
        .map(|i| {
            let p = backups.join(format!("lore-{i:020}-0000.db"));
            std::fs::write(&p, b"old").unwrap();
            p
        })
        .collect();

    let a = create_backup(&conn, &backups, 4).unwrap();
    assert_eq!(
        lore_core::backup::list_backups(&backups).unwrap(),
        vec![
            old[0].clone(),
            old[1].clone(),
            old[2].clone(),
            a.path.clone()
        ],
        "both kinds are listed, oldest first"
    );
    let b = create_backup(&conn, &backups, 4).unwrap();
    let c = create_backup(&conn, &backups, 4).unwrap();
    assert_eq!(
        lore_core::backup::list_backups(&backups).unwrap(),
        vec![old[2].clone(), a.path, b.path, c.path],
        "the oldest plain backups aged out first"
    );
}

#[test]
fn restore_accepts_a_plain_backup_from_an_earlier_build() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let expected = counts(&conn);
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    let plain = backups.join("lore-00000000000000000001-0000.db");
    conn.execute("VACUUM INTO ?1", [plain.to_str().unwrap()])
        .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    assert_eq!(counts(&open_backup(&plain, scratch.path())), expected);
}

#[test]
fn a_truncated_compressed_backup_is_reported_broken_and_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let backups = dir.path().join("backups");
    let info = create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();
    let bytes = std::fs::read(&info.path).unwrap();

    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    for cut in [bytes.len() - 1, bytes.len() / 2, 8, 0] {
        let broken = backups.join("lore-99999999999999999999-0000.db.zst");
        std::fs::write(&broken, &bytes[..cut]).unwrap();
        let dst = out.join("lore.db");
        let err = lore_core::backup::restore_backup(&broken, &dst).unwrap_err();
        assert!(
            matches!(err, lore_core::backup::BackupError::Broken),
            "cut at {cut}: {err}"
        );
        assert!(!dst.exists(), "destination untouched");
        assert_eq!(
            std::fs::read_dir(&out).unwrap().count(),
            0,
            "no temporary file left"
        );
    }

    // A flipped byte fails the content checksum or the decode.
    let mut flipped = bytes.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xFF;
    let broken = backups.join("lore-99999999999999999999-0000.db.zst");
    std::fs::write(&broken, &flipped).unwrap();
    assert!(lore_core::backup::restore_backup(&broken, &out.join("lore.db")).is_err());

    // Recovery skips the broken newest backup for the intact one.
    std::fs::write(&broken, &bytes[..bytes.len() / 2]).unwrap();
    let expected = counts(&conn);
    drop(conn);
    drop(blobs);
    std::fs::write(dir.path().join("lore.db"), b"corrupted main db").unwrap();
    let _ = std::fs::remove_file(dir.path().join("lore.db-wal"));
    let _ = std::fs::remove_file(dir.path().join("lore.db-shm"));
    match lore_core::recovery::recover_archive(dir.path(), &backups).unwrap() {
        lore_core::recovery::RecoveryOutcome::Restored { backup_path, .. } => {
            assert_eq!(backup_path, info.path);
        }
        other => panic!("expected Restored, got {other:?}"),
    }
    let (restored, _) = archive(dir.path());
    assert_eq!(counts(&restored), expected);
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
    assert!(
        result.is_empty(),
        "nonexistent backup directory returns empty list"
    );
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
    assert!(
        result.is_empty(),
        "directories matching backup naming must be ignored"
    );
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

#[test]
fn backup_error_from_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "cannot write backup");
    let backup_err: lore_core::backup::BackupError = io_err.into();
    assert!(matches!(backup_err, lore_core::backup::BackupError::Io));
    assert_eq!(
        backup_err.to_string(),
        "io error while creating or pruning backup"
    );
}

#[test]
fn create_backup_fails_cleanly_when_backup_dir_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();

    let fake_dir = dir.path().join("file_blocking_backup_dir");
    std::fs::write(&fake_dir, b"blocking").unwrap();

    let res = create_backup(&conn, &fake_dir, DEFAULT_BACKUP_RETENTION);
    assert!(res.is_err());
}

#[test]
fn create_backup_with_keep_exceeding_count_retains_all_backups() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let backup_dir = dir.path().join("backups");

    let b1 = create_backup(&conn, &backup_dir, 50).unwrap();
    let b2 = create_backup(&conn, &backup_dir, 50).unwrap();
    let b3 = create_backup(&conn, &backup_dir, 50).unwrap();

    assert!(b1.path.exists());
    assert!(b2.path.exists());
    assert!(b3.path.exists());

    let list = list_backups(&backup_dir);
    assert_eq!(list.len(), 3);
}

#[test]
fn create_backup_handles_trailing_slashes_in_directory_path() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let backup_dir = dir.path().join("backups_trailing");
    std::fs::create_dir_all(&backup_dir).unwrap();

    let path_with_slash = std::path::PathBuf::from(format!("{}/", backup_dir.to_string_lossy()));
    let backup = create_backup(&conn, &path_with_slash, 5).unwrap();
    assert!(backup.path.exists());
    assert_eq!(
        backup.size_bytes,
        std::fs::metadata(&backup.path).unwrap().len()
    );
}
