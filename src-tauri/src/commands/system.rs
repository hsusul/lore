use lore_core::discovery::{watch_roots, DiscoveryConfig};
use lore_core::is_invalid_text_token;
use lore_core::watcher::SessionWatcher;
use lore_ipc::{
    BackupScheduleDto, CheckUpdateResultDto, DetectedAgent, RescanResult, ScanProgress,
    TokenTotalsDto,
};
use tauri::{AppHandle, Emitter, State};

use crate::state::{app_config, AppState, WATCH_QUIET};

/// Report the archive core version.
#[tauri::command]
pub fn core_version() -> String {
    lore_core::version().to_string()
}

/// List every supported adapter using a cheap live root probe plus its
/// ingested-session count. This remains useful before the first scan.
#[tauri::command]
pub fn list_detected_agents(state: State<'_, AppState>) -> Result<Vec<DetectedAgent>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let config = state.config.lock().map_err(|_| "state lock poisoned")?;
    lore_core::source_roots::detected_agents(&conn, &state.registry, &config)
        .map_err(|e| e.to_string())
}

/// Calculate cumulative token usage and estimated cost across the entire archive.
#[tauri::command]
pub fn get_token_totals(state: State<'_, AppState>) -> Result<TokenTotalsDto, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::get_token_totals(&conn).map_err(|e| e.to_string())
}

/// Explicit, user-initiated update check (GAP-M8-02 / ADR-0005).
#[tauri::command]
pub fn check_for_updates(_state: State<'_, AppState>) -> Result<CheckUpdateResultDto, String> {
    Ok(CheckUpdateResultDto {
        update_available: false,
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        latest_version: None,
        release_notes: None,
        published_at: None,
    })
}

pub fn is_invalid_setting_key(key: &str) -> bool {
    is_invalid_text_token(key, 128)
}

/// Read a setting's raw JSON value.
#[tauri::command]
pub fn get_setting(state: State<'_, AppState>, key: String) -> Result<Option<String>, String> {
    if is_invalid_setting_key(&key) {
        return Ok(None);
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::settings::get(&conn, &key).map_err(|e| e.to_string())
}

/// Persist a setting's raw JSON value (Lore-owned; archive clearing preserves it).
#[tauri::command]
pub fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value_json: String,
) -> Result<(), String> {
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
pub fn get_backup_schedule(state: State<'_, AppState>) -> Result<BackupScheduleDto, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let s = lore_core::backup::read_schedule(&conn).map_err(|e| e.to_string())?;
    Ok(BackupScheduleDto {
        interval: s.interval.as_str().to_string(),
        keep: i64::try_from(s.keep).unwrap_or(i64::MAX),
    })
}

/// Persist the automatic-backup schedule.
#[tauri::command]
pub fn set_backup_schedule(
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
pub fn backup_now(state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let keep = lore_core::backup::read_schedule(&conn)
        .map(|s| s.keep)
        .unwrap_or(lore_core::backup::DEFAULT_BACKUP_RETENTION);
    lore_core::backup::create_backup(
        &conn,
        &state.archive_dir.join(lore_core::paths::BACKUPS_DIRNAME),
        keep,
    )
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
pub fn add_agent_root(
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
pub fn remove_agent_root(
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
pub fn rescan(app: AppHandle, state: State<'_, AppState>) -> Result<RescanResult, String> {
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

/// Ask the background worker to schedule a low-priority commit re-verification
/// pass (I2). Non-blocking: the worker enqueues and drains the coalesced jobs on
/// its own thread and connection, so this never holds the UI database lock while
/// reading git.
#[tauri::command]
pub fn reverify(state: State<'_, AppState>) -> Result<(), String> {
    let worker = state.worker.lock().map_err(|_| "state lock poisoned")?;
    let handle = worker
        .as_ref()
        .ok_or_else(|| "background ingestion worker unavailable".to_string())?;
    handle.trigger_reverify();
    Ok(())
}
