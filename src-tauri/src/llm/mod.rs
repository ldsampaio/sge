//! LLM settings lane (Phase 39, Plan 39-01 — LLM-01/LLM-04 storage + validation).
//!
//! Non-secret generation settings persist in the M23 single-row
//! `llm_settings` table via [`crate::store::queries`]. The API key lives
//! only in the OS keyring (Plan 39-02) and never enters [`LlmSettings`] —
//! there is no key field, no key column, and no `Debug` output holding a
//! key (T-39-01). This lane performs zero network I/O: no `reqwest::Client`
//! construction, no `.send()` — generation egress lands in Phase 40.

pub mod key;
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

/// Validated, non-secret LLM generation settings (single M23 row, `id = 1`).
///
/// Persisted via `get_llm_settings` / `upsert_llm_settings`, which only
/// accept output of [`validate::validate`] — unvalidated strings never
/// reach SQLite. No key field exists by design (T-39-01).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmSettings {
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
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            prompt_template: DEFAULT_TEMPLATE.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::queries::{get_llm_settings, upsert_llm_settings};
    use crate::store::Store;

    /// Edited (non-default) settings used by the round-trip tests.
    fn edited() -> LlmSettings {
        LlmSettings {
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
        let got = get_llm_settings(store.conn())
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
        let store = Store::open_in_memory().expect("migration should succeed");
        let validated = validate::validate(&edited()).expect("edited values are valid");
        upsert_llm_settings(store.conn(), &validated).expect("upsert should succeed");
        let got = get_llm_settings(store.conn())
            .expect("read should succeed")
            .expect("row must exist after upsert");
        assert_eq!(got, validated);
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
        let store = Store::open_in_memory().expect("migration should succeed");
        let validated = validate::validate(&edited()).expect("edited values are valid");
        upsert_llm_settings(store.conn(), &validated).expect("first upsert");
        upsert_llm_settings(store.conn(), &validated).expect("second upsert");
        let count: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM llm_settings", [], |r| r.get(0))
            .expect("count should succeed");
        assert_eq!(count, 1, "double-save must stay a single row");
        let got = get_llm_settings(store.conn())
            .expect("read should succeed")
            .expect("row must exist");
        assert_eq!(got, validated, "double-save must keep identical values");
    }

    #[test]
    fn sqlite_dump_contains_no_key_material() {
        // T-39-01: the key never enters this store. Save through the full
        // validated path, then scan every text cell of the settings row —
        // no secret-looking material may appear.
        const FAKE_KEY: &str = "sk-test-fake-key-material-never-stored";
        let store = Store::open_in_memory().expect("migration should succeed");
        let validated = validate::validate(&edited()).expect("edited values are valid");
        upsert_llm_settings(store.conn(), &validated).expect("upsert should succeed");
        let dump: String = store
            .conn()
            .query_row(
                "SELECT base_url || '|' || model || '|' || prompt_template FROM llm_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .expect("dump should succeed");
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
