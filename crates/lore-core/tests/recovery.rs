//! M7 acceptance: recovery wiring (SECURITY.md §6, TESTING.md §7).
//!
//! On an integrity failure the flow closes the active DB, preserves it as a
//! quarantine artifact (never discarded automatically), and restores from the
//! newest Lore-owned local backup — without any original agent logs. A healthy
//! archive is never touched; an absent archive is a fresh start.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::codex::CodexAdapter;
use lore_core::backup::{create_backup, DEFAULT_BACKUP_RETENTION};
use lore_core::ingest::persist_session;
use lore_core::recovery::{recover_archive, RecoveryOutcome};
use lore_core::storage::blob::BlobStore;
use std::path::Path;

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

fn archive(root: &Path) -> (rusqlite::Connection, BlobStore) {
    let conn = lore_core::storage::open(&root.join("lore.db")).unwrap();
    let blobs = BlobStore::open(root.join("blobs")).unwrap();
    (conn, blobs)
}

fn populate(conn: &rusqlite::Connection, blobs: &BlobStore) {
    let parsed = CodexAdapter::new().parse_str(&codex_patch("s-a", "const A = 1"), "s-a");
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
    let parsed = CodexAdapter::new().parse_str(&codex_patch("s-b", "const B = 2"), "s-b");
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
}

fn counts(conn: &rusqlite::Connection) -> (i64, i64, i64) {
    conn.query_row(
        "SELECT
            (SELECT count(*) FROM agent_session),
            (SELECT count(*) FROM message_part),
            (SELECT count(*) FROM search_document)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

#[test]
fn recover_archive_reports_absent_when_there_is_no_archive() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = recover_archive(dir.path(), &dir.path().join("backups")).unwrap();
    assert_eq!(outcome, RecoveryOutcome::Absent);
    assert!(
        !dir.path().join("quarantine").exists(),
        "nothing to quarantine on a fresh start"
    );
}

#[test]
fn recover_archive_leaves_a_healthy_archive_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    drop(conn);
    drop(blobs);

    let outcome = recover_archive(dir.path(), &dir.path().join("backups")).unwrap();
    assert_eq!(outcome, RecoveryOutcome::Healthy);
    assert!(
        !dir.path().join("quarantine").exists(),
        "healthy archive is never quarantined"
    );

    let (conn, _) = archive(dir.path());
    assert_eq!(counts(&conn).0, 2, "archive content untouched");
}

#[test]
fn recover_archive_quarantines_corruption_and_restores_from_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let expected = counts(&conn);
    let backups = dir.path().join("backups");
    create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();
    drop(conn);
    drop(blobs);

    // Corrupt the archive in place (the file is a standalone DB after the
    // connections closed and checkpointed the WAL).
    let corrupt = b"this is not a sqlite database".to_vec();
    std::fs::write(dir.path().join("lore.db"), &corrupt).unwrap();

    let outcome = recover_archive(dir.path(), &backups).unwrap();
    let (quarantine_path, backup_path) = match outcome {
        RecoveryOutcome::Restored {
            quarantine_path,
            backup_path,
        } => (quarantine_path, backup_path),
        other => panic!("expected Restored, got {other:?}"),
    };
    assert!(
        quarantine_path.starts_with(dir.path().join("quarantine")),
        "corrupt archive preserved under quarantine/"
    );
    assert_eq!(
        std::fs::read(&quarantine_path).unwrap(),
        corrupt,
        "the only archive was preserved, never discarded"
    );
    assert!(
        backup_path.starts_with(&backups),
        "restored from a Lore-owned backup"
    );

    let (restored, _) = archive(dir.path());
    assert_eq!(
        counts(&restored),
        expected,
        "recovery works from local backup without source logs"
    );
}

#[test]
fn recover_archive_without_a_backup_preserves_the_corrupt_archive() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _blobs) = archive(dir.path());
    drop(conn);
    std::fs::write(dir.path().join("lore.db"), b"corrupt").unwrap();

    let outcome = recover_archive(dir.path(), &dir.path().join("backups")).unwrap();
    let quarantine_path = match outcome {
        RecoveryOutcome::QuarantinedOnly { quarantine_path } => quarantine_path,
        other => panic!("expected QuarantinedOnly, got {other:?}"),
    };
    assert_eq!(
        std::fs::read(&quarantine_path).unwrap(),
        b"corrupt",
        "the only archive copy is preserved"
    );
    assert!(
        !dir.path().join("lore.db").exists(),
        "no fabricated replacement for the lost archive"
    );
}

#[test]
fn recover_archive_handles_unopenable_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    // Write a completely invalid 4-byte file that fails SQLite header parsing at open
    std::fs::write(dir.path().join("lore.db"), b"\xff\xff\xff\xff").unwrap();

    let outcome = recover_archive(dir.path(), &backups).unwrap();
    let quarantine_path = match outcome {
        RecoveryOutcome::QuarantinedOnly { quarantine_path } => quarantine_path,
        other => panic!("expected QuarantinedOnly, got {other:?}"),
    };
    assert_eq!(
        std::fs::read(&quarantine_path).unwrap(),
        b"\xff\xff\xff\xff"
    );
}

#[test]
fn recover_archive_falls_back_to_older_backup_when_newest_is_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = archive(dir.path());
    populate(&conn, &blobs);
    let expected = counts(&conn);
    let backups = dir.path().join("backups");

    // Create an initial valid backup (b1).
    let b1 = create_backup(&conn, &backups, DEFAULT_BACKUP_RETENTION).unwrap();
    drop(conn);
    drop(blobs);

    // Create a newer second backup (b2) with a future-dated name, but write corrupted data into it.
    let b2 = backups.join("lore-backup-20991231T235959Z-999.db");
    std::fs::write(&b2, b"corrupted backup content").unwrap();

    // Corrupt the main archive as well.
    std::fs::write(dir.path().join("lore.db"), b"corrupted main db").unwrap();

    let outcome = recover_archive(dir.path(), &backups).unwrap();
    let (quarantine_path, backup_path) = match outcome {
        RecoveryOutcome::Restored {
            quarantine_path,
            backup_path,
        } => (quarantine_path, backup_path),
        other => panic!("expected Restored, got {other:?}"),
    };

    assert!(quarantine_path.starts_with(dir.path().join("quarantine")));
    assert_eq!(backup_path, b1.path, "fell back to the older intact backup");

    let (restored, _) = archive(dir.path());
    assert_eq!(
        counts(&restored),
        expected,
        "restored database matches initial state"
    );
}

#[test]
fn recover_archive_handles_zero_byte_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(dir.path().join("lore.db"), b"").unwrap();

    let outcome = recover_archive(dir.path(), &backups).unwrap();
    let quarantine_path = match outcome {
        RecoveryOutcome::QuarantinedOnly { quarantine_path } => quarantine_path,
        other => panic!("expected QuarantinedOnly, got {other:?}"),
    };
    assert_eq!(std::fs::read(&quarantine_path).unwrap(), b"");
}

#[test]
fn recover_archive_quarantines_all_sidecar_files_including_wal_shm_journal() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();

    let db = dir.path().join("lore.db");
    let wal = dir.path().join("lore.db-wal");
    let shm = dir.path().join("lore.db-shm");
    let journal = dir.path().join("lore.db-journal");

    std::fs::write(&db, b"corrupted db").unwrap();
    std::fs::write(&wal, b"wal data").unwrap();
    std::fs::write(&shm, b"shm data").unwrap();
    std::fs::write(&journal, b"journal data").unwrap();

    let outcome = recover_archive(dir.path(), &backups).unwrap();
    let quarantine_path = match outcome {
        RecoveryOutcome::QuarantinedOnly { quarantine_path } => quarantine_path,
        other => panic!("expected QuarantinedOnly, got {other:?}"),
    };

    let q_stem = quarantine_path.file_name().unwrap().to_str().unwrap();
    let q_dir = quarantine_path.parent().unwrap();

    assert_eq!(std::fs::read(&quarantine_path).unwrap(), b"corrupted db");
    assert_eq!(
        std::fs::read(q_dir.join(format!("{q_stem}-wal"))).unwrap(),
        b"wal data"
    );
    assert_eq!(
        std::fs::read(q_dir.join(format!("{q_stem}-shm"))).unwrap(),
        b"shm data"
    );
    assert_eq!(
        std::fs::read(q_dir.join(format!("{q_stem}-journal"))).unwrap(),
        b"journal data"
    );

    // All original files in archive_dir must be cleanly moved
    assert!(!db.exists());
    assert!(!wal.exists());
    assert!(!shm.exists());
    assert!(!journal.exists());
}

#[test]
fn recover_archive_fails_cleanly_when_quarantine_dir_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();

    let db = dir.path().join("lore.db");
    std::fs::write(&db, b"corrupted db").unwrap();

    // Create a regular file named "quarantine" so create_dir_all fails
    let quarantine_file = dir.path().join("quarantine");
    std::fs::write(&quarantine_file, b"blocking file").unwrap();

    let res = recover_archive(dir.path(), &backups);
    assert!(matches!(res, Err(lore_core::recovery::RecoveryError::Io)));
}
