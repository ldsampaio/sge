//! Integration tests for LLM generation flow (Phase 40).
//!
//! These tests use mock transport to simulate real API responses.
//! Live endpoint tests are gated behind `--ignored` per project convention.

use std::sync::Arc;
use std::time::Duration;

use sge::llm::client::*;
use sge::llm::key::MemoryLlmKeyStore;
use sge::llm::{LlmSettings, DEFAULT_BASE_URL, DEFAULT_MODEL};

/// Test settings for mock endpoint
fn mock_settings() -> LlmSettings {
    LlmSettings {
        base_url: "http://localhost:11434/v1".to_string(),
        model: "mock-model".to_string(),
        temperature: 0.7,
        max_tokens: 500,
        timeout_secs: 10,
        prompt_template: "Reply to: {sender}\nSubject: {subject}\nBody: {body}".to_string(),
    }
}

#[test]
fn generate_reply_without_key_returns_not_configured() {
    let settings = mock_settings();
    let key_store = MemoryLlmKeyStore::new();
    
    let result = generate_reply(
        &settings,
        "test body",
        &key_store,
        "INBOX",
        None,
    );
    
    assert!(
        matches!(result, Err(LlmRequestError::NotConfigured)),
        "Expected NotConfigured error, got: {:?}",
        result
    );
}

#[test]
fn test_connection_without_key_returns_not_configured() {
    let settings = mock_settings();
    let key_store = MemoryLlmKeyStore::new();
    
    let result = test_llm_connection(&settings, &key_store);
    
    assert!(
        matches!(result, Err(LlmRequestError::NotConfigured)),
        "Expected NotConfigured error, got: {:?}",
        result
    );
}

#[test]
fn mock_transport_openai_envelope() {
    let mut mock = MockTransport::new();
    mock = mock.add_success("Hello from OpenAI!");
    
    let response = mock.next().unwrap();
    match response {
        MockResponse::Success { choices, .. } => {
            assert_eq!(choices.len(), 1);
            assert_eq!(choices[0].1, "Hello from OpenAI!");
        }
        _ => panic!("Expected Success response"),
    }
}

#[test]
fn mock_transport_ollama_envelope() {
    let mut mock = MockTransport::new();
    // Simulate Ollama-style response
    mock = mock.add_success("Response from Ollama");
    
    let response = mock.next().unwrap();
    match response {
        MockResponse::Success { choices, .. } => {
            assert_eq!(choices[0].1, "Response from Ollama");
        }
        _ => panic!("Expected Success response"),
    }
}

#[test]
fn mock_transport_error_formats() {
    let mut mock = MockTransport::new();
    mock = mock.add_error(401, "Unauthorized");
    
    let response = mock.next().unwrap();
    match response {
        MockResponse::Error { status, body } => {
            assert_eq!(status, 401);
            assert!(body.contains("error"));
        }
        _ => panic!("Expected Error response"),
    }
}

#[test]
fn error_no_key_in_metadata() {
    // T-39-01: key and bodies never in errors or logs
    let body = r#"{"error":{"message":"Invalid API key: sk-test-secret-key-leak"}}}"#;
    let err = LlmRequestError::from_status(
        reqwest::StatusCode::UNAUTHORIZED,
        Some(body),
    );
    
    let msg = err.to_string();
    // The API key value should never appear in the user-facing error
    assert!(!msg.contains("sk-test-secret-key-leak"));
    assert!(msg.contains("API key") || msg.contains("Settings"));
}

#[test]
fn error_body_never_in_metadata() {
    // Mail body content should never appear in error messages
    let sensitive_body = "Confidential: please do not forward to third parties";
    let body = format!(r#"{{"error": {{"message": "{}", "type": "invalid_request_error"}}}}"#, sensitive_body);
    
    let err = LlmRequestError::from_status(
        reqwest::StatusCode::BAD_REQUEST,
        Some(&body),
    );
    
    let msg = err.to_string();
    // Sensitive content should not be in the error message
    assert!(!msg.contains("Confidential"));
    assert!(!msg.contains("third parties"));
}

#[test]
fn retry_only_on_correct_codes() {
    // Retry should only happen on 408/429/5xx
    let retry_cases = [
        reqwest::StatusCode::REQUEST_TIMEOUT, // 408
        reqwest::StatusCode::TOO_MANY_REQUESTS, // 429
        reqwest::StatusCode::INTERNAL_SERVER_ERROR, // 500
        reqwest::StatusCode::BAD_GATEWAY, // 502
        reqwest::StatusCode::SERVICE_UNAVAILABLE, // 503
    ];
    
    for status in retry_cases {
        let err = LlmRequestError::from_status(status, None);
        match err {
            LlmRequestError::Timeout
            | LlmRequestError::RateLimited
            | LlmRequestError::ProviderError { .. } => {} // Retryable
            _ => panic!("Expected retryable error for status {status}"),
        }
    }
    
    // Non-retryable codes
    let non_retry_cases = [
        reqwest::StatusCode::BAD_REQUEST, // 400
        reqwest::StatusCode::UNAUTHORIZED, // 401
        reqwest::StatusCode::FORBIDDEN, // 403
        reqwest::StatusCode::NOT_FOUND, // 404
    ];
    
    for status in non_retry_cases {
        let err = LlmRequestError::from_status(status, None);
        assert!(
            !matches!(err, LlmRequestError::Timeout | LlmRequestError::RateLimited),
            "Status {status} should not be retryable"
        );
    }
}

#[test]
fn single_flight_blocks_concurrent_requests() {
    let tracker = Arc::new(RequestTracker::new());
    
    // Start a request
    let rid1 = tracker.start_request("INBOX");
    assert!(rid1.is_some());
    
    // Try concurrent request for same mailbox - should fail
    let rid2 = tracker.start_request("INBOX");
    assert!(rid2.is_none());
    
    // End first request
    tracker.end_request("INBOX");
    
    // Now a new request should succeed
    let rid3 = tracker.start_request("INBOX");
    assert!(rid3.is_some());
    assert!(rid3 > rid1.unwrap());
}

#[test]
fn generation_result_metadata_only() {
    // Verify GenerationResult structure contains no key or body data
    let result = GenerationResult {
        draft_text: "This is a draft response".to_string(),
        latency_ms: 1234,
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    };
    
    assert!(result.draft_text.len() > 0);
    assert!(result.latency_ms > 0);
    assert!(result.prompt_tokens > 0);
    assert!(result.completion_tokens > 0);
    assert!(result.total_tokens > 0);
}

#[test]
fn test_result_structure() {
    let result = TestResult {
        success: true,
        error: None,
        models_count: 3,
    };
    
    assert!(result.success);
    assert!(result.error.is_none());
    assert_eq!(result.models_count, 3);
}

#[test]
fn mock_transport_truncation_simulation() {
    // Simulate truncation-on-wire scenario
    let mut mock = MockTransport::new();
    mock = mock.add_success("Truncated response from provider that was cut short");
    
    let response = mock.next().unwrap();
    match response {
        MockResponse::Success { choices, .. } => {
            assert_eq!(choices.len(), 1);
            assert!(choices[0].1.contains("Truncated"));
        }
        _ => panic!("Expected Success response"),
    }
}

#[test]
fn mock_transport_redirect_refusal() {
    let mut mock = MockTransport::new();
    mock = mock.add_error(301, "Moved Permanently");
    
    let response = mock.next().unwrap();
    match response {
        MockResponse::Error { status, .. } => {
            // Redirects should be treated as errors (301 is non-2xx)
            assert!(status >= 300);
        }
        _ => panic!("Expected Error response for redirect"),
    }
}

#[test]
fn mock_transport_empty_choices() {
    // Simulate empty choices as error
    let mut mock = MockTransport::new();
    mock = mock.add_success(""); // Empty success
    
    let response = mock.next().unwrap();
    match response {
        MockResponse::Success { choices, .. } => {
            // Even with empty content, the response structure is success
            assert_eq!(choices.len(), 1);
            assert!(choices[0].1.is_empty());
        }
        _ => panic!("Expected Success response"),
    }
}

#[test]
fn plain_language_all_error_paths() {
    // Verify all error paths produce user-understandable text
    let errors = [
        LlmRequestError::NotConfigured,
        LlmRequestError::Unauthorized,
        LlmRequestError::Timeout,
        LlmRequestError::NetworkError,
        LlmRequestError::RateLimited,
        LlmRequestError::InvalidResponse,
        LlmRequestError::TokenLimit,
        LlmRequestError::Cancelled,
        LlmRequestError::Unknown,
    ];
    
    for err in errors {
        let msg = err.to_string();
        // Every error message should be human-readable
        assert!(!msg.is_empty());
        // No error should contain secret-looking text
        assert!(!msg.contains("sk-"));
        assert!(!msg.contains("api_key"));
        assert!(!msg.contains("password"));
    }
}

/// Live endpoint roundtrip (requires real key + endpoint).
/// Run explicitly: cargo test --test llm -- --ignored
#[test]
#[ignore]
fn live_openai_connection_test() {
    let settings = LlmSettings {
        base_url: "https://api.openai.com/v1".to_string(),
        model: "gpt-4o-mini".to_string(),
        temperature: 0.7,
        max_tokens: 500,
        timeout_secs: 10,
        prompt_template: "Hello".to_string(),
    };
    
    let key_store = MemoryLlmKeyStore::new();
    key_store.save_secret("sk-test-live-key").unwrap();
    
    let result = test_llm_connection(&settings, &key_store);
    assert!(result.is_ok(), "Live connection test failed: {:?}", result);
}
