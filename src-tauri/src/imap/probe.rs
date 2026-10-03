//! Shared probe core: one implementation, two thin callers.
//!
//! `connect_account` (Tauri command) and `imap_probe` (CLI binary) both run
//! through [`run_probe`]. Every wait is bounded by [`CONNECT_TIMEOUT_SECS`]
//! (30 s, CONTEXT D-timeout).

use std::time::Duration;

use super::session;
use super::{AccountConfig, ImapError, MailboxSummary, Transcript, CONNECT_TIMEOUT_SECS};

/// Mailbox summary plus the redacted command/response transcript.
#[derive(Debug)]
pub struct ProbeOutcome {
    pub summary: MailboxSummary,
    pub transcript: String,
}

fn validate(cfg: &AccountConfig) -> Result<(), ImapError> {
    if cfg.host.trim().is_empty() {
        return Err(ImapError::Protocol {
            detail: "invalid host: server address must not be empty".to_string(),
        });
    }
    if cfg.port == 0 {
        return Err(ImapError::Protocol {
            detail: "invalid port: must be in range 1-65535".to_string(),
        });
    }
    Ok(())
}

/// Full probe: connect, CAPABILITY, NAMESPACE, LIST, STATUS, SELECT INBOX.
pub async fn run_probe(cfg: &AccountConfig) -> Result<ProbeOutcome, ImapError> {
    validate(cfg)?;
    let mut t = Transcript::new();
    t.note(format!(
        "CONNECT {}:{} mode={}",
        cfg.host,
        cfg.port,
        cfg.security.as_str()
    ));

    let summary = async_std::future::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        session::open_inbox(cfg, &mut t),
    )
    .await
    .map_err(|_| ImapError::Timeout {
        host: cfg.host.clone(),
        port: cfg.port,
        secs: CONNECT_TIMEOUT_SECS,
    })??;

    Ok(ProbeOutcome {
        summary,
        // IN-07: render scrubs passwords of length >= 3 only — shorter
        // secrets would annihilate readable text; LOGIN is never echoed, so
        // this scrub is belt and suspenders either way.
        transcript: t.render(cfg),
    })
}

/// Pre-auth probe: connect, greeting, CAPABILITY only — no login.
/// Used for TLS diagnostics and the live server-profile header.
pub async fn run_pre_auth_probe(cfg: &AccountConfig) -> Result<String, ImapError> {
    validate(cfg)?;
    let mut t = Transcript::new();
    t.note(format!(
        "PRE-AUTH CONNECT {}:{} mode={}",
        cfg.host,
        cfg.port,
        cfg.security.as_str()
    ));

    async_std::future::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        session::pre_auth_caps(cfg, &mut t),
    )
    .await
    .map_err(|_| ImapError::Timeout {
        host: cfg.host.clone(),
        port: cfg.port,
        secs: CONNECT_TIMEOUT_SECS,
    })??;

    Ok(t.render(cfg))
}
