use lore_core::is_invalid_text_token;
use lore_ipc::{RepositorySummary, SessionPage, SessionSummary};
use tauri::State;

use crate::state::AppState;

/// List the repositories resolved by git enrichment.
#[tauri::command]
pub fn list_repositories(state: State<'_, AppState>) -> Result<Vec<RepositorySummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_repositories(&conn).map_err(|e| e.to_string())
}

/// List the most recent sessions that touched `repository_id` (newest first).
#[tauri::command]
pub fn list_repository_sessions(
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
pub fn list_repository_sessions_page(
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

/// Relink a segment to a target repository (user correction / split-merge).
#[tauri::command]
pub fn relink_segment_repository(
    state: State<'_, AppState>,
    segment_id: String,
    repository_id: String,
) -> Result<(), String> {
    if is_invalid_text_token(&segment_id, 256) || is_invalid_text_token(&repository_id, 256) {
        return Err("invalid id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::enrich::relink_segment_repository(&conn, &segment_id, &repository_id)
        .map_err(|e| e.to_string())
}
