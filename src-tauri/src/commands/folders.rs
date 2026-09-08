use lore_core::is_invalid_text_token;
use lore_ipc::{FolderSummary, SessionPage};
use tauri::State;

use crate::state::AppState;

pub fn is_invalid_folder_name(name: &str) -> bool {
    name.len() > 256
        || name
            .chars()
            .any(|c| c.is_control() || lore_core::is_zero_width(c))
}

pub fn is_invalid_folder_id(id: &str) -> bool {
    is_invalid_text_token(id, 64)
}

/// List the user-defined folders with their thread counts.
#[tauri::command]
pub fn list_folders(state: State<'_, AppState>) -> Result<Vec<FolderSummary>, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::list_folders(&conn).map_err(|e| e.to_string())
}

/// Create a folder and return it (name is trimmed and length-capped).
#[tauri::command]
pub fn create_folder(state: State<'_, AppState>, name: String) -> Result<FolderSummary, String> {
    if is_invalid_folder_name(&name) {
        return Err("invalid folder name".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::create_folder(&conn, &name).map_err(|e| e.to_string())
}

/// Rename a folder.
#[tauri::command]
pub fn rename_folder(state: State<'_, AppState>, id: String, name: String) -> Result<(), String> {
    if is_invalid_folder_id(&id) {
        return Err("invalid folder id".to_string());
    }
    if is_invalid_folder_name(&name) {
        return Err("invalid folder name".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::rename_folder(&conn, &id, &name).map_err(|e| e.to_string())
}

/// Delete a folder; its threads become unfiled but are not removed from Lore.
#[tauri::command]
pub fn delete_folder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if is_invalid_folder_id(&id) {
        return Err("invalid folder id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::folders::delete_folder(&conn, &id).map_err(|e| e.to_string())
}

/// File a thread into a folder, replacing any prior membership. A `null`
/// `folderId` unfiles the thread.
#[tauri::command]
pub fn set_session_folder(
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
pub fn list_folder_sessions_page(
    state: State<'_, AppState>,
    id: String,
    limit: i64,
    cursor: Option<String>,
) -> Result<SessionPage, String> {
    if is_invalid_folder_id(&id) {
        return Err("invalid folder id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::query::list_folder_sessions_page(
        &conn,
        &id,
        limit.clamp(1, 10_000),
        cursor.as_deref(),
    )
    .map_err(|e| e.to_string())
}
