//! Unit tests for LLM client (Phase 40 - LLM-05, LLM-07, LLM-09).

#![allow(unused_imports)]

use crate::llm::client::*;
use crate::llm::key::MemoryLlmKeyStore;
use crate::llm::{LlmSettings, DEFAULT_BASE_URL, DEFAULT_MODEL};

fn test_settings() -> LlmSettings {
    LlmSettings {
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
    // Wraps around to first
    assert!(matches!(mock.next(), Some(MockResponse::Success { .. })));
    mock.reset();
    assert!(matches!(mock.next(), Some(MockResponse::Success { .. })));
}

#[test]
fn error_mapping_all_codes() {
    // Not configured (401 with no key context handled at higher level)
    // Unauthorized
    let err = LlmRequestError::from_status(reqwest::StatusCode::UNAUTHORIZED, Some(r#"{"error":{"message":"invalid key"}}"#));
    assert!(matches!(err, LlmRequestError::Unauthorized));

    // Timeout
    let err = LlmRequestError::from_status(reqwest::StatusCode::REQUEST_TIMEOUT, None);
    assert!(matches!(err, LlmRequestError::Timeout));

    // Rate limited
    let err = LlmRequestError::from_status(reqwest::StatusCode::TOO_MANY_REQUESTS, None);
    assert!(matches!(err, LlmRequestError::RateLimited));

    // 500 provider error
    let err = LlmRequestError::from_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, Some(r#"{"error":{"message":"server down"}}"#));
    match err {
        LlmRequestError::ProviderError { code, message } => {
            assert_eq!(code, 500);
            assert!(message.contains("server down"));
        }
        _ => panic!("Expected ProviderError"),
    }

    // 400 client error
    let err = LlmRequestError::from_status(reqwest::StatusCode::BAD_REQUEST, Some(r#"{"error":{"message":"bad model"}}"#));
    match err {
        LlmRequestError::ProviderError { code, .. } => {
            assert_eq!(code, 400);
        }
        _ => panic!("Expected ProviderError"),
    }

    // Network error (not a status code - handled in caller)
    assert_eq!(LlmRequestError::NetworkError.to_string(), "Network error: could not connect to the LLM endpoint. Check your internet connection and endpoint URL.");
}

#[test]
fn error_plain_language() {
    let err = LlmRequestError::NotConfigured;
    let msg = err.to_string();
    assert!(msg.contains("Settings"));
    assert!(!msg.contains("key"));

    let err = LlmRequestError::Unauthorized;
    let msg = err.to_string();
    assert!(msg.contains("API key"));
    assert!(!msg.contains("sk-"));

    let err = LlmRequestError::Timeout;
    let msg = err.to_string();
    assert!(msg.contains("timed out"));

    let err = LlmRequestError::NetworkError;
    let msg = err.to_string();
    assert!(msg.contains("network") || msg.contains("connection"));

    let err = LlmRequestError::RateLimited;
    let msg = err.to_string();
    assert!(msg.contains("rate") || msg.contains("Try again"));
}

#[test]
fn completion_request_serialization() {
    let req = CompletionRequest {
        model: "gpt-4o-mini",
        messages: vec![Message {
            role: "user",
            content: "Hello",
        }],
        temperature: 0.7,
        max_tokens: 500,
    };
    
    let json = serde_json::to_string(&req).expect("serialize");
    assert!(json.contains("gpt-4o-mini"));
    assert!(json.contains("user"));
    assert!(json.contains("Hello"));
    assert!(json.contains("0.7"));
    assert!(json.contains("500"));
}

#[test]
fn completion_response_deserialization() {
    let json = r#"{
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "created": 1234567890,
        "model": "gpt-4o-mini",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello there!"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "total_tokens": 30
        }
    }"#;
    
    let resp: CompletionResponse = serde_json::from_str(json).expect("deserialize");
    assert_eq!(resp.choices.len(), 1);
    assert_eq!(resp.choices[0].message.as_ref().unwrap().content.as_deref(), Some("Hello there!"));
    assert_eq!(resp.usage.as_ref().unwrap().total_tokens, 30);
}

#[test]
fn completion_response_with_text_field() {
    // Some providers use text instead of message
    let json = r#"{
        "choices": [
            {
                "index": 0,
                "text": "Direct text response",
                "finish_reason": "stop"
            }
        ]
    }"#;
    
    let resp: CompletionResponse = serde_json::from_str(json).expect("deserialize");
    assert_eq!(resp.choices[0].text.as_deref(), Some("Direct text response"));
}

#[test]
fn models_response_deserialization() {
    let json = r#"{
        "data": [
            {"id": "gpt-4o-mini"},
            {"id": "gpt-3.5-turbo"}
        ]
    }"#;
    
    let resp: ModelsResponse = serde_json::from_str(json).expect("deserialize");
    assert_eq!(resp.data.len(), 2);
    assert_eq!(resp.data[0].id, "gpt-4o-mini");
}