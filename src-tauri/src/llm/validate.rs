//! Save-time validation for LLM settings (Phase 39, Plan 39-01 — D-05/D-06).
//!
//! Every rejection names the field and the allowed range in plain language.
//! Error values never interpolate secrets: the only interpolated data would
//! be non-secret settings, and the variants below carry no payload at all —
//! key and mail bodies cannot appear in errors by construction (T-39-01).
//! No network I/O: parsing uses `reqwest::Url` (unconditional re-export,
//! zero new dependencies), never a `Client` or `.send()`.

use thiserror::Error;

use super::LlmSettings;

/// Field-named, plain-language save-time rejections. Unit variants only —
/// no payload means no secret can ever be formatted into an error string.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LlmSettingsError {
    #[error(
        "invalid base URL: must use https (for example https://api.openai.com/v1); http is allowed only for local servers such as http://localhost"
    )]
    InvalidBaseUrl,
    #[error("invalid model: model name must not be empty")]
    EmptyModel,
    #[error("invalid temperature: must be between 0.0 and 2.0")]
    TemperatureOutOfRange,
    #[error("invalid max tokens: must be between 1 and 4096")]
    MaxTokensOutOfRange,
    #[error("invalid timeout: must be at least 1 second (no upper limit)")]
    InvalidTimeout,
    #[error("invalid prompt template: must not be empty")]
    EmptyTemplate,
}

/// Validate raw settings into the normalized form stored by SQLite.
///
/// Normalization: `base_url` is trimmed and stripped of trailing `/` (so
/// Phase 40 can safely append `/chat/completions`); `model` is trimmed.
/// Nothing is ever coerced to a default — out-of-range values error.
pub fn validate(raw: &LlmSettings) -> Result<LlmSettings, LlmSettingsError> {
    let base_url = normalize_base_url(&raw.base_url)?;

    let model = raw.model.trim();
    if model.is_empty() {
        return Err(LlmSettingsError::EmptyModel);
    }

    // Direct range comparison on the literal — no rounding, no epsilon
    // merge — so adjacency values (e.g. 2.0000001) never merge into the
    // boundary. NaN/Infinity fail `contains` and are rejected.
    if !(0.0..=2.0).contains(&raw.temperature) {
        return Err(LlmSettingsError::TemperatureOutOfRange);
    }
    if !(1..=4096).contains(&raw.max_tokens) {
        return Err(LlmSettingsError::MaxTokensOutOfRange);
    }
    // Timeout is validate-only per D-06 (exempt from clamping): any
    // positive value is accepted unclamped; 0/negatives are meaningless
    // as a `Duration` and rejected.
    if raw.timeout_secs < 1 {
        return Err(LlmSettingsError::InvalidTimeout);
    }

    if raw.prompt_template.trim().is_empty() {
        return Err(LlmSettingsError::EmptyTemplate);
    }

    Ok(LlmSettings {
        base_url,
        model: model.to_string(),
        temperature: raw.temperature,
        max_tokens: raw.max_tokens,
        timeout_secs: raw.timeout_secs,
        prompt_template: raw.prompt_template.clone(),
    })
}

/// Trim + parse + allowlist-check a base URL, returning the stored form.
///
/// Policy (D-05): `https` on any host, or `http` only when the host matches
/// `imap::is_loopback` semantics (`localhost` / `127.0.0.1` / `::1`,
/// bracket-tolerant) for Ollama-class local servers. Everything else —
/// wrong scheme, bare hostname, remote `http`, unparsable input — is
/// rejected with [`LlmSettingsError::InvalidBaseUrl`].
fn normalize_base_url(raw: &str) -> Result<String, LlmSettingsError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(LlmSettingsError::InvalidBaseUrl);
    }
    let url = reqwest::Url::parse(trimmed).map_err(|_| LlmSettingsError::InvalidBaseUrl)?;
    match url.scheme() {
        "https" => {}
        "http" if crate::imap::is_loopback(url.host_str().unwrap_or("")) => {}
        _ => return Err(LlmSettingsError::InvalidBaseUrl),
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> LlmSettings {
        LlmSettings::default()
    }

    #[test]
    fn accepts_default_settings_unchanged() {
        let got = validate(&valid()).expect("defaults are valid");
        assert_eq!(got, valid());
    }

    #[test]
    fn accepts_https_with_port_and_path() {
        let mut s = valid();
        s.base_url = "https://example.com:8443/api/v1".to_string();
        let got = validate(&s).expect("https with port/path must pass");
        assert_eq!(got.base_url, "https://example.com:8443/api/v1");
    }

    #[test]
    fn accepts_http_loopback_trio_with_ports() {
        for url in [
            "http://localhost:11434",
            "http://127.0.0.1:11434",
            "http://[::1]:11434",
            "http://localhost",
        ] {
            let mut s = valid();
            s.base_url = url.to_string();
            let got = validate(&s).expect("loopback http must pass");
            assert_eq!(got.base_url, url);
        }
    }

    #[test]
    fn rejects_remote_http() {
        for url in ["http://example.com", "http://192.168.1.1"] {
            let mut s = valid();
            s.base_url = url.to_string();
            assert!(
                matches!(validate(&s), Err(LlmSettingsError::InvalidBaseUrl)),
                "{url} must be rejected"
            );
        }
    }

    #[test]
    fn error_text_names_field_and_https() {
        let mut s = valid();
        s.base_url = "http://example.com".to_string();
        let err = validate(&s).unwrap_err().to_string();
        assert!(err.contains("base URL"), "error must name the field");
        assert!(err.contains("https"), "error must state the https policy");
    }
}
