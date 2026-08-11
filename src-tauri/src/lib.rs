//! Lore desktop shell (Tauri 2) — a thin binary over `lore-core`.
//!
//! No archive logic lives here: commands lock the core's SQLite connection,
//! delegate to `lore-core`, and return `lore-ipc` DTOs. Progress is relayed as
//! content-free `scan_progress` events. The updater (the only network-capable
//! component) is behind the off-by-default `updater` feature and is not wired
//! in here.

use std::sync::Mutex;

use lore_core::adapters::AdapterRegistry;
use lore_core::discovery::DiscoveryConfig;
use lore_core::pipeline::{Pipeline, ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_ipc::{
    DetectedAgent, GitObservationDto, RepositorySummary, RescanResult, ScanProgress, SessionDetail,
    SessionSummary,
};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};

/// Backpressure ceiling for queued ingest jobs in one scan.
const QUEUE_CAPACITY: usize = 100_000;
/// Upper bound on jobs drained per rescan call.
const DRAIN_BUDGET: usize = 1_000_000;

/// Process-wide application state: the archive connection (guarded; rusqlite
/// `Connection` is `Send` but not `Sync`), the blob store, the adapter registry,
/// and discovery configuration.
struct AppState {
    db: Mutex<Connection>,
    blobs: BlobStore,
    registry: AdapterRegistry,
    config: DiscoveryConfig,
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

/// Read one session in context (header, segments, ordered-part timeline, files).
#[tauri::command]
fn get_session(state: State<'_, AppState>, id: String) -> Result<Option<SessionDetail>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::get_session(&conn, &id).map_err(|e| e.to_string())
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

/// Open (creating if needed) the archive under the app data directory and build
/// the shared state.
fn init_state(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let conn = lore_core::storage::open(&data_dir.join("lore.db"))?;
    let blobs = BlobStore::open(data_dir.join("blobs"))?;
    Ok(AppState {
        db: Mutex::new(conn),
        blobs,
        registry: AdapterRegistry::v0(),
        config: DiscoveryConfig::new(),
    })
}

/// Build and run the desktop application.
pub fn run() {
    let result = tauri::Builder::default()
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
            get_session,
            get_git_snapshot,
            rescan
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("fatal: error while running Lore: {error}");
        std::process::exit(1);
    }
}
