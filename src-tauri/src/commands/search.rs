//! Saved-filter + search-state Tauri commands (Phase 23, Plan 23-02).
//!
//! Every command follows the `search_messages` spawn_blocking pattern in
//! `commands/sync.rs`: lock the store, run the `store::queries` function,
//! map errors to plain-language strings. Filters apply at QUERY time only
//! (stored query re-executed against local FTS on demand) — the sync path
//! never touches these functions, and no command here opens an IMAP
//! session (live-gate: filters + state are local-SQLite, no live leg).

use tauri::State;

use crate::store::queries::{
    self, SavedFilter, SearchHit, SearchState, SORT_RANK,
};

/// Save a named filter query. `folders = None` = whole account;
/// `Some(set)` narrows to those folders. `sort` defaults to `"rank"`.
#[tauri::command]
pub async fn save_filter(
    state: State<'_, crate::AppState>,
    name: String,
    query: String,
    folders: Option<Vec<String>>,
    sort: Option<String>,
) -> Result<SavedFilter, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::create_filter(
            conn,
            &name,
            &query,
            folders.as_deref(),
            sort.as_deref().unwrap_or(SORT_RANK),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: save filter task failed ({e})"))?
}

/// List all saved filters ordered by name.
#[tauri::command]
pub async fn list_filters(
    state: State<'_, crate::AppState>,
) -> Result<Vec<SavedFilter>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::list_filters(conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: list filters task failed ({e})"))?
}

/// Rename a saved filter (backend name rules enforced).
#[tauri::command]
pub async fn rename_filter(
    state: State<'_, crate::AppState>,
    id: i64,
    name: String,
) -> Result<SavedFilter, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::rename_filter(conn, id, &name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: rename filter task failed ({e})"))?
}

/// Delete a saved filter (immediate — filters hold no mail; only the
/// filter row is removed, never mail rows).
#[tauri::command]
pub async fn delete_filter(state: State<'_, crate::AppState>, id: i64) -> Result<(), String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::delete_filter(conn, id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: delete filter task failed ({e})"))?
}

/// Apply a saved filter: re-execute its stored query against local FTS
/// now (query time). Returns ranked hits with sanitized snippets.
#[tauri::command]
pub async fn apply_filter(
    state: State<'_, crate::AppState>,
    id: i64,
) -> Result<Vec<SearchHit>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::apply_saved_filter(conn, id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: apply filter task failed ({e})"))?
}

/// Scoped search with per-hit snippets. `folders = None` searches the
/// whole account; `Some(set)` narrows. One capped local FTS query —
/// no sync wake, no IMAP traffic.
#[tauri::command]
pub async fn search_scoped(
    state: State<'_, crate::AppState>,
    query: String,
    folders: Option<Vec<String>>,
) -> Result<Vec<SearchHit>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let refs: Option<Vec<&str>> = folders
            .as_ref()
            .map(|v| v.iter().map(|s| s.as_str()).collect());
        queries::search_scoped_with_snippets(conn, refs.as_deref(), &query, SORT_RANK)
            .map_err(|e| format!("store: {e}"))
    })
    .await
    .map_err(|e| format!("internal error: scoped search task failed ({e})"))?
}

/// Persist the search view (query + folder scope + sort + active filter
/// id) on change. Empty `query` means the normal folder list view.
#[tauri::command]
pub async fn save_search_state(
    state: State<'_, crate::AppState>,
    query: String,
    folders: Option<Vec<String>>,
    sort: Option<String>,
    active_filter_id: Option<i64>,
) -> Result<(), String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::save_search_state(
            conn,
            &SearchState {
                query,
                folders,
                sort: sort.unwrap_or_else(|| SORT_RANK.to_string()),
                active_filter_id,
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: save search state task failed ({e})"))?
}

/// Load the persisted search view (`None` = never searched → render the
/// normal folder list). Called on launch and after reconnect.
#[tauri::command]
pub async fn load_search_state(
    state: State<'_, crate::AppState>,
) -> Result<Option<SearchState>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        queries::load_search_state(conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("internal error: load search state task failed ({e})"))?
}
