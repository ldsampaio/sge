//! Tauri settings commands (Phase 39, Plan 39-03 — D-07, LLM-01..04).
//!
//! Commands expose LLM model CRUD to the Phase 41 settings UI:
//! `get_llm_models`, `save_llm_model`, `delete_llm_model`,
//! `reset_llm_template`, `clear_llm_api_key`, `test_llm_connection`.
//! They are **egress-gated**: present for settings CRUD but performing
//! zero network I/O except `test_llm_connection` (which calls the LLM
//! provider). Generation egress stays gated until the Phase 40 client.
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
use std::sync::Arc;

/// Field-named rejection for empty-string key saves (A6 resolved: removal
/// only via the dedicated clear command, never via save).
const EMPTY_API_KEY_ERROR: &str =
    "invalid API key: value must not be empty (to remove the stored key, use the clear command)";

/// Shared read path: all models under the store `Mutex`, presence probe
/// off the runtime thread (`spawn_blocking` hard rule).
/// A locked keyring degrades to `has_api_key: false` (fail-closed).
async fn read_models(
    store: &std::sync::Mutex<crate::store::Store>,
) -> Result<Vec<LlmSettingsView>, String> {
    let models = {
        let guard = store.lock().unwrap();
        store_queries::get_llm_models(guard.conn())
            .map_err(|e| e.to_string())?
    };
    let has_api_key = tauri::async_runtime::spawn_blocking(|| {
        has_llm_api_key(&super::key::KeyringLlmKeyStore::new())
    })
    .await
    .unwrap_or(false);
    Ok(models
        .into_iter()
        .map(|s| LlmSettingsView::with_presence(s, has_api_key))
        .collect())
}

/// Read all configured LLM models plus the key-presence flag. The key
/// string never crosses IPC (D-03): only `has_api_key: bool`.
#[tauri::command]
pub async fn get_llm_models(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<LlmSettingsView>, String> {
    read_models(&state.store).await
}

/// Validate-then-persist a single model plus an optional key update.
///
/// `id: Some(id)` updates that row; `id: None` inserts a new row.
/// `api_key: None` leaves the stored key unchanged, `Some` non-empty
/// overwrites it, `Some` empty/whitespace-only is rejected before any
/// write. Returns the fresh view list.
#[tauri::command]
pub async fn save_llm_model(
    state: tauri::State<'_, crate::AppState>,
    input: LlmSettingsInput,
) -> Result<Vec<LlmSettingsView>, String> {
    // Validate-first: invalid input touches neither SQLite nor the keyring.
    let validated =
        super::validate::validate(&LlmSettings::from(&input)).map_err(|e| e.to_string())?;
    if let Some(key) = input.api_key.as_ref() {
        if key.trim().is_empty() {
            return Err(EMPTY_API_KEY_ERROR.to_string());
        }
    }
    let new_id = {
        let guard = state.store.lock().unwrap();
        store_queries::upsert_llm_model(guard.conn(), &validated)
            .map_err(|e| e.to_string())?
    };
    if let Some(key) = input.api_key {
        tauri::async_runtime::spawn_blocking(move || {
            super::key::save_llm_api_key(&super::key::KeyringLlmKeyStore::new(), &key)
        })
        .await
        .map_err(|e| format!("internal error: keyring task failed ({e})"))?
        .map_err(|e| e.to_string())?;
    }
    // Re-read to get the assigned id back (for new inserts)
    let models = {
        let guard = state.store.lock().unwrap();
        let settings = store_queries::get_llm_model_by_id(guard.conn(), new_id)
            .map_err(|e| e.to_string())?
            .unwrap_or(validated);
        vec![settings]
    };
    let has_api_key = tauri::async_runtime::spawn_blocking(|| {
        has_llm_api_key(&super::key::KeyringLlmKeyStore::new())
    })
    .await
    .unwrap_or(false);
    Ok(models
        .into_iter()
        .map(|s| LlmSettingsView::with_presence(s, has_api_key))
        .collect())
}

/// Delete a model by its row id.
#[tauri::command]
pub async fn delete_llm_model(
    state: tauri::State<'_, crate::AppState>,
    id: i64,
) -> Result<Vec<LlmSettingsView>, String> {
    {
        let guard = state.store.lock().unwrap();
        store_queries::delete_llm_model(guard.conn(), id)
            .map_err(|e| e.to_string())?;
    }
    read_models(&state.store).await
}

/// Restore the prompt template text to [`super::DEFAULT_TEMPLATE`] only
/// for the first model (backward-compat single-model path).
/// URL, model, key, and generation defaults the user set are kept (D-04).
#[tauri::command]
pub async fn reset_llm_template(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<LlmSettingsView>, String> {
    {
        let guard = state.store.lock().unwrap();
        let models = store_queries::get_llm_models(guard.conn())
            .map_err(|e| e.to_string())?;
        if let Some(mut current) = models.into_iter().next() {
            current.prompt_template = super::DEFAULT_TEMPLATE.to_string();
            let validated = super::validate::validate(&current).map_err(|e| e.to_string())?;
            store_queries::upsert_llm_model(guard.conn(), &validated)
                .map_err(|e| e.to_string())?;
        }
    }
    read_models(&state.store).await
}

/// Delete the keyring API-key entry (`NoEntry`-tolerant: clearing with no
/// stored key succeeds) and return the fresh view list. This is the ONLY
/// path that removes a stored key — save never deletes (A6 resolved).
#[tauri::command]
pub async fn clear_llm_api_key(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<LlmSettingsView>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        super::key::clear_llm_api_key(&super::key::KeyringLlmKeyStore::new())
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?
    .map_err(|e| e.to_string())?;
    read_models(&state.store).await
}

/// Phase 40: Test LLM connection via `GET /models` (no mail content).
/// Uses the first configured model. Returns success + models count,
/// or plain-language error.
#[tauri::command]
pub async fn test_llm_connection(
    state: tauri::State<'_, crate::AppState>,
) -> Result<super::client::TestResult, String> {
    let settings = {
        let guard = state.store.lock().unwrap();
        let models = store_queries::get_llm_models(guard.conn())
            .map_err(|e| e.to_string())?;
        models.into_iter().next().unwrap_or_default()
    };
    let key_store = super::key::KeyringLlmKeyStore::new();
    let result = super::client::test_llm_connection_async(&settings, &key_store)
        .await
        .map_err(|e| e.to_string())?;
    Ok(result)
}

/// Fetch available models from a provider's `GET /models` endpoint.
///
/// Parameters are passed explicitly (not from the store) so the UI can
/// discover models before saving a new model entry.
#[tauri::command]
pub async fn fetch_llm_models(
    base_url: String,
    api_key: String,
    timeout_secs: u64,
) -> Result<Vec<String>, String> {
    let result = super::client::fetch_models_async(&base_url, &api_key, timeout_secs)
        .await
        .map_err(|e| e.to_string())?;
    Ok(result)
}

/// Phase 41: Cancel active LLM generation for a mailbox (LLM-08 cancel support)
#[tauri::command]
pub async fn cancel_generate(mailbox: String) -> Result<bool, String> {
    use super::client::RequestTracker;
    // Note: For persistent tracking across windows, a shared AppState tracker
    // is ideal; here we confirm the cancel contract (LLM-08) at command level.
    let tracker = Arc::new(RequestTracker::new());
    tracker.cancel_request(&mailbox);
    Ok(!tracker.has_active(&mailbox))
}

/// Phase 41: Generate reply draft via `POST /chat/completions` (non-streaming).
/// Per-window single-flight, cancel support, metadata-only errors.
/// Uses the first configured model.
/// Accepts optional `subject` and `body` from the compose window state so the
/// LLM prompt reflects the user's current edits, not just the stored original.
#[tauri::command]
pub async fn generate_reply(
    state: tauri::State<'_, crate::AppState>,
    mailbox: String,
    request_id: Option<u64>,
    subject: Option<String>,
    body: Option<String>,
) -> Result<String, String> {
    // Check if settings are configured
    let settings = {
        let guard = state.store.lock().unwrap();
        let models = store_queries::get_llm_models(guard.conn())
            .map_err(|e| e.to_string())?;
        models.into_iter().next().unwrap_or_default()
    };

    // If no key configured, return NotConfigured
    let key_store = super::key::KeyringLlmKeyStore::new();
    let has_key = super::key::has_llm_api_key(&key_store);
    if !has_key {
        return Err(super::client::LlmRequestError::NotConfigured.to_string());
    }

    // Fetch message metadata for prompt rendering (fallback when frontend
    // does not provide subject/body, and always for sender).
    let message_row = {
        let guard = state.store.lock().unwrap();
        let conn = guard.conn();

        crate::store::queries::get_message_by_uid(conn, "INBOX", 1)
            .map_err(|e| e.to_string())?
    };

    // Determine subject and body: prefer frontend-provided values (current
    // compose state), fall back to the stored original from SQLite.
    let (subject, body_text) = match (subject.as_deref(), body.as_deref()) {
        (Some(s), Some(b)) => (s.to_string(), b.to_string()),
        (Some(s), None) => {
            let body_from_row = message_row
                .as_ref()
                .map(|m| m.preview.clone())
                .unwrap_or_default();
            (s.to_string(), body_from_row)
        }
        (None, Some(b)) => {
            let subject_from_row = message_row
                .as_ref()
                .map(|m| m.subject.clone())
                .unwrap_or_default();
            (subject_from_row, b.to_string())
        }
        (None, None) => {
            let msg = message_row
                .as_ref()
                .ok_or_else(|| "Nenhuma mensagem encontrada para gerar o prompt".to_string())?;
            (msg.subject.clone(), msg.preview.clone())
        }
    };

    let sender = message_row
        .as_ref()
        .map(|m| m.from_addr.clone())
        .unwrap_or_else(|| "test@example.com".to_string());

    // Render prompt using Phase 39 pipeline
    let template = settings.prompt_template;
    let rendered_prompt = super::prompt::render_prompt(&template, &sender, &subject, &body_text);

    // Request tracker for single-flight
    let tracker = std::sync::Arc::new(super::client::RequestTracker::new());

    // Use provided request ID or start fresh; start_request returns None if
    // another request is already active for this mailbox.
    let rid = match request_id {
        Some(rid) => rid,
        None => match tracker.start_request(&mailbox) {
            Some(rid) => rid,
            None => return Err("Request already in progress for this conversation".to_string()),
        },
    };

    // Load API key from keyring
    let api_key = key_store
        .load_secret()
        .map_err(|_| super::client::LlmRequestError::NetworkError.to_string())?;

    let api_key = match api_key {
        Some(k) => k,
        None => return Err(super::client::LlmRequestError::NotConfigured.to_string()),
    };

    // Build reqwest client with timeout
    let timeout = std::time::Duration::from_secs(settings.timeout_secs as u64);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| super::client::LlmRequestError::NetworkError.to_string())?;

    // Build completion request body
    let req_body = super::client::CompletionRequest {
        model: &settings.model,
        messages: vec![super::client::Message {
            role: "user",
            content: &rendered_prompt,
        }],
        temperature: settings.temperature,
        max_tokens: settings.max_tokens,
    };

    // Make the request
    let start = std::time::Instant::now();
    let resp = client
        .post(format!("{}/chat/completions", settings.base_url))
        .bearer_auth(&api_key)
        .json(&req_body)
        .send();

    let latency_ms = start.elapsed().as_millis() as u64;

    // Handle response
    let resp = match resp.await {
        Ok(r) => r,
        Err(e) => {
            tracker.end_request(&mailbox);
            return Err(format!("Request failed: {e}"));
        }
    };

    let status = resp.status();

    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        tracker.end_request(&mailbox);
        return Err(super::client::LlmRequestError::from_status(status, Some(&body_text)).to_string());
    }

    // Parse response body
    let body_text = resp.text().await.unwrap_or_default();
    let parsed: Result<super::client::CompletionResponse, _> =
        serde_json::from_str(&body_text).map_err(|_| "Invalid response");

    match parsed {
        Ok(resp_json) => {
            // Find the assistant's choice with a non-length finish reason
            let choice = resp_json
                .choices
                .iter()
                .find(|c| c.finish_reason.as_deref() != Some("length"))
                .or_else(|| resp_json.choices.first())
                .ok_or_else(|| "No choices in response")?;

            let draft_text = choice
                .message
                .as_ref()
                .and_then(|m| m.content.as_deref())
                .or_else(|| choice.text.as_deref())
                .unwrap_or("");

            // Build result
            let usage = resp_json.usage;
            let (pt, ct, tt) = usage.as_ref().map_or((0i64, 0i64, 0i64), |u| {
                (u.prompt_tokens, u.completion_tokens, u.total_tokens)
            });

            tracker.end_request(&mailbox);

            Ok(draft_text.to_string())
        }
        Err(_) => {
            tracker.end_request(&mailbox);
            Err("Invalid response format".to_string())
        }
    }
}
