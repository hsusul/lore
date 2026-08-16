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
