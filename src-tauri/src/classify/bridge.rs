//! Sidecar bridge (Phase 17, Plan 17-01) — single-flight
//! `POST /v1/systemone` client.
//!
//! Wire shape verified against `laya==0.4.2` (`sidecar/CONTRACT.md`):
//! request `{state, questions: {qid: {type, instructions, criteria,
//! controls...}}}`, response `{model, answers: {qid: {type, choice,
//! probabilities, confidence, x_jev_confidence?}}, usage}`.
//!
//! The bridge NEVER sees unredacted text (callers pass evidence output),
//! NEVER touches IMAP, and NEVER logs the API key.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Inference timeout: cold model + 1024-token multilingual rows need room;
/// the queue worker retries on timeout, the UI never waits on it.
pub const INFERENCE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("classifier is unreachable (sidecar down or still warming up)")]
    Unreachable,
    #[error("classifier timed out (mail too long or model overloaded)")]
    Timeout,
    #[error("classifier refused the request (oversized state or too many options)")]
    Refused(String),
    #[error("classifier answered in an unknown shape")]
    BadShape,
    #[error("sidecar I/O fault: {0}")]
    Io(String),
}

/// One `choice` answer with its full distribution (for runner-up).
#[derive(Debug, Clone)]
pub struct ChoiceOutcome {
    pub choice: String,
    pub confidence: f64,
    pub jev_confidence: Option<f64>,
    pub probabilities: HashMap<String, f64>,
}

impl ChoiceOutcome {
    /// Runner-up label (highest probability among non-winners).
    pub fn runner_up(&self) -> Option<String> {
        self.probabilities
            .iter()
            .filter(|(k, _)| *k != &self.choice)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k.clone())
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChoiceQuestion {
    #[serde(rename = "type")]
    kind: String,
    instructions: String,
    criteria: HashMap<String, Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_len: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SystemOneResponse {
    answers: HashMap<String, ChoiceAnswer>,
}

#[derive(Debug, Deserialize)]
struct ChoiceAnswer {
    #[serde(rename = "type")]
    kind: String,
    choice: Option<String>,
    confidence: Option<f64>,
    probabilities: Option<HashMap<String, f64>>,
    x_jev_confidence: Option<f64>,
}

/// Single-flight `/v1/systemone` client. The mutex serializes forward
/// passes (the server runs one worker — concurrent posts would starve
/// `/health` and each other).
pub struct Bridge {
    client: reqwest::Client,
    base_url: String,
    api_key: zeroize::Zeroizing<String>,
    flight: tokio::sync::Mutex<()>,
}

impl Bridge {
    pub fn new(base_url: &str, api_key: String) -> Result<Self, BridgeError> {
        let client = reqwest::Client::builder()
            .timeout(INFERENCE_TIMEOUT)
            .build()
            .map_err(|e| BridgeError::Io(e.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: zeroize::Zeroizing::new(api_key),
            flight: tokio::sync::Mutex::new(()),
        })
    }

    /// Ask one top-level `choice` question. `criteria` maps option label →
    /// optional pt-BR description (None keeps the wire minimal).
    pub async fn choice(
        &self,
        state: &str,
        qid: &str,
        instructions: &str,
        criteria: &HashMap<String, Option<String>>,
        max_len: Option<u32>,
    ) -> Result<ChoiceOutcome, BridgeError> {
        let _flight = self.flight.lock().await;
        let mut questions = HashMap::new();
        questions.insert(
            qid.to_string(),
            ChoiceQuestion {
                kind: "choice".to_string(),
                instructions: instructions.to_string(),
                criteria: criteria.clone(),
                max_len,
                lang: Some("pt".to_string()),
            },
        );
        let body = serde_json::json!({ "state": state, "questions": questions });
        let res = self
            .client
            .post(format!("{}/v1/systemone", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.as_str()),
            )
            // NOTE: no `json` feature on reqwest (loopback-only minimal
            // build) — serialize via serde_json directly.
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&body).unwrap_or_default())
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    BridgeError::Timeout
                } else {
                    BridgeError::Unreachable
                }
            })?;
        let status = res.status();
        if status.as_u16() == 400 || status.as_u16() == 413 || status.as_u16() == 422 {
            let detail = res.text().await.unwrap_or_default();
            let short: String = detail.chars().take(160).collect();
            return Err(BridgeError::Refused(short));
        }
        if !status.is_success() {
            return Err(BridgeError::Io(format!("status {status}")));
        }
        let text = res.text().await.map_err(|_| BridgeError::BadShape)?;
        let parsed: SystemOneResponse =
            serde_json::from_str(&text).map_err(|_| BridgeError::BadShape)?;
        let ans = parsed.answers.get(qid).ok_or(BridgeError::BadShape)?;
        if ans.kind != "choice" {
            return Err(BridgeError::BadShape);
        }
        Ok(ChoiceOutcome {
            choice: ans.choice.clone().ok_or(BridgeError::BadShape)?,
            confidence: ans.confidence.unwrap_or(0.0),
            jev_confidence: ans.x_jev_confidence,
            probabilities: ans.probabilities.clone().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Minimal canned HTTP stub (no extra deps): responds `body` to the
    /// first request on an ephemeral loopback port.
    fn stub_server(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = body.to_string();
        let status = status.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        format!("http://127.0.0.1:{port}")
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn parses_choice_with_runner_up() {
        // tokio runtime available? tauri brings tokio; the test harness runs
        // inside it via block_on on a fresh current-thread runtime.
        let url = stub_server(
            "200 OK",
            r#"{"model":"multilingual","answers":{"top":{"type":"choice","choice":"Financeiro",
            "confidence":0.81,"probabilities":{"Financeiro":0.81,"Acadêmico":0.12,"Outros":0.07},
            "x_jev_confidence":0.79}},"usage":{"input_tokens":40,"output_tokens":0}}"#,
        );
        let b = Bridge::new(&url, "k".to_string()).unwrap();
        let mut crit = HashMap::new();
        crit.insert("Financeiro".to_string(), None);
        crit.insert("Acadêmico".to_string(), None);
        let out = rt()
            .block_on(b.choice("boleto vencido", "top", "Qual a categoria?", &crit, Some(512)))
            .unwrap();
        assert_eq!(out.choice, "Financeiro");
        assert!((out.confidence - 0.81).abs() < 1e-9);
        assert_eq!(out.runner_up().as_deref(), Some("Acadêmico"));
    }

    #[test]
    fn maps_413_to_refused_and_500_to_io() {
        let url = stub_server("413 Payload Too Large", r#"{"detail":"state too large"}"#);
        let b = Bridge::new(&url, "k".to_string()).unwrap();
        let err = rt()
            .block_on(b.choice("x", "q", "i?", &HashMap::new(), None))
            .unwrap_err();
        assert!(matches!(err, BridgeError::Refused(_)), "got {err}");

        let url2 = stub_server("500 Internal Server Error", "inference failed");
        let b2 = Bridge::new(&url2, "k".to_string()).unwrap();
        let err2 = rt()
            .block_on(b2.choice("x", "q", "i?", &HashMap::new(), None))
            .unwrap_err();
        assert!(matches!(err2, BridgeError::Io(_)), "got {err2}");
    }

    #[test]
    fn unreachable_maps_to_unreachable_not_panic() {
        let b = Bridge::new("http://127.0.0.1:1", "k".to_string()).unwrap();
        let err = rt()
            .block_on(b.choice("x", "q", "i?", &HashMap::new(), None))
            .unwrap_err();
        assert!(matches!(err, BridgeError::Unreachable), "got {err}");
    }
}
