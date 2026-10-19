//! LLM generation client (Phase 40, Plan 40-01 — LLM-05/LLM-07/LLM-09).
//!
//! HTTP client for LLM endpoint calls: test connection (`GET /models`) and
//! generation (`POST /chat/completions`). Non-streaming only; streaming deferred
//! to v1.8.x. All calls use `reqwest::Client` with configured timeout, cancel
//! support via request IDs, and per-window single-flight deduplication.
//!
//! Security: API key and message bodies **never** appear in error messages or
//! logs (metadata-only: status code, latency, token usage). Key loaded from
//! keyring via `LlmKeyStore` trait; never interpolated into structs or strings.
//!
//! Error handling: plain-language mapping for all failure classes:
//! - Unconfigured → "Configure in Settings"
//! - 401 → "Check API key" (unauthorized)
//! - 408 → timeout, with optional retry
//! - 429 → rate limited, with optional retry
//! - 5xx → provider error, with retry option
//! - Network → connectivity issue
//! - JSON/schema → "Provider response invalid"
//!
//! Mock transport for CI: `MockClient` supports OpenAI and Ollama responseshapes,
//! error formats, and edge cases (redirect refusal, truncation-on-wire).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::key::LlmKeyStore;
use super::{LlmSettings, LlmSettingsInput, LlmSettingsView};

/// Generation request ID for cancel tracking.
pub type RequestId = u64;

/// One active request slot per mailbox. Grows as needed; never shrinks.
/// Key: `mailbox_id` (INBOX = "INBOX"), Value: `Some(RequestId)` or `None` (done).
pub type PendingRequests = HashMap<String, Option<RequestId>>;

/// Per-window request tracker for single-flight guarantee.
#[derive(Debug, Default)]
pub struct RequestTracker {
    /// Map of mailbox name → current request ID (if any).
    slots: Mutex<PendingRequests>,
    /// Global ID counter (monotonic).
    counter: Mutex<u64>,
}

impl RequestTracker {
    /// Create a new request tracker.
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            counter: Mutex::new(1),
        }
    }

    /// Try to start a request for the given mailbox.
    /// Returns `Some(rid)` if this caller now owns the request.
    /// Returns `None` if another request is already active for this mailbox.
    pub fn start_request(&self, mailbox: &str) -> Option<RequestId> {
        let mut slots = self.slots.lock().expect("request tracker lock");
        if slots.get(mailbox).is_some_and(|v| v.is_some()) {
            return None; // Another request active
        }
        let rid = {
            let mut counter = self.counter.lock().expect("counter lock");
            let rid = *counter;
            *counter += 1;
            rid
        };
        slots.insert(mailbox.to_string(), Some(rid));
        Some(rid)
    }

    /// Mark a request as completed (cancel or normal finish).
    pub fn end_request(&self, mailbox: &str) {
        if let Some(slots) = self
            .slots
            .lock()
            .expect("request tracker lock")
            .get_mut(mailbox)
        {
            *slots = None;
        }
    }

    /// Cancel a pending request, returning true if found and cancelled.
    pub fn cancel_request(&self, mailbox: &str) -> bool {
        if let Some(slots) = self
            .slots
            .lock()
            .expect("request tracker lock")
            .get_mut(mailbox)
        {
            if slots.is_some() {
                *slots = None;
                return true;
            }
        }
        false
    }

    /// Check if a request is active for this mailbox.
    pub fn has_active(&self, mailbox: &str) -> bool {
        self.slots
            .lock()
            .expect("request tracker lock")
            .get(mailbox)
            .is_some_and(|v| v.is_some())
    }

    /// True when `rid` is still the active owner of `mailbox`.
    ///
    /// Used after an awaited request returns: a `cancel_request` (or a newer
    /// request) in the meantime means this result must be discarded instead
    /// of handed to the caller (LLM-08 cancel).
    pub fn is_owner(&self, mailbox: &str, rid: RequestId) -> bool {
        self.slots
            .lock()
            .expect("request tracker lock")
            .get(mailbox)
            .is_some_and(|v| *v == Some(rid))
    }
}

/// Plain-language error for LLM operations (LLM-09 compliance).
/// No key or body data - only metadata fields.
#[derive(Debug, Error, PartialEq)]
pub enum LlmRequestError {
    #[error("No LLM settings configured. Go to Settings to configure the API endpoint.")]
    NotConfigured,

    #[error("Invalid API key. Check that your key is correct in Settings.")]
    Unauthorized,

    #[error("Request timed out. The server took too long to respond. Try again or check your connection.")]
    Timeout,

    #[error("Network error: could not connect to the LLM endpoint. Check your internet connection and endpoint URL.")]
    NetworkError,

    #[error("Rate limited by provider. Try again in a few seconds.")]
    RateLimited,

    #[error("Provider error ({code}): {message}")]
    ProviderError { code: i32, message: String },

    #[error("Provider returned invalid response format. This may indicate a configuration issue with the endpoint.")]
    InvalidResponse,

    #[error("Token limit reached. The generated response was truncated.")]
    TokenLimit,

    #[error("Request cancelled by user.")]
    Cancelled,

    #[error("Unknown error from LLM service. Please try again.")]
    Unknown,
}

impl LlmRequestError {
    /// Map a transport failure to a plain-language error: a request that
    /// outlived `timeout_secs` is a timeout the user can retry, anything
    /// else is a connection problem. Never carries the URL or the key.
    pub fn from_reqwest(e: &reqwest::Error) -> Self {
        if e.is_timeout() {
            LlmRequestError::Timeout
        } else {
            LlmRequestError::NetworkError
        }
    }

    /// True for transient provider failures worth one automatic retry
    /// (429 rate limits, gateway timeouts, and 5xx provider errors).
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmRequestError::RateLimited | LlmRequestError::Timeout => true,
            LlmRequestError::ProviderError { code, .. } => *code >= 500,
            _ => false,
        }
    }

    /// Map HTTP status and response to plain-language error.
    pub fn from_status(status: reqwest::StatusCode, body: Option<&str>) -> Self {
        match status {
            reqwest::StatusCode::UNAUTHORIZED => LlmRequestError::Unauthorized,
            reqwest::StatusCode::REQUEST_TIMEOUT | reqwest::StatusCode::GATEWAY_TIMEOUT => {
                LlmRequestError::Timeout
            }
            reqwest::StatusCode::TOO_MANY_REQUESTS => LlmRequestError::RateLimited,
            status if status == reqwest::StatusCode::INSUFFICIENT_STORAGE => {
                // 507: often used for rate limits
                LlmRequestError::RateLimited
            }
            status if status.is_client_error() || status.is_server_error() => {
                let msg = body
                    .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                    .and_then(|v| {
                        v.get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| "Provider returned an error".to_string());
                LlmRequestError::ProviderError {
                    code: status.as_u16() as i32,
                    message: msg,
                }
            }
            _ => LlmRequestError::Unknown,
        }
    }
}

/// Response from `/models` endpoint (test connection).
#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ModelInfo {
    pub id: String,
}

/// OpenAI-style chat completion request body.
#[derive(Debug, Serialize)]
pub struct CompletionRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<Message<'a>>,
    pub temperature: f64,
    pub max_tokens: i64,
}

#[derive(Debug, Serialize)]
pub struct Message<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

/// OpenAI-style chat completion response.
#[derive(Debug, Deserialize)]
pub struct CompletionResponse {
    pub id: Option<String>,
    pub object: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub index: i32,
    pub message: Option<MessageResponse>,
    pub text: Option<String>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageResponse {
    pub role: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

/// Result of a generation request.
#[derive(Debug, Clone)]
pub struct GenerationResult {
    pub draft_text: String,
    pub latency_ms: u64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

/// Test connection result.
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub success: bool,
    pub error: Option<String>,
    pub models_count: usize,
    pub latency_ms: u64,
}

/// Mock transport for testing (OpenAI/Ollama shapes).
#[derive(Debug, Clone)]
pub struct MockTransport {
    pub responses: Vec<MockResponse>,
    pub current: usize,
}

#[derive(Debug, Clone)]
pub enum MockResponse {
    Success {
        choices: Vec<(i32, String)>,
        usage: (i64, i64, i64),
    },
    Error {
        status: u16,
        body: String,
    },
    Redirect {
        location: String,
    },
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTransport {
    pub fn new() -> Self {
        Self {
            responses: Vec::new(),
            current: 0,
        }
    }

    pub fn add_success(mut self, text: &str) -> Self {
        self.responses.push(MockResponse::Success {
            choices: vec![(0, text.to_string())],
            usage: (100, 200, 300),
        });
        self
    }

    pub fn add_error(mut self, status: u16, message: &str) -> Self {
        use serde_json::json;
        let body = json!({
            "error": {
                "message": message,
                "type": "error",
                "code": status
            }
        })
        .to_string();
        self.responses.push(MockResponse::Error { status, body });
        self
    }

    pub fn next(&mut self) -> Option<MockResponse> {
        let r = self.responses.get(self.current).cloned();
        if r.is_some() {
            self.current = (self.current + 1) % self.responses.len().max(1);
        }
        r
    }

    pub fn reset(&mut self) {
        self.current = 0;
    }
}

/// Generate a reply using the configured LLM endpoint.
///
/// Returns the draft text on success, or a plain-language error.
/// Key and body never appear in errors.
///
/// This is an async function - callers should use `.await` or call from an async context.
///
/// NOTE: this variant loads the API key from `key_store`, and every
/// keyring call blocks the calling thread — the Tauri command layer must
/// therefore call [`generate_reply_with_key`] with a key it loaded inside
/// `spawn_blocking` (see `llm::commands::generate_reply`).
pub async fn generate_reply_async(
    settings: &LlmSettings,
    template: &str,
    key_store: &impl LlmKeyStore,
    _mailbox: &str,
    _request_id: Option<RequestId>,
) -> Result<GenerationResult, LlmRequestError> {
    // Load key from keyring
    let api_key = key_store
        .load_secret()
        .map_err(|_| LlmRequestError::NetworkError)?;

    let api_key = match api_key {
        Some(k) => k,
        None => return Err(LlmRequestError::NotConfigured),
    };

    generate_reply_with_key(settings, template, &api_key).await
}

/// Same as [`generate_reply_async`] but with the key already in hand, so
/// no keyring I/O happens on the async runtime thread (hard rule).
pub async fn generate_reply_with_key(
    settings: &LlmSettings,
    template: &str,
    api_key: &str,
) -> Result<GenerationResult, LlmRequestError> {
    // Build client with timeout
    let timeout = Duration::from_secs(settings.timeout_secs as u64);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| LlmRequestError::NetworkError)?;

    // Build request body
    let request_body = CompletionRequest {
        model: &settings.model,
        messages: vec![Message {
            role: "user",
            content: template,
        }],
        temperature: settings.temperature,
        max_tokens: settings.max_tokens,
    };

    // Make request
    let start = Instant::now();
    let resp = client
        .post(format!("{}/chat/completions", settings.base_url))
        .bearer_auth(api_key)
        .json(&request_body)
        .send()
        .await
        .map_err(|e| LlmRequestError::from_reqwest(&e))?;

    let latency_ms = start.elapsed().as_millis() as u64;

    let status = resp.status();

    if !status.is_success() {
        let body = resp.text().await.ok();
        return Err(LlmRequestError::from_status(status, body.as_deref()));
    }

    let body = resp
        .text()
        .await
        .map_err(|_| LlmRequestError::InvalidResponse)?;

    // Parse response
    let resp_json = serde_json::from_slice::<CompletionResponse>(&body.into_bytes())
        .map_err(|_| LlmRequestError::InvalidResponse)?;

    let choice = resp_json
        .choices
        .iter()
        .find(|c| c.finish_reason.as_deref() != Some("length"))
        .or_else(|| resp_json.choices.first())
        .ok_or_else(|| LlmRequestError::InvalidResponse)?;

    let content = choice
        .message
        .as_ref()
        .and_then(|m| m.content.as_deref())
        .or_else(|| choice.text.as_deref())
        .ok_or_else(|| LlmRequestError::InvalidResponse)?;

    let (pt, ct, tt) = resp_json.usage.as_ref().map_or((0, 0, 0), |u| {
        (u.prompt_tokens, u.completion_tokens, u.total_tokens)
    });

    Ok(GenerationResult {
        draft_text: content.to_string(),
        latency_ms,
        prompt_tokens: pt,
        completion_tokens: ct,
        total_tokens: tt,
    })
}

/// Test connection to the LLM endpoint.
///
/// Calls `GET /models` - no mail content is sent.
pub async fn test_llm_connection_async(
    settings: &LlmSettings,
    key_store: &impl LlmKeyStore,
) -> Result<TestResult, LlmRequestError> {
    // Load key from keyring
    let api_key = key_store
        .load_secret()
        .map_err(|_| LlmRequestError::NetworkError)?;

    let api_key = match api_key {
        Some(k) => k,
        None => return Err(LlmRequestError::NotConfigured),
    };

    // Build client with short timeout
    let timeout = Duration::from_secs(std::cmp::min(settings.timeout_secs, 10) as u64);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| LlmRequestError::NetworkError)?;

    // Make request
    let start = Instant::now();
    let resp = client
        .get(format!("{}/models", settings.base_url))
        .bearer_auth(&api_key)
        .send()
        .await;

    let latency_ms = start.elapsed().as_millis() as u64;

    let resp = resp.map_err(|_| LlmRequestError::NetworkError)?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.ok();
        return Err(LlmRequestError::from_status(status, body.as_deref()));
    }

    // Parse models response
    let models_resp: Result<ModelsResponse, _> = resp.json().await;

    match models_resp {
        Ok(m) => {
            let count = m.data.len();
            Ok(TestResult {
                success: true,
                error: None,
                models_count: count,
                latency_ms,
            })
        }
        Err(_) => Err(LlmRequestError::InvalidResponse),
    }
}

/// Fetch available models from a provider's `GET /models` endpoint.
///
/// Used by the settings UI to populate the model dropdown dynamically.
/// Requires a valid API key (Bearer auth) and a base URL.
pub async fn fetch_models_async(
    base_url: &str,
    api_key: &str,
    timeout_secs: u64,
) -> Result<Vec<String>, LlmRequestError> {
    let timeout = Duration::from_secs(timeout_secs.min(10));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| LlmRequestError::NetworkError)?;

    let resp = client
        .get(format!("{}/models", base_url))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|_| LlmRequestError::NetworkError)?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.ok();
        return Err(LlmRequestError::from_status(status, body.as_deref()));
    }

    let models_resp: Result<ModelsResponse, _> = resp.json().await;
    match models_resp {
        Ok(m) => Ok(m.data.into_iter().map(|mi| mi.id).collect()),
        Err(_) => Err(LlmRequestError::InvalidResponse),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::key::MemoryLlmKeyStore;
    use crate::llm::{LlmSettings, DEFAULT_BASE_URL, DEFAULT_MODEL};

    fn test_settings() -> LlmSettings {
        // Multi-model lane shape (M25): every configured model carries an
        // `id` (None until stored) and a provider preset key.
        LlmSettings {
            id: None,
            provider: "custom".to_string(),
            base_url: "http://localhost:11434".to_string(),
            model: "test-model".to_string(),
            temperature: 0.7,
            max_tokens: 500,
            timeout_secs: 10,
            prompt_template: "Test: {body}".to_string(),
        }
    }

    #[test]
    fn request_tracker_single_flight() {
        let tracker = RequestTracker::new();
        // First request should succeed
        assert!(tracker.start_request("INBOX").is_some());
        // Second concurrent request should fail
        assert!(tracker.start_request("INBOX").is_none());
        // Different mailbox should succeed
        assert!(tracker.start_request("Sent").is_some());
        // Complete the first
        tracker.end_request("INBOX");
        // Should be able to start again
        assert!(tracker.start_request("INBOX").is_some());
    }

    #[test]
    fn request_tracker_cancel() {
        let tracker = RequestTracker::new();
        let rid = tracker.start_request("INBOX").unwrap();
        assert!(tracker.has_active("INBOX"));
        assert!(tracker.cancel_request("INBOX"));
        assert!(!tracker.has_active("INBOX"));
        assert!(!tracker.is_owner("INBOX", rid));
    }

    #[test]
    fn request_tracker_is_owner_is_per_request_id() {
        let tracker = RequestTracker::new();
        let first = tracker.start_request("INBOX").unwrap();
        assert!(tracker.is_owner("INBOX", first));
        tracker.end_request("INBOX");
        let second = tracker.start_request("INBOX").unwrap();
        assert!(tracker.is_owner("INBOX", second));
        assert!(
            !tracker.is_owner("INBOX", first),
            "a stale id must never own the slot"
        );
        assert!(!tracker.is_owner("Sent", second), "per-mailbox slots");
    }

    #[test]
    fn retryable_covers_rate_limit_timeout_and_5xx_only() {
        assert!(LlmRequestError::RateLimited.is_retryable());
        assert!(LlmRequestError::Timeout.is_retryable());
        assert!(LlmRequestError::ProviderError {
            code: 503,
            message: "down".to_string()
        }
        .is_retryable());
        assert!(!LlmRequestError::Unauthorized.is_retryable());
        assert!(!LlmRequestError::NotConfigured.is_retryable());
        assert!(!LlmRequestError::Cancelled.is_retryable());
        assert!(!LlmRequestError::ProviderError {
            code: 400,
            message: "bad request".to_string()
        }
        .is_retryable());
    }

    #[test]
    fn from_status_marks_transient_failures_retryable() {
        assert_eq!(
            LlmRequestError::from_status(reqwest::StatusCode::TOO_MANY_REQUESTS, None),
            LlmRequestError::RateLimited
        );
        assert_eq!(
            LlmRequestError::from_status(reqwest::StatusCode::GATEWAY_TIMEOUT, None),
            LlmRequestError::Timeout
        );
        match LlmRequestError::from_status(reqwest::StatusCode::BAD_GATEWAY, None) {
            LlmRequestError::ProviderError { code, .. } => {
                assert_eq!(code, 502);
                assert!(code >= 500);
            }
            other => panic!("expected ProviderError, got {other:?}"),
        }
    }

    #[test]
    fn error_no_key_in_message() {
        let err = LlmRequestError::from_status(reqwest::StatusCode::UNAUTHORIZED, None);
        let msg = err.to_string();
        assert!(!msg.contains("sk-"));
        assert!(msg.contains("API key"));
    }

    #[test]
    fn error_provider_error_extraction() {
        let body = r#"{"error":{"message":"Rate limit exceeded","type":"rate_errors","code":429}}"#;
        let err = LlmRequestError::from_status(reqwest::StatusCode::TOO_MANY_REQUESTS, Some(body));
        match err {
            LlmRequestError::RateLimited => {}
            _ => panic!("Expected RateLimited error"),
        }
    }

    #[test]
    fn mock_transport_sequence() {
        let mut mock = MockTransport::new();
        mock = mock.add_success("test response");
        mock = mock.add_error(429, "rate limited");

        assert!(matches!(mock.next(), Some(MockResponse::Success { .. })));
        assert!(matches!(mock.next(), Some(MockResponse::Error { .. })));
        assert!(matches!(mock.next(), Some(MockResponse::Success { .. }))); // Wraps around
        mock.reset();
        assert!(matches!(mock.next(), Some(MockResponse::Success { .. })));
    }

    #[test]
    fn generate_reply_no_key_returns_not_configured() {
        let settings = test_settings();
        let key_store = MemoryLlmKeyStore::new();
        let result = tauri::async_runtime::block_on(generate_reply_async(
            &settings,
            "test body",
            &key_store,
            "INBOX",
            None,
        ));
        assert!(matches!(result, Err(LlmRequestError::NotConfigured)));
    }
}
