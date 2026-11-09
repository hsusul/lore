//! The archive holds full agent transcripts, so its database, the database's
//! `-wal` and `-shm` sidecars, and its blobs are readable by their owner only,
//! the same posture backups already have.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use lore_core::storage::{self, blob::BlobStore};

/// Octal, so a failure reads as `644` rather than `420`.
fn mode(path: &Path) -> String {
    format!(
        "{:o}",
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    )
}

fn sidecar(db: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[test]
fn a_new_database_and_its_sidecars_are_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lore.db");
    let conn = storage::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE probe (x); INSERT INTO probe VALUES (1);")
        .unwrap();

    assert_eq!(mode(&db), "600", "lore.db");
    assert_eq!(mode(&sidecar(&db, "-wal")), "600", "lore.db-wal");
    assert_eq!(mode(&sidecar(&db, "-shm")), "600", "lore.db-shm");
}

#[test]
fn new_blobs_are_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("blobs");
    let store = BlobStore::open(&root).unwrap();
    let staged = store.stage(b"@@ -1 +1 @@\n-old\n+new\n").unwrap();
    let file = root.join(staged.relpath());

    assert_eq!(mode(&root), "700", "blobs/");
    assert_eq!(mode(&root.join("tmp")), "700", "blobs/tmp/");
    assert_eq!(mode(file.parent().unwrap()), "700", "shard directory");
    assert_eq!(mode(&file), "600", "blob file");
}

#[test]
fn an_existing_archive_is_tightened_when_a_writer_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lore.db");
    let root = dir.path().join("blobs");
    drop(storage::open(&db).unwrap());
    BlobStore::open(&root).unwrap();
    // What an archive written before this fix looks like under umask 022.
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

    let _conn = storage::open(&db).unwrap();
    BlobStore::open(&root).unwrap();

    assert_eq!(mode(&db), "600", "lore.db");
    assert_eq!(mode(&root), "700", "blobs/");
}

#[test]
fn sidecars_an_older_build_left_behind_are_tightened_when_a_writer_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lore.db");
    drop(storage::open(&db).unwrap());
    // A crash under an older build leaves non-empty sidecars at 0644. SQLite
    // only gives an empty sidecar lore.db's mode, so it would keep these.
    for suffix in ["-wal", "-shm"] {
        let path = sidecar(&db, suffix);
        std::fs::write(&path, [0u8; 64]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    let _conn = storage::open(&db).unwrap();

    assert_eq!(mode(&sidecar(&db, "-wal")), "600", "lore.db-wal");
    assert_eq!(mode(&sidecar(&db, "-shm")), "600", "lore.db-shm");
}

#[test]
fn a_read_only_database_is_reported_as_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lore.db");
    drop(storage::open(&db).unwrap());
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o444)).unwrap();

    let err = storage::open(&db).unwrap_err().to_string();

    assert!(err.contains("readonly"), "{err}");
    assert_eq!(mode(&db), "400", "lore.db stays read-only");
}
