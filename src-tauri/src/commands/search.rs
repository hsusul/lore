use lore_ipc::{SearchHit, SearchPage};
use tauri::State;

use crate::state::AppState;

/// Full-text search over the redacted projections (secret-safe by construction).
#[tauri::command]
pub fn search(
    state: State<'_, AppState>,
    query: String,
    limit: i64,
) -> Result<Vec<SearchHit>, String> {
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
pub fn search_page(
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
