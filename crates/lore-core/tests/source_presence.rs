//! N7a acceptance: discovery reconciliation marks `source_artifact` rows
//! `missing` when their file disappears from a still-readable, non-empty root,
//! restores them to `active` on reappearance, and never mass-marks when the root
//! itself is absent or empty (unmounted volume / removed custom root).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::DiscoveryConfig;
use lore_core::pipeline::{Pipeline, ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

struct NullSink;

impl ProgressSink for NullSink {
    fn emit(&self, _event: ProgressEvent) {}
}

fn claude_fixture() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/claude_code/basic_text.jsonl");
    fs::read_to_string(path).unwrap()
}

struct Home {
    _dir: tempfile::TempDir,
    _blob_dir: tempfile::TempDir,
    root: PathBuf,
    file: PathBuf,
    blobs: BlobStore,
    config: DiscoveryConfig,
}

fn home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("claude/projects");
    fs::create_dir_all(root.join("repo")).unwrap();
    let file = root.join("repo/session.jsonl");
    fs::write(&file, claude_fixture()).unwrap();

    let mut config = DiscoveryConfig::new();
    config.set_roots("claude-code", DiscoveryRoots::new(vec![root.clone()]));
    // Pin codex too: an unconfigured adapter falls back to its real default
    // root, which would discover the developer's actual history.
    config.set_roots(
        "codex",
        DiscoveryRoots::new(vec![dir.path().join("codex-sessions")]),
    );
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    Home {
        _dir: dir,
        _blob_dir: blob_dir,
        root,
        file,
        blobs,
        config,
    }
}

fn state_of(conn: &Connection, path: &Path) -> String {
    conn.query_row(
        "SELECT state FROM source_artifact WHERE current_path = ?1",
        [path.to_string_lossy()],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn deleted_source_flips_to_missing() {
    let home = home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    pipeline.enqueue_scan(&NullSink).unwrap();
    pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(state_of(&conn, &home.file), "active");

    fs::remove_file(&home.file).unwrap();
    pipeline.enqueue_scan(&NullSink).unwrap();

    assert_eq!(
        state_of(&conn, &home.file),
        "missing",
        "a file deleted from a live root marks its artifact missing"
    );
}

#[test]
fn reappearing_source_flips_back_to_active() {
    let home = home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    pipeline.enqueue_scan(&NullSink).unwrap();
    pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(state_of(&conn, &home.file), "active");

    fs::remove_file(&home.file).unwrap();
    pipeline.enqueue_scan(&NullSink).unwrap();
    assert_eq!(state_of(&conn, &home.file), "missing");

    fs::write(&home.file, claude_fixture()).unwrap();
    pipeline.enqueue_scan(&NullSink).unwrap();
    assert_eq!(
        state_of(&conn, &home.file),
        "active",
        "a reappearing file restores its artifact to active"
    );
}

#[test]
fn absent_root_marks_nothing() {
    let home = home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    pipeline.enqueue_scan(&NullSink).unwrap();
    pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(state_of(&conn, &home.file), "active");

    // The whole root disappears (and the file with it). An unmounted volume or
    // removed custom root must not mass-mark every artifact missing.
    fs::remove_dir_all(&home.root).unwrap();
    pipeline.enqueue_scan(&NullSink).unwrap();

    assert_eq!(
        state_of(&conn, &home.file),
        "active",
        "an absent root must not mark its artifacts missing"
    );
}

#[test]
fn empty_root_marks_nothing() {
    let home = home();
    let registry = AdapterRegistry::v0();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let pipeline = Pipeline::new(&conn, &registry, &home.blobs, &home.config, 128);
    pipeline.enqueue_scan(&NullSink).unwrap();
    pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(state_of(&conn, &home.file), "active");

    // Remove the file's parent directory so the root is present but empty — the
    // unmounted-volume shape. Emptiness is treated as "don't know", not "gone".
    fs::remove_dir_all(home.root.join("repo")).unwrap();
    pipeline.enqueue_scan(&NullSink).unwrap();

    assert_eq!(
        state_of(&conn, &home.file),
        "active",
        "an empty root must not mark its artifacts missing"
    );
}
