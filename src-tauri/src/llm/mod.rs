//! LLM settings lane (Phase 39, Plan 39-01 — LLM-01/LLM-04 storage + validation).
//!
//! Non-secret generation settings persist in the M23 single-row
//! `llm_settings` table via [`crate::store::queries`]. The API key lives
//! only in the OS keyring (Plan 39-02) and never enters [`LlmSettings`] —
//! there is no key field, no key column, and no `Debug` output holding a
//! key (T-39-01). This lane performs zero network I/O: no `reqwest::Client`
//! construction, no `.send()` — generation egress lands in Phase 40.

pub mod client;
pub mod commands;
pub mod key;
pub mod prompt;
pub mod validate;

use serde::{Deserialize, Serialize};

/// Default base URL (OpenAI-class endpoint). Seeded into the M23 row on
/// fresh install; the M23 seed test asserts the seed equals this const.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default model name (OpenAI-class default).
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
/// Default sampling temperature (OpenAI 0.0–2.0 range).
pub const DEFAULT_TEMPERATURE: f64 = 0.7;
/// Default max output tokens.
pub const DEFAULT_MAX_TOKENS: i64 = 500;
/// Default request timeout in seconds (validate-only, unclamped above 1).
pub const DEFAULT_TIMEOUT_SECS: i64 = 60;
/// Default prompt template. Each of `{sender}`, `{subject}`, `{body}`
/// appears exactly once; rendering (substitution, fencing, truncation)
/// lands in Plan 39-02 — this plan treats the template as opaque data.
///
/// Single-line, no single quotes: the same literal is embedded in the M23
/// seed `INSERT`, where SQL string literals cannot express newlines.
pub const DEFAULT_TEMPLATE: &str =
    "Write a helpful reply to the email below. From: {sender} Subject: {subject} Body: {body}";

/// Validated, non-secret LLM generation settings (one row per model in `llm_models`).
///
/// Persisted via `get_llm_models` / `upsert_llm_model`, which only
/// accept output of [`validate::validate`] — unvalidated strings never
/// reach SQLite. No key field exists by design (T-39-01).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmSettings {
    /// Optional row id (set when reading/updating an existing row; `None` for new inserts).
    pub id: Option<i64>,
    /// Provider preset key: `"groq"`, `"openrouter"`, or `"custom"`.
    pub provider: String,
    /// Normalized base URL: trimmed, trailing `/` stripped.
    pub base_url: String,
    /// Model name, trimmed, never empty.
    pub model: String,
    /// Sampling temperature, 0.0–2.0 inclusive.
    pub temperature: f64,
    /// Max output tokens, 1–4096 inclusive.
    pub max_tokens: i64,
    /// Request timeout in seconds, `>= 1`, no upper bound.
    pub timeout_secs: i64,
    /// Prompt template source text (opaque until Plan 39-02 renders it).
    pub prompt_template: String,
}

impl Default for LlmSettings {
    fn default() -> Self {
        LlmSettings {
            id: None,
            provider: "custom".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            prompt_template: DEFAULT_TEMPLATE.to_string(),
        }
    }
}

/// IPC view of the settings row (Plan 39-03, D-03/D-07): all non-secret
/// fields plus a presence-only key flag. The key string never crosses IPC
/// — there is no key field, only `has_api_key: bool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmSettingsView {
    /// Optional row id (used for edit/delete targeting).
    pub id: Option<i64>,
    /// Provider preset key: `"groq"`, `"openrouter"`, or `"custom"`.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: i64,
    pub timeout_secs: i64,
    pub prompt_template: String,
    pub has_api_key: bool,
}

impl LlmSettingsView {
    /// Join a settings row with a keyring presence probe into one view.
    pub fn with_presence(settings: LlmSettings, has_api_key: bool) -> Self {
        LlmSettingsView {
            id: settings.id,
            provider: settings.provider,
            base_url: settings.base_url,
            model: settings.model,
            temperature: settings.temperature,
            max_tokens: settings.max_tokens,
            timeout_secs: settings.timeout_secs,
            prompt_template: settings.prompt_template,
            has_api_key,
        }
    }
}

/// IPC input for saving settings (Plan 39-03, A6 resolved): the non-secret
/// fields plus an optional key. `api_key` semantics: `None` leaves the
/// stored key unchanged, `Some` non-empty overwrites it, `Some`
/// empty/whitespace-only is rejected — removal only via the dedicated
/// clear command, never via save.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmSettingsInput {
    /// Optional row id: `Some(id)` updates that row; `None` inserts new.
    pub id: Option<i64>,
    /// Provider preset key: `"groq"`, `"openrouter"`, or `"custom"`.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: i64,
    pub timeout_secs: i64,
    pub prompt_template: String,
    pub api_key: Option<String>,
}

// Manual `Debug`: the derived form would print key material into logs on
// any `{:?}` of the input. Presence is shown, the value never is (T-39-01).
impl std::fmt::Debug for LlmSettingsInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmSettingsInput")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("timeout_secs", &self.timeout_secs)
            .field("prompt_template", &self.prompt_template)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl From<&LlmSettingsInput> for LlmSettings {
    /// Project the non-secret fields out of save input (the key never
    /// enters [`LlmSettings`] — it travels a separate keyring path).
    fn from(input: &LlmSettingsInput) -> Self {
        LlmSettings {
            id: input.id,
            provider: input.provider.clone(),
            base_url: input.base_url.clone(),
            model: input.model.clone(),
            temperature: input.temperature,
            max_tokens: input.max_tokens,
            timeout_secs: input.timeout_secs,
            prompt_template: input.prompt_template.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::queries::{get_llm_models, get_llm_settings_legacy, upsert_llm_model};
    use crate::store::Store;

    /// Edited (non-default) settings used by the round-trip tests.
    fn edited() -> LlmSettings {
        LlmSettings {
            id: None,
            provider: "groq".to_string(),
            base_url: "http://localhost:11434".to_string(),
            model: "llama3".to_string(),
            temperature: 0.1,
            max_tokens: 100,
            timeout_secs: 30,
            prompt_template: "S: {sender} Sub: {subject} B: {body}".to_string(),
        }
    }

    #[test]
    fn seed_defaults_equal_consts() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let got = get_llm_settings_legacy(store.conn())
            .expect("read should succeed")
            .expect("M23 must seed row id 1");
        assert_eq!(got.base_url, DEFAULT_BASE_URL);
        assert_eq!(got.base_url, "https://api.openai.com/v1");
        assert_eq!(got.model, DEFAULT_MODEL);
        assert_eq!(got.temperature, DEFAULT_TEMPERATURE);
        assert_eq!(got.temperature, 0.7);
        assert_eq!(got.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(got.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(got.prompt_template, DEFAULT_TEMPLATE);
    }

    #[test]
    fn save_then_read_round_trips_edited_values() {
        // Multi-model lane (M25): the save path writes `llm_models`, so the
        // round-trip reads it back through `get_llm_models` (the legacy
        // single-row `llm_settings` table now only carries the M23 seed).
        let store = Store::open_in_memory().expect("migration should succeed");
        let validated = validate::validate(&edited()).expect("edited values are valid");
        let id = upsert_llm_model(store.conn(), &validated).expect("upsert should succeed");
        let rows = get_llm_models(store.conn()).expect("read should succeed");
        assert_eq!(rows.len(), 1, "one configured model after one insert");
        let got = &rows[0];
        assert_eq!(got.id, Some(id), "the insert reports the stored row id");
        assert_eq!(
            *got,
            LlmSettings {
                id: Some(id),
                ..validated.clone()
            },
            "every validated field round-trips unchanged"
        );
        assert_eq!(got.base_url, "http://localhost:11434");
        assert_eq!(got.model, "llama3");
    }

    #[test]
    fn remote_http_and_empty_model_rejected() {
        let mut bad_url = LlmSettings::default();
        bad_url.base_url = "http://example.com".to_string();
        assert!(
            matches!(
                validate::validate(&bad_url),
                Err(validate::LlmSettingsError::InvalidBaseUrl)
            ),
            "remote http must be rejected"
        );

        let mut bad_model = LlmSettings::default();
        bad_model.model = String::new();
        assert!(
            matches!(
                validate::validate(&bad_model),
                Err(validate::LlmSettingsError::EmptyModel)
            ),
            "empty model must be rejected, not stored"
        );
        let mut blank_model = LlmSettings::default();
        blank_model.model = "   ".to_string();
        assert!(
            matches!(
                validate::validate(&blank_model),
                Err(validate::LlmSettingsError::EmptyModel)
            ),
            "whitespace-only model must be rejected, not stored"
        );
    }

    #[test]
    fn double_save_leaves_one_identical_row() {
        // `upsert_llm_model` inserts when `id` is `None` and updates in place
        // when it is `Some` — so "double save" is save-then-update-its-id, and
        // the invariant to keep is one row carrying the last values.
        let store = Store::open_in_memory().expect("migration should succeed");
        let validated = validate::validate(&edited()).expect("edited values are valid");
        let id = upsert_llm_model(store.conn(), &validated).expect("first save");
        let with_id = LlmSettings {
            id: Some(id),
            ..validated.clone()
        };
        upsert_llm_model(store.conn(), &with_id).expect("second save");
        let count: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM llm_models", [], |r| r.get(0))
            .expect("count should succeed");
        assert_eq!(count, 1, "double-save must stay a single row");
        let rows = get_llm_models(store.conn()).expect("read should succeed");
        assert_eq!(rows, vec![with_id], "double-save must keep identical values");
    }

    #[test]
    fn sqlite_dump_contains_no_key_material() {
        // T-39-01: the key never enters this store. Save through the full
        // validated path, then scan every text cell of the settings row —
        // no secret-looking material may appear.
        const FAKE_KEY: &str = "sk-test-fake-key-material-never-stored";
        let store = Store::open_in_memory().expect("migration should succeed");
        let validated = validate::validate(&edited()).expect("edited values are valid");
        let saved = upsert_llm_model(store.conn(), &validated).expect("save should succeed");
        assert!(saved > 0, "the save must report the stored row id");
        // Scan every settings text cell in the store — the row this test just
        // wrote in `llm_models` AND the M23 seed row in the legacy
        // `llm_settings` table.
        let dump: String = store
            .conn()
            .query_row(
                "SELECT group_concat(line, '|') FROM (
                     SELECT provider || '|' || base_url || '|' || model || '|' || prompt_template AS line
                       FROM llm_settings
                     UNION ALL
                     SELECT provider || '|' || base_url || '|' || model || '|' || prompt_template
                       FROM llm_models
                 )",
                [],
                |r| r.get(0),
            )
            .expect("dump should succeed");
        let debug = format!("{validated:?}");
        assert!(
            !dump.contains(FAKE_KEY),
            "settings row must not contain key material"
        );
        assert!(
            !dump.contains("sk-test"),
            "settings row must not contain secret-looking text"
        );
        // The struct itself carries no key: its Debug form holds only the
        // six non-secret fields.
        let debug = format!("{validated:?}");
        assert!(!debug.contains(FAKE_KEY));
    }
}
