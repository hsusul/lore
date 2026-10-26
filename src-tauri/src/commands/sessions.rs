use lore_core::is_invalid_text_token;
use lore_ipc::{
    ForgetReport, GitObservationDto, MessagePage, SessionDetail, SessionPage, SessionSummary,
};
use tauri::State;

use crate::state::AppState;

/// List the most recent sessions (newest first), capped at `limit`.
#[tauri::command]
pub fn list_sessions(
    state: State<'_, AppState>,
    limit: i64,
) -> Result<Vec<SessionSummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_sessions(&conn, limit.clamp(1, 10_000)).map_err(|e| e.to_string())
}

/// List one newest-first page of sessions using an opaque keyset cursor.
#[tauri::command]
pub fn list_sessions_page(
    state: State<'_, AppState>,
    limit: i64,
    cursor: Option<String>,
) -> Result<SessionPage, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_sessions_page(&conn, limit.clamp(1, 10_000), cursor.as_deref())
        .map_err(|e| e.to_string())
}

/// Read one session in context (header, segments, ordered-part timeline, files).
#[tauri::command]
pub fn get_session(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<SessionDetail>, String> {
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
pub fn get_file_patch(state: State<'_, AppState>, id: String) -> Result<Option<String>, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid event id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::file_patch_text(&conn, &state.blobs, &id).map_err(|e| e.to_string())
}

/// Read the provenance-labeled git observations for a session.
#[tauri::command]
pub fn get_git_snapshot(
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
pub fn session_secret_count(state: State<'_, AppState>, id: String) -> Result<i64, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::secret_count(&conn, &id).map_err(|e| e.to_string())
}

/// List one stable page of messages for a session.
#[tauri::command]
pub fn list_session_messages_page(
    state: State<'_, AppState>,
    id: String,
    limit: Option<i64>,
    cursor: Option<String>,
) -> Result<MessagePage, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_session_messages_page(
        &conn,
        &id,
        limit.unwrap_or(200),
        cursor.as_deref(),
    )
    .map_err(|e| e.to_string())
}

/// Forget a session: remove its rows, projections, findings, and orphan blobs.
#[tauri::command]
pub fn forget_session(state: State<'_, AppState>, id: String) -> Result<ForgetReport, String> {
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
pub fn forget_everything(state: State<'_, AppState>) -> Result<ForgetReport, String> {
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
