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
use lore_core::pipeline::{ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_core::watcher::SessionWatcher;
use lore_core::worker::{self, WorkerConfig, WorkerHandle};
use lore_ipc::{
    BackupScheduleDto, DetectedAgent, FolderSummary, ForgetReport, GitObservationDto,
    RepositorySummary, RescanResult, ScanProgress, SearchHit, SearchPage, SessionDetail,
    SessionPage, SessionSummary,
};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

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
    config: Mutex<DiscoveryConfig>,
    worker: Mutex<Option<WorkerHandle>>,
    /// The Lore-owned archive root (`app_data_dir`); used to purge on-disk
    /// backups/cache/quarantine on "forget everything".
    archive_dir: std::path::PathBuf,
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
                    *progress = ScanProgress {
                        discovered: i64::try_from(discovered).unwrap_or(i64::MAX),
                        done: false,
                        ..ScanProgress::default()
                    };
                }
                ProgressEvent::ScanFinished => progress.done = true,
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

/// List every supported adapter using a cheap live root probe plus its
/// ingested-session count. This remains useful before the first scan.
#[tauri::command]
fn list_detected_agents(state: State<'_, AppState>) -> Result<Vec<DetectedAgent>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let config = state.config.lock().map_err(|_| "state lock poisoned")?;
    lore_core::source_roots::detected_agents(&conn, &state.registry, &config)
        .map_err(|e| e.to_string())
}

/// List the most recent sessions (newest first), capped at `limit`.
#[tauri::command]
fn list_sessions(state: State<'_, AppState>, limit: i64) -> Result<Vec<SessionSummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_sessions(&conn, limit.clamp(1, 10_000)).map_err(|e| e.to_string())
}

/// List one newest-first page of sessions using an opaque keyset cursor.
#[tauri::command]
fn list_sessions_page(
    state: State<'_, AppState>,
    limit: i64,
    cursor: Option<String>,
) -> Result<SessionPage, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_sessions_page(&conn, limit.clamp(1, 10_000), cursor.as_deref())
        .map_err(|e| e.to_string())
}

/// List the repositories resolved by git enrichment.
#[tauri::command]
fn list_repositories(state: State<'_, AppState>) -> Result<Vec<RepositorySummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_repositories(&conn).map_err(|e| e.to_string())
}

fn is_invalid_text_token(s: &str, max_len: usize) -> bool {
    s.is_empty()
        || s.len() > max_len
        || s.chars().any(|c| c.is_control() || lore_core::is_zero_width(c))
}

/// List the most recent sessions that touched `repository_id` (newest first).
#[tauri::command]
fn list_repository_sessions(
    state: State<'_, AppState>,
    id: String,
    limit: i64,
) -> Result<Vec<SessionSummary>, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid repository id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_repository_sessions(&conn, &id, limit.clamp(1, 10_000))
        .map_err(|e| e.to_string())
}

/// List one newest-first page of sessions that touched a repository.
#[tauri::command]
fn list_repository_sessions_page(
    state: State<'_, AppState>,
    id: String,
    limit: i64,
    cursor: Option<String>,
) -> Result<SessionPage, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid repository id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_repository_sessions_page(
        &conn,
        &id,
        limit.clamp(1, 10_000),
        cursor.as_deref(),
    )
    .map_err(|e| e.to_string())
}

/// Read one session in context (header, segments, ordered-part timeline, files).
#[tauri::command]
fn get_session(state: State<'_, AppState>, id: String) -> Result<Option<SessionDetail>, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::get_session(&conn, &id).map_err(|e| e.to_string())
}

/// Fetch the recorded patch text for a file event, or null when none is stored,
/// the payload is not valid UTF-8, or the blob is quarantined (its scan never
/// completed, so its content stays unavailable — SECRET_SCANNING.md §6).
#[tauri::command]
fn get_file_patch(state: State<'_, AppState>, id: String) -> Result<Option<String>, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid event id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::file_patch_text(&conn, &state.blobs, &id).map_err(|e| e.to_string())
}

/// Read the provenance-labeled git observations for a session.
#[tauri::command]
fn get_git_snapshot(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<GitObservationDto>, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::get_git_snapshot(&conn, &id).map_err(|e| e.to_string())
}

/// How many secrets were flagged in a session (all redacted from derived surfaces).
#[tauri::command]
fn session_secret_count(state: State<'_, AppState>, id: String) -> Result<i64, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
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
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::export::export_session_markdown(&conn, &id, include_secrets)
        .map_err(|e| e.to_string())
}

/// Forget a session: remove its rows, projections, findings, and orphan blobs.
#[tauri::command]
fn forget_session(state: State<'_, AppState>, id: String) -> Result<ForgetReport, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
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
    // Wipe the live database rows and blobs…
    let report = {
        let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
        lore_core::forget::forget_all(&conn, &state.blobs).map_err(|e| e.to_string())?
    };
    // …then clear the on-disk stores from which that data could be recovered
    // (backups hold whole-database copies), so "forget everything" truly forgets.
    lore_core::forget::purge_recoverable_copies(&state.archive_dir).map_err(|e| e.to_string())?;
    Ok(ForgetReport {
        blobs_removed: i64::try_from(report.blobs_removed).unwrap_or(i64::MAX),
        source_paths: report.source_paths,
    })
}

/// Full-text search over the redacted projections (secret-safe by construction).
#[tauri::command]
fn search(state: State<'_, AppState>, query: String, limit: i64) -> Result<Vec<SearchHit>, String> {
    if query.len() > 10_000 {
        return Err("query exceeds maximum length".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::search::search(&conn, &query, limit.clamp(1, 10_000)).map_err(|e| e.to_string())
}

/// Paginated full-text search. `cursor` is `None` for the first page; pass the
/// returned `next_cursor` back verbatim for the next page (valid only for the
/// same query and sort). `sort` is `"relevance"` (default), `"newest"`, or
/// `"oldest"`. Keyset-based, so paging never drops or repeats a result.
#[tauri::command]
fn search_page(
    state: State<'_, AppState>,
    query: String,
    limit: i64,
    cursor: Option<String>,
    sort: Option<String>,
) -> Result<SearchPage, String> {
    if query.len() > 10_000 {
        return Err("query exceeds maximum length".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let sort = lore_core::search::SortOrder::parse(sort.as_deref());
    lore_core::search::search_page(
        &conn,
        &query,
        limit.clamp(1, 10_000),
        cursor.as_deref(),
        sort,
    )
    .map_err(|e| e.to_string())
}

/// List the user-defined folders with their thread counts.
#[tauri::command]
fn list_folders(state: State<'_, AppState>) -> Result<Vec<FolderSummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::list_folders(&conn).map_err(|e| e.to_string())
}

/// Create a folder and return it (name is trimmed and length-capped).
#[tauri::command]
fn create_folder(state: State<'_, AppState>, name: String) -> Result<FolderSummary, String> {
    if name.len() > 256 {
        return Err("folder name exceeds maximum length".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::create_folder(&conn, &name).map_err(|e| e.to_string())
}

fn is_invalid_folder_id(id: &str) -> bool {
    id.is_empty()
        || id.len() > 64
        || id.chars().any(|c| c.is_control() || lore_core::is_zero_width(c))
}

/// Rename a folder.
#[tauri::command]
fn rename_folder(state: State<'_, AppState>, id: String, name: String) -> Result<(), String> {
    if is_invalid_folder_id(&id) {
        return Err("invalid folder id".to_string());
    }
    if name.len() > 256 {
        return Err("folder name exceeds maximum length".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::rename_folder(&conn, &id, &name).map_err(|e| e.to_string())
}

/// Delete a folder; its threads become unfiled but are not removed from Lore.
#[tauri::command]
fn delete_folder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if is_invalid_folder_id(&id) {
        return Err("invalid folder id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::delete_folder(&conn, &id).map_err(|e| e.to_string())
}

/// File a thread into a folder, replacing any prior membership. A `null`
/// `folderId` unfiles the thread.
#[tauri::command]
fn set_session_folder(
    state: State<'_, AppState>,
    session_id: String,
    folder_id: Option<String>,
) -> Result<(), String> {
    if is_invalid_text_token(&session_id, 256) {
        return Err("invalid session id".to_string());
    }
    if let Some(ref fid) = folder_id {
        if is_invalid_folder_id(fid) {
            return Err("invalid folder id".to_string());
        }
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::set_session_folder(&conn, &session_id, folder_id.as_deref())
        .map_err(|e| e.to_string())
}

/// List one newest-first page of the threads filed in a folder.
#[tauri::command]
fn list_folder_sessions_page(
    state: State<'_, AppState>,
    id: String,
    limit: i64,
    cursor: Option<String>,
) -> Result<SessionPage, String> {
    if is_invalid_folder_id(&id) {
        return Err("invalid folder id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_folder_sessions_page(&conn, &id, limit.clamp(1, 10_000), cursor.as_deref())
        .map_err(|e| e.to_string())
}

fn is_invalid_setting_key(key: &str) -> bool {
    is_invalid_text_token(key, 128)
}

/// Read a setting's raw JSON value.
#[tauri::command]
fn get_setting(state: State<'_, AppState>, key: String) -> Result<Option<String>, String> {
    if is_invalid_setting_key(&key) {
        return Ok(None);
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::settings::get(&conn, &key).map_err(|e| e.to_string())
}

/// Persist a setting's raw JSON value (Lore-owned; archive clearing preserves it).
#[tauri::command]
fn set_setting(state: State<'_, AppState>, key: String, value_json: String) -> Result<(), String> {
    if is_invalid_setting_key(&key) {
        return Err("invalid setting key".to_string());
    }
    if key.starts_with("agent_roots.") {
        return Err("agent root settings require the folder picker".to_string());
    }
    if value_json.len() > 65_536 {
        return Err("setting value exceeds maximum size".to_string());
    }
    serde_json::from_str::<serde_json::Value>(&value_json)
        .map_err(|e| format!("invalid JSON value: {e}"))?;
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::settings::set(&conn, &key, &value_json).map_err(|e| e.to_string())
}

/// Read the automatic-backup schedule (interval + retention).
#[tauri::command]
fn get_backup_schedule(state: State<'_, AppState>) -> Result<BackupScheduleDto, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let s = lore_core::backup::read_schedule(&conn).map_err(|e| e.to_string())?;
    Ok(BackupScheduleDto {
        interval: s.interval.as_str().to_string(),
        keep: i64::try_from(s.keep).unwrap_or(i64::MAX),
    })
}

/// Persist the automatic-backup schedule.
#[tauri::command]
fn set_backup_schedule(
    state: State<'_, AppState>,
    interval: String,
    keep: i64,
) -> Result<(), String> {
    if is_invalid_text_token(&interval, 64) {
        return Err("invalid backup interval".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::backup::write_schedule(
        &conn,
        lore_core::backup::BackupSchedule {
            interval: lore_core::backup::BackupInterval::parse(&interval),
            keep: usize::try_from(keep.clamp(1, 100))
                .unwrap_or(lore_core::backup::DEFAULT_BACKUP_RETENTION),
        },
    )
    .map_err(|e| e.to_string())
}

/// Create a Lore-owned backup now, pruning to the configured retention.
#[tauri::command]
fn backup_now(state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let keep = lore_core::backup::read_schedule(&conn)
        .map(|s| s.keep)
        .unwrap_or(lore_core::backup::DEFAULT_BACKUP_RETENTION);
    lore_core::backup::create_backup(&conn, &state.archive_dir.join("backups"), keep)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn replace_source_configuration(state: &AppState, config: DiscoveryConfig) -> Result<(), String> {
    let watcher = match SessionWatcher::new(&watch_roots(&state.registry, &config), WATCH_QUIET) {
        Ok(watcher) => Some(watcher),
        Err(_) => {
            eprintln!("warning: filesystem watcher unavailable; live updates disabled");
            None
        }
    };
    *state.config.lock().map_err(|_| "state lock poisoned")? = config.clone();
    let worker = state.worker.lock().map_err(|_| "state lock poisoned")?;
    let handle = worker
        .as_ref()
        .ok_or_else(|| "background ingestion worker unavailable".to_string())?;
    handle.reconfigure(config, watcher);
    Ok(())
}

/// Persist a user-selected read-only source folder, rebuild live watches, and
/// queue an incremental scan without restarting Lore.
#[tauri::command]
fn add_agent_root(
    state: State<'_, AppState>,
    agent_id: String,
    path: String,
) -> Result<(), String> {
    if is_invalid_text_token(&agent_id, 64) {
        return Err("invalid agent id".to_string());
    }
    if is_invalid_text_token(&path, 4096) {
        return Err("invalid root path".to_string());
    }
    let config = {
        let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
        lore_core::source_roots::add_custom_root(&conn, &state.registry, &agent_id, &path)
            .map_err(|e| e.to_string())?;
        app_config(&conn, &state.registry).map_err(|e| e.to_string())?
    };
    replace_source_configuration(&state, config)
}

/// Stop scanning a user-selected folder. Previously archived sessions stay in
/// Lore until the user explicitly forgets them; original logs are untouched.
#[tauri::command]
fn remove_agent_root(
    state: State<'_, AppState>,
    agent_id: String,
    path: String,
) -> Result<(), String> {
    if is_invalid_text_token(&agent_id, 64) {
        return Err("invalid agent id".to_string());
    }
    if is_invalid_text_token(&path, 4096) {
        return Err("invalid root path".to_string());
    }
    let config = {
        let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
        lore_core::source_roots::remove_custom_root(&conn, &state.registry, &agent_id, &path)
            .map_err(|e| e.to_string())?;
        app_config(&conn, &state.registry).map_err(|e| e.to_string())?
    };
    replace_source_configuration(&state, config)
}

/// Queue a discovery pass on the background worker and return immediately.
/// Ingestion progress continues through `scan_progress` events, so a manual
/// rescan never holds the UI database lock while parsing large histories.
#[tauri::command]
fn rescan(app: AppHandle, state: State<'_, AppState>) -> Result<RescanResult, String> {
    let discovered = {
        let config = state.config.lock().map_err(|_| "state lock poisoned")?;
        lore_core::discovery::discover(&state.registry, &config)
            .sessions
            .len()
    };
    let progress = ScanProgress {
        discovered: i64::try_from(discovered).unwrap_or(i64::MAX),
        done: false,
        ..ScanProgress::default()
    };
    let _ = app.emit("scan_progress", progress);
    let worker = state.worker.lock().map_err(|_| "state lock poisoned")?;
    let handle = worker
        .as_ref()
        .ok_or_else(|| "background ingestion worker unavailable".to_string())?;
    handle.trigger_rescan();

    Ok(RescanResult {
        discovered: progress.discovered,
        ingested: 0,
        skipped: 0,
        failed: 0,
        enriched: 0,
    })
}

/// Build the discovery configuration.
///
/// In release builds this combines the adapters' documented defaults with
/// persisted user-selected roots. In **debug** builds only, either adapter can
/// instead be redirected to a synthetic profile via `LORE_DEV_CLAUDE_ROOT` /
/// `LORE_DEV_CODEX_ROOT` so `cargo tauri dev` can run against generated fixtures
/// (see `lore_core::synthetic`) instead of real history. Release builds ignore
/// these variables entirely, so shipped Lore never takes an env-driven root.
fn app_config(
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
    let registry = AdapterRegistry::v0();
    let config = app_config(&conn, &registry)?;

    // Run an automatic backup at launch if one is due per the user's schedule
    // (a no-op when off or not yet due). Best-effort: a backup failure must never
    // block the app from starting.
    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        let now_ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
        if let Err(e) =
            lore_core::backup::run_scheduled_backup(&conn, &data_dir.join("backups"), now_ms)
        {
            eprintln!("warning: scheduled backup skipped: {e}");
        }
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
    let handle = worker::spawn(worker, watcher, WorkerSink::new(app.clone()));

    Ok(AppState {
        db: Mutex::new(conn),
        blobs,
        registry,
        config: Mutex::new(config),
        worker: Mutex::new(Some(handle)),
        archive_dir: data_dir,
    })
}

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
            get_git_snapshot,
            get_file_patch,
            session_secret_count,
            export_session_markdown,
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
