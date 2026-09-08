use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use lore_core::adapters::AdapterRegistry;
use lore_core::discovery::{watch_roots, DiscoveryConfig};
use lore_core::lock::{LockError, ScanLock};
use lore_core::pipeline::{ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_core::watcher::SessionWatcher;
use lore_core::worker::{self, WorkerConfig, WorkerHandle};
use lore_ipc::{IndexUpdatedEvent, JobFailedEvent, ScanProgress, SessionIngestedEvent};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

/// Quiet period a source path must be idle before the watcher hands it to the
/// worker, coalescing partial writes and event storms.
pub const WATCH_QUIET: Duration = Duration::from_millis(400);

/// Process-wide application state: the UI archive connection (guarded; rusqlite
/// `Connection` is `Send` but not `Sync`), the blob store, the adapter registry,
/// discovery configuration, and a handle to the background ingestion worker.
///
/// The worker runs on its own thread with its own connection, so continuous
/// background ingestion never blocks UI queries or holds this connection's lock
/// across file parsing or Git work.
pub struct AppState {
    pub db: Mutex<Connection>,
    pub blobs: BlobStore,
    pub registry: AdapterRegistry,
    pub config: Mutex<DiscoveryConfig>,
    pub worker: Mutex<Option<WorkerHandle>>,
    /// The Lore-owned archive root (`app_data_dir`); used to purge on-disk
    /// backups/cache/quarantine on "forget everything".
    pub archive_dir: PathBuf,
    /// Exclusive writer hold on this archive, kept for the app's lifetime.
    ///
    /// Held because `jobs::recover_running` returns **every** `running` job to
    /// `pending` and the `job` table has no owner column: a second writer that
    /// recovers while this app is mid-ingest reclaims its in-flight work, and
    /// both then run the same source. The lock is what makes "one writer at a
    /// time" true across processes (`lore_core::lock`, `ARCHITECTURE.md` §3.2).
    ///
    /// `None` means the lock was unavailable at startup, in which case no worker
    /// was spawned and this instance never writes. Dropping the guard with the
    /// app releases the lock.
    pub _scan_lock: Option<ScanLock>,
}

/// An owned, thread-safe progress sink for the background worker. Accumulates
/// content-free counts and relays them to the webview as `scan_progress` events.
/// Unlike [`EmitSink`] it owns a cloned [`AppHandle`], so it can live on the
/// worker thread for the whole app lifetime.
pub struct WorkerSink {
    app: AppHandle,
    progress: Mutex<ScanProgress>,
}

impl WorkerSink {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            progress: Mutex::new(ScanProgress::default()),
        }
    }
}

impl ProgressSink for WorkerSink {
    fn emit(&self, event: ProgressEvent) {
        if let Ok(mut progress) = self.progress.lock() {
            match event {
                ProgressEvent::ScanEnqueued { discovered, .. } => {
                    *progress = ScanProgress {
                        discovered: i64::try_from(discovered).unwrap_or(i64::MAX),
                        done: false,
                        ..ScanProgress::default()
                    };
                }
                ProgressEvent::ScanFinished => progress.done = true,
                ProgressEvent::Ingested {
                    agent_id,
                    session_id,
                    ..
                } => {
                    progress.ingested += 1;
                    let _ = self.app.emit(
                        "session_ingested",
                        SessionIngestedEvent {
                            session_id: session_id.clone(),
                            agent_id,
                            title: None,
                            parse_status: "ok".to_string(),
                        },
                    );
                    let _ = self.app.emit(
                        "index_updated",
                        IndexUpdatedEvent {
                            session_id,
                            documents_indexed: 1,
                        },
                    );
                }
                ProgressEvent::Skipped { .. } => progress.skipped += 1,
                ProgressEvent::Failed { agent_id, kind } => {
                    progress.failed += 1;
                    let _ = self.app.emit(
                        "job_failed",
                        JobFailedEvent {
                            job_id: "".to_string(),
                            kind: format!("{kind:?}"),
                            error: format!("Ingest failed for adapter {agent_id}"),
                        },
                    );
                }
                ProgressEvent::Requeued { .. } => {}
            }
            let _ = self.app.emit("scan_progress", *progress);
        }
    }
}

/// Build the discovery configuration.
///
/// In release builds this combines the adapters' documented defaults with
/// persisted user-selected roots. In **debug** builds only, either adapter can
/// instead be redirected to a synthetic profile via `LORE_DEV_CLAUDE_ROOT` /
/// `LORE_DEV_CODEX_ROOT` so `cargo tauri dev` can run against generated fixtures
/// (see `lore_core::synthetic`) instead of real history. Release builds ignore
/// these variables entirely, so shipped Lore never takes an env-driven root.
pub fn app_config(
    conn: &Connection,
    registry: &AdapterRegistry,
) -> lore_core::source_roots::Result<DiscoveryConfig> {
    let mut config = lore_core::source_roots::discovery_config(conn, registry)?;
    #[cfg(debug_assertions)]
    {
        use lore_core::adapters::DiscoveryRoots;
        for (var, agent) in [
            ("LORE_DEV_CLAUDE_ROOT", "claude-code"),
            ("LORE_DEV_CODEX_ROOT", "codex"),
        ] {
            match std::env::var(var) {
                Ok(path) if !path.is_empty() => {
                    eprintln!("dev: {agent} discovery root overridden by {var}");
                    config.set_roots(agent, DiscoveryRoots::new(vec![path.into()]));
                }
                _ => {}
            }
        }
    }
    Ok(config)
}

/// Take the archive writer lock, or explain why the app is starting read-only.
///
/// Extracted from [`init_state`] so the decision is testable without a Tauri
/// `AppHandle`. The decision is the safety-relevant part: `worker::spawn`
/// immediately recovers and scans, and `jobs::recover_running` returns *every*
/// `running` job to `pending` with no owner column, so spawning a worker without
/// this lock reclaims another live writer's in-flight work
/// (`ARCHITECTURE.md` §3.2).
///
/// `None` means no lock, which must mean no worker. It is deliberately not an
/// error: the archive is still readable, so the app starts and every
/// worker-backed command reports "background ingestion worker unavailable"
/// rather than the app refusing to launch because a CLI scan happens to be
/// running.
pub fn acquire_writer_lock(archive_dir: &Path) -> Option<ScanLock> {
    match ScanLock::acquire(archive_dir) {
        Ok(lock) => Some(lock),
        Err(LockError::Held) => {
            eprintln!(
                "warning: another Lore process is writing to this archive; \
                 starting read-only (no background ingestion)"
            );
            None
        }
        Err(error) => {
            eprintln!(
                "warning: could not take the archive writer lock ({error}); \
                 starting read-only (no background ingestion)"
            );
            None
        }
    }
}

pub fn init_state(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
    // Resolved by lore-core, not by `app.path().app_data_dir()`, so a CLI over
    // the same core opens the same archive. The default is byte-identical to
    // Tauri's (`dirs::data_dir()` joined with the bundle identifier — asserted
    // by `tauri_conf_identifier_matches_core_constant` below); an explicit
    // `LORE_ARCHIVE_DIR` now moves both surfaces together rather than only one.
    let data_dir = lore_core::paths::archive_dir(None)?;
    std::fs::create_dir_all(&data_dir)?;

    // Taken before the database is opened, and held for the whole run. Anything
    // between opening and locking is a window in which two writers both believe
    // they are alone — the same ordering `lorectl scan` uses.
    //
    // Unavailable is not fatal: the archive is still perfectly readable, so the
    // app starts *without* a background worker rather than refusing to launch
    // because a CLI scan happens to be running. Every worker-backed command
    // already reports "background ingestion worker unavailable" for that state.
    // What must never happen is starting a worker without the lock — that is the
    // hijack this exists to prevent.
    let scan_lock = acquire_writer_lock(&data_dir);

    let db_path = data_dir.join(lore_core::paths::ARCHIVE_DB_FILENAME);
    let conn = lore_core::storage::open(&db_path)?;
    let blobs = BlobStore::open(data_dir.join(lore_core::paths::BLOBS_DIRNAME))?;
    let registry = AdapterRegistry::v0();
    let config = app_config(&conn, &registry)?;

    // Run an automatic backup at launch if one is due per the user's schedule
    // (a no-op when off or not yet due). Best-effort: a backup failure must never
    // block the app from starting.
    if let Err(e) = lore_core::backup::run_scheduled_backup(
        &conn,
        &data_dir.join(lore_core::paths::BACKUPS_DIRNAME),
        lore_core::now_ms(),
    ) {
        eprintln!("warning: scheduled backup skipped: {e}");
    }

    // Background worker: independent connection + registry, watching the same
    // roots the UI's discovery config resolves.
    let worker = worker::open_worker(
        &db_path,
        AdapterRegistry::v0(),
        blobs.clone(),
        config.clone(),
        WorkerConfig::default(),
    )?;
    let watcher =
        match SessionWatcher::new(&watch_roots(&AdapterRegistry::v0(), &config), WATCH_QUIET) {
            Ok(watcher) => Some(watcher),
            // A watcher that cannot start (e.g. no roots yet) must not block the
            // app; the initial scan and manual rescans still work without it.
            Err(_) => {
                eprintln!("warning: filesystem watcher unavailable; live updates disabled");
                None
            }
        };
    // Only with the lock in hand. `spawn` immediately recovers and scans, which
    // is exactly the sequence that would steal another writer's jobs.
    let handle = scan_lock
        .as_ref()
        .map(|_| worker::spawn(worker, watcher, WorkerSink::new(app.clone())));

    Ok(AppState {
        db: Mutex::new(conn),
        blobs,
        registry,
        config: Mutex::new(config),
        worker: Mutex::new(handle),
        archive_dir: data_dir,
        _scan_lock: scan_lock,
    })
}
