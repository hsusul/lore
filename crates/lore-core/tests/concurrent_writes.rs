//! Two archive writers on the same on-disk database — like the UI connection and
//! the background ingest worker — must never surface "database is locked".
//! lore-core serializes writers in-process with `storage::write_lock`, so the
//! two connections take turns instead of colliding on SQLite's write lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use lore_core::storage::StorageError;
use lore_core::{folders, storage};

fn is_locked(error: &StorageError) -> bool {
    let message = error.to_string();
    message.contains("locked") || message.contains("busy")
}

#[test]
fn concurrent_writers_never_hit_a_locked_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    let ui = storage::open(&path).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let worker_path = path.clone();
    let worker_stop = stop.clone();
    // A second connection to the same file, writing continuously through the
    // guarded folder path — stands in for the background worker's ingestion.
    let worker = std::thread::spawn(move || {
        let conn = storage::open(&worker_path).unwrap();
        let mut busy = 0u64;
        while !worker_stop.load(Ordering::Relaxed) {
            if let Err(error) = folders::create_folder(&conn, "w") {
                if is_locked(&error) {
                    busy += 1;
                }
            }
        }
        busy
    });

    // Meanwhile the "UI" connection writes folders as fast as it can.
    let mut ui_busy = 0u64;
    for _ in 0..500 {
        if let Err(error) = folders::create_folder(&ui, "x") {
            if is_locked(&error) {
                ui_busy += 1;
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let worker_busy = worker.join().unwrap();

    assert_eq!(
        ui_busy, 0,
        "UI writes hit 'database is locked' {ui_busy} time(s)"
    );
    assert_eq!(
        worker_busy, 0,
        "worker writes hit 'database is locked' {worker_busy} time(s)"
    );
}

#[test]
fn ingest_file_serializes_on_the_process_write_lock() {
    use lore_core::adapters::claude_code::ClaudeCodeAdapter;
    use lore_core::ingest::ingest_file;
    use lore_core::storage::blob::BlobStore;

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("session.jsonl");
    std::fs::write(
        &source,
        "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"s1\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
    )
    .unwrap();

    let conn = storage::open_in_memory().unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let adapter = ClaudeCodeAdapter::new();

    let (tx, rx) = std::sync::mpsc::channel();
    // Hold the process-wide write lock so the worker's ingest must wait for it.
    let guard = storage::write_lock();
    let handle = std::thread::spawn(move || {
        let result = ingest_file(&conn, &adapter, &source, &blobs);
        tx.send(result.is_ok()).unwrap();
    });

    // With the lock held, ingest_file must block rather than completing (and
    // sending) while another archive writer is mid-transaction. Without the
    // guard this in-memory ingest finishes almost instantly and this assertion
    // fails, catching the regression.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        rx.try_recv().is_err(),
        "ingest_file must serialize on the process write lock"
    );

    drop(guard);
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(),
        "ingest_file must complete once the lock is released"
    );
    handle.join().unwrap();
}
