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

use super::key::{has_llm_api_key, LlmKeyStore};
use super::{LlmSettings, LlmSettingsInput, LlmSettingsView};
use crate::store::queries as store_queries;

/// Field-named rejection for empty-string key saves (A6 resolved: removal
/// only via the dedicated clear command, never via save).
const EMPTY_API_KEY_ERROR: &str =
    "invalid API key: value must not be empty (to remove the stored key, use the clear command)";

/// Automatic retry budget for one generation: one extra attempt after a
/// transient provider failure (429 / gateway timeout / 5xx).
const GENERATE_MAX_ATTEMPTS: u32 = 2;
/// Pause before the retry — short enough that the window never feels hung.
const GENERATE_RETRY_DELAY_MS: u64 = 600;

/// Shared read path: all models under the store `Mutex`, presence probe
/// off the runtime thread (`spawn_blocking` hard rule).
/// A locked keyring degrades to `has_api_key: false` (fail-closed).
async fn read_models(
    store: &std::sync::Mutex<crate::store::Store>,
) -> Result<Vec<LlmSettingsView>, String> {
    let models = {
        let guard = store.lock().unwrap();
        store_queries::get_llm_models(guard.conn()).map_err(|e| e.to_string())?
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
        store_queries::upsert_llm_model(guard.conn(), &validated).map_err(|e| e.to_string())?
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
        store_queries::delete_llm_model(guard.conn(), id).map_err(|e| e.to_string())?;
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
        let models = store_queries::get_llm_models(guard.conn()).map_err(|e| e.to_string())?;
        if let Some(mut current) = models.into_iter().next() {
            current.prompt_template = super::DEFAULT_TEMPLATE.to_string();
            let validated = super::validate::validate(&current).map_err(|e| e.to_string())?;
            store_queries::upsert_llm_model(guard.conn(), &validated).map_err(|e| e.to_string())?;
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
        let models = store_queries::get_llm_models(guard.conn()).map_err(|e| e.to_string())?;
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

/// Cancel the in-flight LLM generation for a mailbox (LLM-08 cancel support).
///
/// Cancels on the app-wide tracker held in [`crate::AppState`] — the same
/// tracker the generation command registered on — so the cancel actually
/// reaches the running request. Returns `true` when a request was pending
/// and is now cancelled (`false` means it had already finished).
#[tauri::command]
pub async fn cancel_generate(
    state: tauri::State<'_, crate::AppState>,
    mailbox: String,
) -> Result<bool, String> {
    Ok(state.llm_tracker.cancel_request(&mailbox))
}

/// Generate a reply draft via `POST /chat/completions` (non-streaming).
///
/// Behaviour (Phase 38 fix — the original implementation answered from a
/// hardcoded `INBOX` UID 1 and tracked requests per call, so every reply
/// was generated from the wrong message and cancel was a no-op):
///
/// - the original message is the one the window is replying to (`uid` +
///   `mailbox`), read from the local BODY.PEEK cache — never a server
///   call, never a `\Seen` write;
/// - single-flight and cancel share [`crate::AppState::llm_tracker`];
/// - the key is loaded inside `spawn_blocking` (keyring hard rule) and
///   never leaves this frame — errors carry metadata only (T-39-01);
/// - transient provider failures (429 / 408+504 / 5xx) get one retry.
///
/// `subject`/`body` are the compose window's *current* state and are used
/// only when the cache holds no better original body.
#[tauri::command]
pub async fn generate_reply(
    state: tauri::State<'_, crate::AppState>,
    mailbox: Option<String>,
    uid: Option<u32>,
    request_id: Option<u64>,
    subject: Option<String>,
    body: Option<String>,
) -> Result<String, String> {
    // Kept for IPC compatibility (the window sends `request_id: null`);
    // the authoritative id is minted by the shared tracker below.
    let _ = request_id;

    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());

    // 1. Settings (store mutex only — no network, no keyring).
    let settings = {
        let guard = state.store.lock().unwrap();
        let models = store_queries::get_llm_models(guard.conn()).map_err(|e| e.to_string())?;
        models.into_iter().next().unwrap_or_default()
    };
    if settings.base_url.trim().is_empty() || settings.model.trim().is_empty() {
        return Err(super::client::LlmRequestError::NotConfigured.to_string());
    }

    // 2. Original message context from the local cache (BODY.PEEK data).
    let ctx = {
        let guard = state.store.lock().unwrap();
        match uid {
            Some(uid) => store_queries::get_reply_context(guard.conn(), &mailbox, uid)
                .map_err(|e| e.to_string())?,
            None => None,
        }
    };
    let ctx = ctx.ok_or_else(|| {
        "Mensagem original não encontrada no cache local — abra a mensagem antes de gerar a resposta."
            .to_string()
    })?;

    // 3. API key: presence + load in one blocking hop (hard rule).
    let api_key = tauri::async_runtime::spawn_blocking(|| {
        super::key::KeyringLlmKeyStore::new()
            .load_secret()
            .ok()
            .flatten()
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?
    .ok_or_else(|| super::client::LlmRequestError::NotConfigured.to_string())?;

    // 4. Prompt inputs from the cached original + the window's current state.
    let (sender, subject_text, original_body) =
        prompt_inputs(&ctx, subject.as_deref(), body.as_deref());
    if original_body.trim().is_empty() {
        return Err(
            "Mensagem original sem conteúdo no cache local — abra a mensagem antes de gerar a resposta."
                .to_string(),
        );
    }

    let rendered_prompt = super::prompt::render_prompt(
        &settings.prompt_template,
        &sender,
        &subject_text,
        &original_body,
    );

    // 5. Single-flight on the shared tracker: a second generation for the
    //    same mailbox is refused while one is running.
    let tracker = &state.llm_tracker;
    let rid = tracker.start_request(&mailbox).ok_or_else(|| {
        "Já existe uma geração em andamento para esta mensagem — cancele antes de gerar de novo."
            .to_string()
    })?;

    // 6. Request with one automatic retry on transient failures.
    let mut attempt: u32 = 0;
    loop {
        if !tracker.is_owner(&mailbox, rid) {
            return Err(super::client::LlmRequestError::Cancelled.to_string());
        }
        attempt += 1;
        match super::client::generate_reply_with_key(&settings, &rendered_prompt, &api_key).await {
            Ok(generation) => {
                if !tracker.is_owner(&mailbox, rid) {
                    return Err(super::client::LlmRequestError::Cancelled.to_string());
                }
                tracker.end_request(&mailbox);
                return Ok(generation.draft_text);
            }
            Err(e) if e.is_retryable() && attempt < GENERATE_MAX_ATTEMPTS => {
                async_std::task::sleep(std::time::Duration::from_millis(GENERATE_RETRY_DELAY_MS))
                    .await;
            }
            Err(e) => {
                // Only release the slot we still own: a cancel (or a newer
                // request) must not be clobbered by this late failure.
                if tracker.is_owner(&mailbox, rid) {
                    tracker.end_request(&mailbox);
                }
                return Err(e.to_string());
            }
        }
    }
}

/// Pick the `(sender, subject, body)` the prompt is rendered from.
///
/// The cached original wins for the sender and the subject — the prompt is
/// about the *received* message, not the user's "Re:" edit. The body prefers
/// the cached sanitized text, then the compose window's body (which carries
/// the quoted original), then the header preview (bodies over the cache cap
/// are never stored). Never returns an empty sender.
fn prompt_inputs(
    ctx: &store_queries::ReplyContextRow,
    subject: Option<&str>,
    body: Option<&str>,
) -> (String, String, String) {
    let sender = if ctx.sender.trim().is_empty() {
        "remetente desconhecido".to_string()
    } else {
        ctx.sender.trim().to_string()
    };
    let subject_text = non_empty(Some(ctx.subject.as_str()))
        .or_else(|| non_empty(subject))
        .unwrap_or_default();
    let original_body = non_empty(ctx.body_text.as_deref())
        .or_else(|| non_empty(body))
        .or_else(|| non_empty(Some(ctx.preview.as_str())))
        .unwrap_or_default();
    (sender, subject_text, original_body)
}

/// `Some(trimmed)` when the value holds non-whitespace text.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(
        sender: &str,
        subject: &str,
        preview: &str,
        body_text: Option<&str>,
    ) -> store_queries::ReplyContextRow {
        store_queries::ReplyContextRow {
            sender: sender.to_string(),
            subject: subject.to_string(),
            cc_addrs_json: "[]".to_string(),
            message_id: Some("<orig@example.com>".to_string()),
            preview: preview.to_string(),
            body_text: body_text.map(str::to_string),
        }
    }

    #[test]
    fn prompt_inputs_prefer_the_cached_original() {
        let c = ctx(
            "ana@example.com",
            "Prova de cálculo",
            "trecho…",
            Some("corpo completo"),
        );
        let (sender, subject, body) =
            prompt_inputs(&c, Some("Re: Prova de cálculo"), Some("> citado"));
        assert_eq!(sender, "ana@example.com");
        assert_eq!(
            subject, "Prova de cálculo",
            "the received subject, not the Re: edit"
        );
        assert_eq!(body, "corpo completo", "cached body beats the quote");
    }

    #[test]
    fn prompt_inputs_fall_back_to_the_compose_body_then_preview() {
        // No cached body (over the cache cap): the window body carries the
        // quoted original and beats the header preview.
        let c = ctx("ana@example.com", "Assunto", "preview curto", None);
        let (_, _, body) = prompt_inputs(&c, None, Some("\n\nEm 10/10, ana escreveu:\n> oi"));
        assert!(body.contains("> oi"), "got: {body}");

        // Nothing from the window either: the preview is the last resort.
        let (_, subject, body) = prompt_inputs(&c, None, None);
        assert_eq!(body, "preview curto");
        assert_eq!(subject, "Assunto");
    }

    #[test]
    fn prompt_inputs_never_yield_an_empty_sender() {
        let c = ctx("   ", "X", "Y", None);
        let (sender, _, _) = prompt_inputs(&c, None, None);
        assert_eq!(sender, "remetente desconhecido");
    }

    #[test]
    fn prompt_inputs_without_any_body_signal_an_empty_string() {
        // The caller turns this into a plain-language refusal instead of
        // sending an empty prompt to the provider.
        let c = ctx("ana@example.com", "  ", "   ", None);
        let (_, subject, body) = prompt_inputs(&c, Some("  "), Some(""));
        assert!(body.trim().is_empty());
        assert!(subject.is_empty(), "whitespace-only values are dropped");
    }
}
