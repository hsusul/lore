//! Lore desktop shell (Tauri 2) — a thin binary over `lore-core`.
//!
//! No archive logic lives here: commands lock the core's SQLite connection,
//! delegate to `lore-core`, and return `lore-ipc` DTOs. Progress is relayed as
//! content-free `scan_progress` events. The updater (the only network-capable
//! component) is behind the off-by-default `updater` feature and is not wired
//! in here.

pub mod commands;
pub mod state;

pub use commands::*;
pub use state::{acquire_writer_lock, app_config, init_state, AppState, WorkerSink, WATCH_QUIET};

use tauri::{Manager, RunEvent};

/// Build and run the desktop application.
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = init_state(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_version,
            list_detected_agents,
            list_sessions,
            list_sessions_page,
            list_repositories,
            list_repository_sessions,
            list_repository_sessions_page,
            get_session,
            list_session_messages_page,
            get_git_snapshot,
            get_file_patch,
            session_secret_count,
            export_session_markdown,
            save_session_export,
            forget_session,
            forget_everything,
            search,
            search_page,
            list_folders,
            create_folder,
            rename_folder,
            delete_folder,
            set_session_folder,
            list_folder_sessions_page,
            get_setting,
            set_setting,
            get_backup_schedule,
            set_backup_schedule,
            backup_now,
            add_agent_root,
            remove_agent_root,
            rescan,
            reverify,
            get_token_totals,
            relink_segment_repository,
            check_for_updates
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("fatal: error while running Lore: {error}");
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| {
        // Shut the background worker down cleanly as the event loop exits: it
        // finishes its current bounded step, then the thread joins. Interrupted
        // work stays durable in SQLite and is recovered on the next launch, so
        // checkpoints are never corrupted and no job is left unrecoverable.
        if let RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                if let Ok(mut guard) = state.worker.lock() {
                    if let Some(handle) = guard.take() {
                        handle.shutdown();
                    }
                }
                // Fold the write-ahead log back now that the worker thread is
                // joined. This is the only moment a `TRUNCATE` checkpoint can
                // actually shrink the file: for the whole run this connection is
                // an open reader, and a reader holds up truncation, so the
                // in-scan checkpoint can only ever fall back to `PASSIVE`. An
                // archive left running for weeks accumulated a 360 MB WAL beside
                // a 1.25 GB database, and every read had to traverse it.
                if let Ok(conn) = state.db.lock() {
                    lore_core::storage::checkpoint(&conn);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lore_core::adapters::AdapterRegistry;
    use lore_core::discovery::DiscoveryConfig;
    use lore_core::is_invalid_text_token;

    /// The safety property the desktop app owes every other writer: it may run a
    /// background worker only while it holds the archive's writer lock.
    ///
    /// `worker::spawn` recovers and scans immediately, and `jobs::recover_running`
    /// returns every `running` job to `pending` with no way to tell whose it was,
    /// so a worker started without the lock reclaims a live CLI scan's jobs and
    /// both then ingest the same source.
    #[test]
    fn no_writer_lock_means_no_background_worker() {
        let archive = tempfile::tempdir().unwrap();

        // Nothing else holds it: the app gets the lock, so a worker is allowed.
        let held = acquire_writer_lock(archive.path());
        assert!(held.is_some(), "the lock was free and was not taken");

        // Something else holds it: the app must start read-only. `is_some()` on
        // the returned lock is exactly the condition `init_state` uses to decide
        // whether to spawn, so this is the real gate, not a proxy for it.
        let contended = acquire_writer_lock(archive.path());
        assert!(
            contended.is_none(),
            "the app would have started a worker while another writer held the lock"
        );

        drop(held);
        assert!(
            acquire_writer_lock(archive.path()).is_some(),
            "the lock was not released when the holder went away"
        );
    }

    #[test]
    fn an_unusable_archive_directory_also_means_no_worker() {
        // A lock that cannot be taken for any reason — not just contention —
        // must still withhold the worker rather than proceeding unlocked.
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("no-such-archive");
        assert!(acquire_writer_lock(&missing).is_none());
    }

    #[test]
    fn the_app_and_the_cli_contend_for_the_same_lock() {
        // Both surfaces must name the same file, or each would hold its own and
        // believe it was alone. `lorectl` builds the path the same way, from
        // `paths::SCAN_LOCK_FILENAME`.
        let archive = tempfile::tempdir().unwrap();
        let app_hold = acquire_writer_lock(archive.path()).expect("app takes it");
        assert_eq!(
            app_hold.path(),
            archive.path().join(lore_core::paths::SCAN_LOCK_FILENAME)
        );
        assert_eq!(
            lore_core::lock::ScanLock::acquire(archive.path()).unwrap_err(),
            lore_core::lock::LockError::Held,
            "a CLI writer was not blocked by the app's lock"
        );
    }

    /// The archive location is `dirs::data_dir()/<identifier>`, and `lore-core`
    /// holds that identifier as a constant so a CLI can resolve the same path
    /// without Tauri. Editing `identifier` in `tauri.conf.json` alone would move
    /// the app's archive and leave every other surface pointed at the old one —
    /// a silently empty archive rather than an error. This pins them together.
    #[test]
    fn tauri_conf_identifier_matches_core_constant() {
        let conf = include_str!("../tauri.conf.json");
        let needle = format!("\"identifier\": \"{}\"", lore_core::paths::APP_IDENTIFIER);
        assert!(
            conf.contains(&needle),
            "tauri.conf.json identifier must equal lore_core::paths::APP_IDENTIFIER ({})",
            lore_core::paths::APP_IDENTIFIER
        );
    }

    #[test]
    fn test_is_invalid_folder_name() {
        assert!(!is_invalid_folder_name("My Projects"));
        assert!(is_invalid_folder_name("Folder\x00Name"));
        assert!(is_invalid_folder_name("Folder\u{200B}Name"));
        assert!(is_invalid_folder_name(&"a".repeat(257)));
    }

    #[test]
    fn test_is_invalid_text_token() {
        assert!(!is_invalid_text_token("valid_token_123", 64));
        assert!(is_invalid_text_token("", 64));
        assert!(is_invalid_text_token("invalid\x07", 64));
        assert!(is_invalid_text_token("invalid\u{200C}", 64));
        assert!(is_invalid_text_token(&"x".repeat(65), 64));
    }

    #[test]
    fn test_is_invalid_setting_key_and_folder_id() {
        assert!(!is_invalid_setting_key("ui.theme"));
        assert!(is_invalid_setting_key(""));
        assert!(is_invalid_setting_key("ui\0theme"));
        assert!(is_invalid_setting_key(&"k".repeat(129)));

        assert!(!is_invalid_folder_id("0123456789abcdef0123456789abcdef"));
        assert!(is_invalid_folder_id(""));
        assert!(is_invalid_folder_id("0123\u{200d}456"));
        assert!(is_invalid_folder_id(&"f".repeat(65)));
    }

    struct TestTempDir(std::path::PathBuf);
    impl TestTempDir {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "lore_test_{}_{}_{}",
                std::process::id(),
                name,
                nanos
            ));
            let _ = std::fs::create_dir_all(&path);
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_validate_export_path() {
        let temp = TestTempDir::new("export_guard");
        let archive_dir = temp.path().join("archive");
        let codex_home = temp.path().join("codex_home");
        let valid_export_dir = temp.path().join("exports");
        std::fs::create_dir_all(&archive_dir).unwrap();
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&valid_export_dir).unwrap();

        let config = DiscoveryConfig::new();
        let registry = AdapterRegistry::v0();

        // 1. Relative path rejected
        let rel = std::path::Path::new("export.md");
        assert_eq!(
            validate_export_path(rel, &archive_dir, &registry, &config).unwrap_err(),
            "export path must be absolute"
        );

        // 2. Archive dir rejected
        let inside_archive = archive_dir.join("export.md");
        assert_eq!(
            validate_export_path(&inside_archive, &archive_dir, &registry, &config).unwrap_err(),
            "cannot export inside the Lore archive directory"
        );

        // 3. CODEX_HOME root rejected
        std::env::set_var("CODEX_HOME", &codex_home);
        let inside_codex = codex_home.join("export.md");
        let codex_err =
            validate_export_path(&inside_codex, &archive_dir, &registry, &config).unwrap_err();
        std::env::remove_var("CODEX_HOME");
        assert_eq!(codex_err, "cannot export inside agent session directories");

        // 4. Valid destination accepted
        let valid_path = valid_export_dir.canonicalize().unwrap().join("export.md");
        let res = validate_export_path(&valid_path, &archive_dir, &registry, &config).unwrap();
        assert_eq!(res, valid_path);
    }
}
