//! Lore desktop shell (Tauri 2) — a thin binary over `lore-core`.
//!
//! No archive logic lives here: commands lock the core's SQLite connection,
//! delegate to `lore-core`, and return `lore-ipc` DTOs. Progress is relayed as
//! content-free `scan_progress` events. The updater (the only network-capable
//! component) is behind the off-by-default `updater` feature and is not wired
//! in here.

use std::sync::Mutex;
use std::time::Duration;

use lore_core::adapters::AdapterRegistry;
use lore_core::discovery::{watch_roots, DiscoveryConfig};
use lore_core::pipeline::{Pipeline, ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_core::watcher::SessionWatcher;
use lore_core::worker::{self, WorkerConfig, WorkerHandle};
use lore_ipc::{
    DetectedAgent, ForgetReport, GitObservationDto, RepositorySummary, RescanResult, ScanProgress,
    SearchHit, SearchPage, SessionDetail, SessionSummary,
};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

/// Backpressure ceiling for queued ingest jobs in one scan.
const QUEUE_CAPACITY: usize = 100_000;
/// Upper bound on jobs drained per rescan call.
const DRAIN_BUDGET: usize = 1_000_000;
/// Quiet period a source path must be idle before the watcher hands it to the
/// worker, coalescing partial writes and event storms.
const WATCH_QUIET: Duration = Duration::from_millis(400);

/// Process-wide application state: the UI archive connection (guarded; rusqlite
/// `Connection` is `Send` but not `Sync`), the blob store, the adapter registry,
/// discovery configuration, and a handle to the background ingestion worker.
///
/// The worker runs on its own thread with its own connection, so continuous
/// background ingestion never blocks UI queries or holds this connection's lock
/// across file parsing or Git work.
struct AppState {
    db: Mutex<Connection>,
    blobs: BlobStore,
    registry: AdapterRegistry,
    config: DiscoveryConfig,
    worker: Mutex<Option<WorkerHandle>>,
}

/// A progress sink that accumulates content-free counts and relays them to the
/// webview as `scan_progress` events.
struct EmitSink<'a> {
    app: &'a AppHandle,
    progress: Mutex<ScanProgress>,
}

impl<'a> EmitSink<'a> {
    fn new(app: &'a AppHandle) -> Self {
        Self {
            app,
            progress: Mutex::new(ScanProgress::default()),
        }
    }

    fn snapshot(&self) -> ScanProgress {
        self.progress.lock().map(|p| *p).unwrap_or_default()
    }
}

impl ProgressSink for EmitSink<'_> {
    fn emit(&self, event: ProgressEvent) {
        if let Ok(mut progress) = self.progress.lock() {
            match event {
                ProgressEvent::ScanEnqueued { discovered, .. } => {
                    progress.discovered = discovered as i64;
                }
                ProgressEvent::Ingested { .. } => progress.ingested += 1,
                ProgressEvent::Skipped { .. } => progress.skipped += 1,
                ProgressEvent::Failed { .. } => progress.failed += 1,
                ProgressEvent::Requeued { .. } => {}
            }
            let _ = self.app.emit("scan_progress", *progress);
        }
    }
}

/// An owned, thread-safe progress sink for the background worker. Accumulates
/// content-free counts and relays them to the webview as `scan_progress` events.
/// Unlike [`EmitSink`] it owns a cloned [`AppHandle`], so it can live on the
/// worker thread for the whole app lifetime.
struct WorkerSink {
    app: AppHandle,
    progress: Mutex<ScanProgress>,
}

impl WorkerSink {
    fn new(app: AppHandle) -> Self {
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
                    // Live cumulative gauge: the worker never claims a pass is
                    // "done" (ingestion is continuous), so `done` stays false.
                    progress.discovered = progress.discovered.max(discovered as i64);
                }
                ProgressEvent::Ingested { .. } => progress.ingested += 1,
                ProgressEvent::Skipped { .. } => progress.skipped += 1,
                ProgressEvent::Failed { .. } => progress.failed += 1,
                ProgressEvent::Requeued { .. } => {}
            }
            let _ = self.app.emit("scan_progress", *progress);
        }
    }
}

/// Report the archive core version.
#[tauri::command]
fn core_version() -> String {
    lore_core::version().to_string()
}

/// List the agents Lore knows about, with ingested-session counts.
#[tauri::command]
fn list_detected_agents(state: State<'_, AppState>) -> Result<Vec<DetectedAgent>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_agents(&conn).map_err(|e| e.to_string())
}

/// List the most recent sessions (newest first), capped at `limit`.
#[tauri::command]
fn list_sessions(state: State<'_, AppState>, limit: i64) -> Result<Vec<SessionSummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_sessions(&conn, limit.clamp(1, 10_000)).map_err(|e| e.to_string())
}

/// List the repositories resolved by git enrichment.
#[tauri::command]
fn list_repositories(state: State<'_, AppState>) -> Result<Vec<RepositorySummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_repositories(&conn).map_err(|e| e.to_string())
}

/// List the most recent sessions that touched a repository.
#[tauri::command]
fn list_repository_sessions(
    state: State<'_, AppState>,
    id: String,
    limit: i64,
) -> Result<Vec<SessionSummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_repository_sessions(&conn, &id, limit.clamp(1, 10_000))
        .map_err(|e| e.to_string())
}

/// Read one session in context (header, segments, ordered-part timeline, files).
#[tauri::command]
fn get_session(state: State<'_, AppState>, id: String) -> Result<Option<SessionDetail>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::get_session(&conn, &id).map_err(|e| e.to_string())
}

/// Fetch the recorded patch text for a file event, or null when none is stored,
/// the payload is not valid UTF-8, or the blob is quarantined (its scan never
/// completed, so its content stays unavailable — SECRET_SCANNING.md §6).
#[tauri::command]
fn get_file_patch(state: State<'_, AppState>, id: String) -> Result<Option<String>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::file_patch_text(&conn, &state.blobs, &id).map_err(|e| e.to_string())
}

/// Read the provenance-labeled git observations for a session.
#[tauri::command]
fn get_git_snapshot(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<GitObservationDto>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::get_git_snapshot(&conn, &id).map_err(|e| e.to_string())
}

/// How many secrets were flagged in a session (all redacted from derived surfaces).
#[tauri::command]
fn session_secret_count(state: State<'_, AppState>, id: String) -> Result<i64, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::secret_count(&conn, &id).map_err(|e| e.to_string())
}

/// Export a session as Markdown. `include_secrets` defaults off (masked); passing
/// true is an explicit opt-in to full-fidelity content.
#[tauri::command]
fn export_session_markdown(
    state: State<'_, AppState>,
    id: String,
    include_secrets: bool,
) -> Result<Option<String>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::export::export_session_markdown(&conn, &id, include_secrets)
        .map_err(|e| e.to_string())
}

/// Forget a session: remove its rows, projections, findings, and orphan blobs.
#[tauri::command]
fn forget_session(state: State<'_, AppState>, id: String) -> Result<ForgetReport, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let report =
        lore_core::forget::forget_session(&conn, &state.blobs, &id).map_err(|e| e.to_string())?;
    Ok(ForgetReport {
        blobs_removed: i64::try_from(report.blobs_removed).unwrap_or(i64::MAX),
        source_paths: report.source_paths,
    })
}

/// Forget everything: wipe all archive content (sessions, repos, sources,
/// projections, findings, blobs) while keeping the database file open. Settings
/// and the job queue are preserved.
#[tauri::command]
fn forget_everything(state: State<'_, AppState>) -> Result<ForgetReport, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let report = lore_core::forget::forget_all(&conn, &state.blobs).map_err(|e| e.to_string())?;
    Ok(ForgetReport {
        blobs_removed: i64::try_from(report.blobs_removed).unwrap_or(i64::MAX),
        source_paths: report.source_paths,
    })
}

/// Full-text search over the redacted projections (secret-safe by construction).
#[tauri::command]
fn search(state: State<'_, AppState>, query: String, limit: i64) -> Result<Vec<SearchHit>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::search::search(&conn, &query, limit).map_err(|e| e.to_string())
}

/// Paginated full-text search. `cursor` is `None` for the first page; pass the
/// returned `next_cursor` back verbatim for the next page (valid only for the
/// same query). Keyset-based, so paging never drops or repeats a result.
#[tauri::command]
fn search_page(
    state: State<'_, AppState>,
    query: String,
    limit: i64,
    cursor: Option<String>,
) -> Result<SearchPage, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::search::search_page(&conn, &query, limit, cursor.as_deref())
        .map_err(|e| e.to_string())
}

/// Run a discovery→ingest→enrich pass, streaming `scan_progress` events, and
/// return the final tally.
#[tauri::command]
fn rescan(app: AppHandle, state: State<'_, AppState>) -> Result<RescanResult, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let sink = EmitSink::new(&app);
    let pipeline = Pipeline::new(
        &conn,
        &state.registry,
        &state.blobs,
        &state.config,
        QUEUE_CAPACITY,
    );
    pipeline.enqueue_scan(&sink).map_err(|e| e.to_string())?;
    let summary = pipeline
        .drain(&sink, DRAIN_BUDGET)
        .map_err(|e| e.to_string())?;

    let mut final_progress = sink.snapshot();
    final_progress.enriched = summary.enriched as i64;
    final_progress.done = true;
    let _ = app.emit("scan_progress", final_progress);

    Ok(RescanResult {
        discovered: final_progress.discovered,
        ingested: summary.ingested as i64,
        skipped: summary.skipped as i64,
        failed: summary.failed as i64,
        enriched: summary.enriched as i64,
    })
}

/// Build the discovery configuration.
///
/// In release builds this is the adapters' documented default roots (the user's
/// real `~/.claude` / `~/.codex`). In **debug** builds only, the roots can be
/// redirected to a synthetic profile via `LORE_DEV_CLAUDE_ROOT` /
/// `LORE_DEV_CODEX_ROOT` so `cargo tauri dev` can run against generated fixtures
/// (see `lore_core::synthetic`) instead of real history. Release builds ignore
/// these variables entirely, so shipped Lore never takes an env-driven root.
fn dev_config() -> DiscoveryConfig {
    #[allow(unused_mut)]
    let mut config = DiscoveryConfig::new();
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
    config
}

/// Open (creating if needed) the archive under the app data directory, build the
/// shared state, and start the background ingestion worker.
///
/// The worker gets its **own** connection to the same database file (WAL lets
/// the UI and worker connections read/write concurrently) and its own recursive
/// [`SessionWatcher`] over the adapters' effective roots. On its thread it
/// recovers interrupted jobs, runs the initial incremental scan, then keeps
/// converting debounced source changes into durable coalesced jobs and draining
/// them in bounded batches — all off the UI thread.
fn init_state(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("lore.db");
    let conn = lore_core::storage::open(&db_path)?;
    let blobs = BlobStore::open(data_dir.join("blobs"))?;
    let config = dev_config();

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
    let handle = worker::spawn(worker, watcher, WorkerSink::new(app.clone()));

    Ok(AppState {
        db: Mutex::new(conn),
        blobs,
        registry: AdapterRegistry::v0(),
        config,
        worker: Mutex::new(Some(handle)),
    })
}

/// Build and run the desktop application.
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let state = init_state(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_version,
            list_detected_agents,
            list_sessions,
            list_repositories,
            list_repository_sessions,
            get_session,
            get_git_snapshot,
            get_file_patch,
            session_secret_count,
            export_session_markdown,
            forget_session,
            forget_everything,
            search,
            search_page,
            rescan
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
            }
        }
    });
}
