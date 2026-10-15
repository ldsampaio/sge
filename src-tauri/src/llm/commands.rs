//! Tauri settings commands (Phase 39, Plan 39-03 — D-07, LLM-01..04).
//!
//! Four commands expose Plans 39-01/39-02 to the Phase 41 settings UI:
//! `get_llm_settings`, `save_llm_settings`, `reset_llm_template`,
//! `clear_llm_api_key`. They are **egress-gated**: present for settings
//! CRUD but performing zero network I/O — no `reqwest::Client`
//! construction, no `.send()`, no `Url` fetch beyond the pure parse already
//! in [`crate::llm::validate`]. Generation egress stays gated until the
//! Phase 40 client.
//!
//! Conventions copied from the `lib.rs` precedents: validate-first (before
//! any SQLite or keyring write, so invalid input touches nothing),
//! keyring I/O only inside `spawn_blocking` (hard rule), join errors via
//! `internal error: keyring task failed ({e})`, SQLite behind the store
//! `Mutex` (single-writer invariant), and `StoreUnavailable` degrading to
//! `Ok(false)` presence on the get path. Errors and views carry metadata
//! only — the key string and mail bodies never appear (T-39-01).

use super::key::{LlmKeyStore, has_llm_api_key};
use super::{LlmSettings, LlmSettingsInput, LlmSettingsView};
use crate::store::queries as store_queries;

/// Field-named rejection for empty-string key saves (A6 resolved: removal
/// only via the dedicated clear command, never via save).
const EMPTY_API_KEY_ERROR: &str =
    "invalid API key: value must not be empty (to remove the stored key, use the clear command)";

/// Join the stored row with a presence flag into one view. `None` rows
/// (pre-M23 databases) read as defaults so fresh and upgraded installs
/// agree.
fn view_of(
    conn: &rusqlite::Connection,
    has_api_key: bool,
) -> Result<LlmSettingsView, String> {
    let settings = store_queries::get_llm_settings(conn)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    Ok(LlmSettingsView::with_presence(settings, has_api_key))
}

/// Shared read path for the async commands: row under the store `Mutex`,
/// presence probe off the runtime thread (`spawn_blocking` hard rule).
/// A locked keyring degrades to `has_api_key: false` (fail-closed: the UI
/// prompts for a key instead of erroring).
async fn read_view(
    store: &std::sync::Mutex<crate::store::Store>,
) -> Result<LlmSettingsView, String> {
    let settings = {
        let guard = store.lock().unwrap();
        store_queries::get_llm_settings(guard.conn())
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
    };
    let has_api_key = tauri::async_runtime::spawn_blocking(|| {
        has_llm_api_key(&super::key::KeyringLlmKeyStore::new())
    })
    .await
    .unwrap_or(false);
    Ok(LlmSettingsView::with_presence(settings, has_api_key))
}

/// Read all non-secret settings plus the key-presence flag. The key string
/// never crosses IPC (D-03): only `has_api_key: bool` leaves the backend.
#[tauri::command]
pub async fn get_llm_settings(
    state: tauri::State<'_, crate::AppState>,
) -> Result<LlmSettingsView, String> {
    read_view(&state.store).await
}

// Phase 40 handoff: `timeout_secs` is accepted unclamped here (validate-only
// per D-06 — any positive value persists as-is). The effective request
// abort and any user-facing cancel are owned by the Phase 40 generation
// client, not by these settings commands.
/// Validate-then-persist settings plus an optional key update.
///
/// `api_key: None` leaves the stored key unchanged, `Some` non-empty
/// overwrites it, `Some` empty/whitespace-only is rejected before any
/// write. Returns the fresh view (re-read row + presence probe).
#[tauri::command]
pub async fn save_llm_settings(
    state: tauri::State<'_, crate::AppState>,
    input: LlmSettingsInput,
) -> Result<LlmSettingsView, String> {
    // Validate-first: invalid input touches neither SQLite nor the keyring.
    let validated =
        super::validate::validate(&LlmSettings::from(&input)).map_err(|e| e.to_string())?;
    if let Some(key) = input.api_key.as_ref() {
        if key.trim().is_empty() {
            return Err(EMPTY_API_KEY_ERROR.to_string());
        }
    }
    {
        let guard = state.store.lock().unwrap();
        store_queries::upsert_llm_settings(guard.conn(), &validated)
            .map_err(|e| e.to_string())?;
    }
    if let Some(key) = input.api_key {
        tauri::async_runtime::spawn_blocking(move || {
            super::key::save_llm_api_key(&super::key::KeyringLlmKeyStore::new(), &key)
        })
        .await
        .map_err(|e| format!("internal error: keyring task failed ({e})"))?
        .map_err(|e| e.to_string())?;
    }
    read_view(&state.store).await
}

/// Restore the prompt template text to [`super::DEFAULT_TEMPLATE`] only.
/// URL, model, key, and generation defaults the user set are kept (D-04).
#[tauri::command]
pub async fn reset_llm_template(
    state: tauri::State<'_, crate::AppState>,
) -> Result<LlmSettingsView, String> {
    {
        let guard = state.store.lock().unwrap();
        let validated = apply_reset_template(guard.conn())?;
        store_queries::upsert_llm_settings(guard.conn(), &validated)
            .map_err(|e| e.to_string())?;
    }
    read_view(&state.store).await
}

/// Delete the keyring API-key entry (`NoEntry`-tolerant: clearing with no
/// stored key succeeds) and return the fresh view. This is the ONLY path
/// that removes a stored key — save never deletes (A6 resolved).
#[tauri::command]
pub async fn clear_llm_api_key(
    state: tauri::State<'_, crate::AppState>,
) -> Result<LlmSettingsView, String> {
    tauri::async_runtime::spawn_blocking(|| {
        super::key::clear_llm_api_key(&super::key::KeyringLlmKeyStore::new())
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?
    .map_err(|e| e.to_string())?;
    read_view(&state.store).await
}

/// Template-only reset against a raw connection (shared by the command
/// and the unit tests): reads the current row, restores only
/// `prompt_template` to the default, and returns the validated row with
/// the other five columns unchanged. Callers persist via
/// `upsert_llm_settings` under the store `Mutex` themselves.
fn apply_reset_template(conn: &rusqlite::Connection) -> Result<LlmSettings, String> {
    let mut current = store_queries::get_llm_settings(conn)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    current.prompt_template = super::DEFAULT_TEMPLATE.to_string();
    // The default literal is always valid; still route through `validate`
    // so a future const drift fails closed instead of persisting garbage.
    super::validate::validate(&current).map_err(|e| e.to_string())
}

/// Key-semantics probe shared by the command and the unit tests:
/// classify an incoming `api_key` WITHOUT touching any store.
/// `None` = leave unchanged, `Some` non-empty = overwrite allowed,
/// `Some` empty/whitespace-only = rejected.
fn classify_api_key(api_key: &Option<String>) -> Result<bool, String> {
    match api_key {
        None => Ok(false),
        Some(key) if key.trim().is_empty() => Err(EMPTY_API_KEY_ERROR.to_string()),
        Some(_) => Ok(true),
    }
}

/// Synchronous save path for unit tests (memory store + memory keyring):
/// validate-first, then SQLite write, then key handling per [`classify_api_key`].
#[cfg(test)]
fn apply_save(
    conn: &rusqlite::Connection,
    key_store: &impl LlmKeyStore,
    input: &LlmSettingsInput,
) -> Result<LlmSettingsView, String> {
    let validated =
        super::validate::validate(&LlmSettings::from(input)).map_err(|e| e.to_string())?;
    let overwrite = classify_api_key(&input.api_key)?;
    store_queries::upsert_llm_settings(conn, &validated).map_err(|e| e.to_string())?;
    if overwrite {
        let key = input.api_key.as_ref().expect("classified Some");
        super::key::save_llm_api_key(key_store, key).map_err(|e| e.to_string())?;
    }
    view_of(conn, has_llm_api_key(key_store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use super::super::key::MemoryLlmKeyStore;
    use super::super::{DEFAULT_MODEL, DEFAULT_TEMPLATE};

    const SEED_URL: &str = "https://api.openai.com/v1";

    fn edited_input(api_key: Option<&str>) -> LlmSettingsInput {
        LlmSettingsInput {
            base_url: "http://localhost:11434".to_string(),
            model: "llama3".to_string(),
            temperature: 0.1,
            max_tokens: 100,
            timeout_secs: 30,
            prompt_template: "S: {sender} Sub: {subject} B: {body}".to_string(),
            api_key: api_key.map(str::to_string),
        }
    }

    #[test]
    fn save_with_invalid_url_leaves_stored_row_unchanged() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let keys = MemoryLlmKeyStore::new();
        let before = apply_save(store.conn(), &keys, &edited_input(None))
            .expect("seed save should succeed");
        assert_eq!(before.base_url, "http://localhost:11434");
        let mut bad = edited_input(None);
        bad.base_url = "http://example.com".to_string();
        let err = apply_save(store.conn(), &keys, &bad).unwrap_err();
        assert!(
            err.contains("base URL"),
            "rejection must name the field: {err}"
        );
        let after = view_of(store.conn(), false).expect("read should succeed");
        assert_eq!(after.base_url, before.base_url, "failed save must not touch the row");
        assert_eq!(after.model, before.model);
        assert_eq!(after.prompt_template, before.prompt_template);
    }

    #[test]
    fn save_with_api_key_none_preserves_prior_presence_true() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let keys = MemoryLlmKeyStore::new();
        let saved = apply_save(store.conn(), &keys, &edited_input(Some("sk-test-persist")))
            .expect("keyed save should succeed");
        assert!(saved.has_api_key, "saved key must report presence");
        let again = apply_save(store.conn(), &keys, &edited_input(None))
            .expect("keyless save should succeed");
        assert!(
            again.has_api_key,
            "api_key None must leave the stored key unchanged"
        );
        assert_eq!(
            super::super::key::load_llm_api_key(&keys).unwrap().as_deref(),
            Some("sk-test-persist"),
            "stored key bytes must be untouched"
        );
    }

    #[test]
    fn save_with_empty_api_key_errors_naming_key_and_writes_nothing() {
        for empty in ["", "   "] {
            let store = Store::open_in_memory().expect("migration should succeed");
            let keys = MemoryLlmKeyStore::new();
            let mut input = edited_input(None);
            input.api_key = Some(empty.to_string());
            let err = apply_save(store.conn(), &keys, &input).unwrap_err();
            assert!(
                err.contains("API key"),
                "rejection must name the API key: {err}"
            );
            let row = view_of(store.conn(), false).expect("read should succeed");
            assert_eq!(row.base_url, SEED_URL, "rejected save must not touch the row");
            assert_eq!(row.model, DEFAULT_MODEL);
            assert!(
                !has_llm_api_key(&keys),
                "rejected save must not create a key entry"
            );
        }
    }

    #[test]
    fn save_valid_input_round_trips_view_with_presence() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let keys = MemoryLlmKeyStore::new();
        let view = apply_save(store.conn(), &keys, &edited_input(Some("sk-test-view")))
            .expect("valid save should succeed");
        assert_eq!(view.base_url, "http://localhost:11434");
        assert_eq!(view.model, "llama3");
        assert_eq!(view.temperature, 0.1);
        assert_eq!(view.max_tokens, 100);
        assert_eq!(view.timeout_secs, 30);
        assert!(view.has_api_key, "saved key must surface as presence-bool");
        // The view carries no key field by construction: its Debug form
        // must not contain the secret.
        let debug = format!("{view:?}");
        assert!(!debug.contains("sk-test-view"), "view must not carry key material");
    }

    #[test]
    fn reset_restores_template_only_keeping_url_and_model() {
        let store = Store::open_in_memory().expect("migration should succeed");
        apply_save(store.conn(), &MemoryLlmKeyStore::new(), &edited_input(None))
            .expect("seed save should succeed");
        let reset = apply_reset_template(store.conn()).expect("reset should succeed");
        store_queries::upsert_llm_settings(store.conn(), &reset).expect("persist should succeed");
        let back = view_of(store.conn(), false).expect("read should succeed");
        assert_eq!(
            back.prompt_template, DEFAULT_TEMPLATE,
            "reset must restore the default template"
        );
        assert_eq!(back.base_url, "http://localhost:11434", "reset must keep the URL");
        assert_eq!(back.model, "llama3", "reset must keep the model");
        assert_eq!(back.temperature, 0.1, "reset must keep generation defaults");
        assert_eq!(back.max_tokens, 100);
        assert_eq!(back.timeout_secs, 30);
    }

    #[test]
    fn clear_removes_key_and_returns_keyless_view() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let keys = MemoryLlmKeyStore::new();
        apply_save(store.conn(), &keys, &edited_input(Some("sk-test-clear")))
            .expect("keyed save should succeed");
        assert!(has_llm_api_key(&keys));
        super::super::key::clear_llm_api_key(&keys).expect("clear should succeed");
        let view = view_of(store.conn(), has_llm_api_key(&keys))
            .expect("read should succeed");
        assert!(!view.has_api_key, "cleared key must report absence");
        assert_eq!(view.model, "llama3", "clear must not touch the settings row");
        // Clearing with no entry succeeds (NoEntry-tolerant contract).
        super::super::key::clear_llm_api_key(&keys).expect("second clear must succeed");
    }

    #[test]
    fn input_debug_redacts_key_material() {
        let input = edited_input(Some("sk-test-debug-secret"));
        let debug = format!("{input:?}");
        assert!(!debug.contains("sk-test-debug-secret"), "Debug must redact the key");
        assert!(debug.contains("<redacted>"), "Debug must show presence only");
    }
}
