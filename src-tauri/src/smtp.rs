//! SMTP send transport over lettre 0.11 (Phase 13, Plan 13-02).
//!
//! The durable queue from Plan 13-01 becomes real delivery here. This module
//! owns the only network-send path in the app:
//!
//! - [`SmtpTransport`] trait: `send_raw(envelope_from, envelope_to, bytes)`
//!   returning [`SmtpOutcome`] — `Sent`, `Uncertain`, `Transient`, or
//!   `Permanent`. `FakeTransport` in the worker tests implements the same
//!   trait (record-and-replay, mirroring the `FakeSession`/`MockSession`
//!   pattern).
//! - [`SmtpTransporter`]: the lettre-backed implementation using the sync
//!   SMTP transport with a per-account client pool. Sync means blocking —
//!   every call MUST run on a blocking thread (`spawn_blocking`, the
//!   `creds.rs` STACK hard rule), never on an async runtime thread. The
//!   production flush path (`start_sync`) already runs the whole worker
//!   inside `spawn_blocking`, so the SMTP leg stays off the runtime there.
//! - Fail-closed STARTTLS: clients build exclusively through
//!   `starttls_relay` (upgrade failure is a hard error — credentials and
//!   mail bytes never cross plaintext; see the `fail_closed` tripwire test).
//! - Credentials arrive per call in [`SmtpAccount`] (loaded by the caller
//!   from the existing keyring path / in-memory account — no new secret
//!   storage here) and are never formatted into errors or logs (Phase 10
//!   discipline: hosts with ports are fine, secrets never are).
//! - Ambiguity rule (T-13-07): anything that might have reached the server
//!   (timeouts, connection breaks mid-send, unclassed replies) maps to
//!   [`SmtpOutcome::Uncertain`], never to success and never to a blind
//!   retry — the flush layer reconciles via Sent SEARCH before any re-send.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::Credentials;

/// Per-command SMTP timeout (seconds): every command — connect, EHLO,
/// STARTTLS upgrade, AUTH, MAIL/RCPT/DATA — must answer inside this window.
/// A DATA-timeout maps to [`SmtpOutcome::Uncertain`] (the server may already
/// have accepted the bytes), never to success.
pub const SMTP_COMMAND_TIMEOUT_SECS: u64 = 60;

/// SMTP account for one send: server + credentials. Built by the caller from
/// the existing keyring path (`KeyringStore::load`) or the in-memory
/// `ActiveAccount` — this module stores no secrets itself.
///
/// The custom [`std::fmt::Debug`] impl prints host/port/username only: the
/// password never appears in logs, errors, or snapshots.
pub struct SmtpAccount {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl SmtpAccount {
    /// Pool identity: `host:port:username` — the exact shape of
    /// [`crate::imap::manager::SessionManager::account_key`] (never secrets).
    pub fn key(&self) -> String {
        SmtpTransporter::account_key(&self.host, self.port, &self.username)
    }
}

impl std::fmt::Debug for SmtpAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpAccount")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Verdict of one SMTP send attempt — the single vocabulary the flush layer
/// reasons about (T-13-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpOutcome {
    /// The server accepted the bytes (2xx to the final DATA `.`).
    Sent,
    /// The bytes MAY be on the server (timeout, mid-send connection break,
    /// unclassed reply): reconcile via Sent SEARCH before any re-send.
    Uncertain { reason: String },
    /// Provably unsent and worth retrying (4xx reply, TLS-upgrade failure,
    /// client-setup failure): backoff schedule, attempts counted.
    Transient { message: String },
    /// Provably unsent and never worth auto-retrying (5xx reply, bad
    /// envelope): terminal `failed`, manual retry only.
    Permanent { message: String },
}

/// Blocking send of pre-rendered `.eml` bytes with an explicit envelope.
///
/// Sync by design (lettre's `SmtpTransport` is blocking): implementations
/// MUST be called on a blocking thread only. `Send + Sync` so a pooled
/// transporter can sit behind an `Arc` across flush passes.
pub trait SmtpTransport: Send + Sync {
    fn send_raw(
        &self,
        account: &SmtpAccount,
        envelope_from: &str,
        envelope_to: &[String],
        bytes: &[u8],
    ) -> SmtpOutcome;
}

/// Transport-level signals extracted from one lettre SMTP error, feeding the
/// pure [`classify_fault`] below. Split out so the mapping is unit-testable
/// without a live server (every arm is pinned by tests).
#[derive(Debug, Clone, Default)]
pub struct SmtpFault {
    pub timeout: bool,
    pub response: bool,
    pub transient: bool,
    pub permanent: bool,
    pub tls: bool,
    pub client: bool,
    /// Numeric SMTP reply digits only (e.g. `"550"`) — sanitized at the
    /// boundary, safe to surface in plain-language errors.
    pub code: Option<String>,
}

/// Pure verdict mapping (T-13-07): timeouts and connection-class failures are
/// [`SmtpOutcome::Uncertain`]; classified replies follow their 4xx/5xx
/// class; TLS-upgrade failures are [`SmtpOutcome::Transient`] (retryable,
/// never a plaintext fallback); client/envelope errors are
/// [`SmtpOutcome::Permanent`]. Messages are static plain-language strings —
/// no secret, address, or body content ever flows through here.
pub fn classify_fault(f: &SmtpFault) -> SmtpOutcome {
    if f.timeout {
        return SmtpOutcome::Uncertain {
            reason: "timed out while sending — the server may already have \
                     accepted the message; it will be checked in Sent \
                     before any re-send"
                .to_string(),
        };
    }
    if f.response {
        let code = f.code.as_deref().unwrap_or("?");
        if f.permanent {
            return SmtpOutcome::Permanent {
                message: format!(
                    "the mail server refused the message (SMTP {code}) — \
                     it will not be retried automatically"
                ),
            };
        }
        if f.transient {
            return SmtpOutcome::Transient {
                message: format!(
                    "the mail server temporarily refused the message \
                     (SMTP {code}) — retrying automatically with backoff"
                ),
            };
        }
        return SmtpOutcome::Uncertain {
            reason: format!(
                "unclear server reply (SMTP {code}) — it will be checked \
                 in Sent before any re-send"
            ),
        };
    }
    if f.tls {
        return SmtpOutcome::Transient {
            message: "could not upgrade to an encrypted connection — \
                      retrying automatically (never sending unencrypted)"
                .to_string(),
        };
    }
    if f.client {
        return SmtpOutcome::Permanent {
            message: "the message could not be sent as addressed — it will \
                      not be retried automatically"
                .to_string(),
        };
    }
    // Connection / I/O / pool-shutdown failures mid-send: bytes may already
    // have reached the server, so this is reconcile-not-resend, never a
    // blind retry (a pure connect refusal also lands here — the cost is one
    // Sent SEARCH that misses and requeues with backoff, never a duplicate).
    SmtpOutcome::Uncertain {
        reason: "the connection broke while sending — the message may \
                 already be on the server; it will be checked in Sent \
                 before any re-send"
            .to_string(),
    }
}

/// Keep only ASCII digits (SMTP reply codes are three digits): whatever the
/// server sent back, the surfaced text can never carry content or secrets.
fn sanitize_code(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.chars().take(3).collect()
}

/// Map one lettre SMTP error onto [`SmtpOutcome`] via [`classify_fault`].
/// The lettre `Display` is never surfaced — verdicts carry only static
/// plain-language text plus the sanitized numeric reply code.
pub fn map_lettre_error(e: &lettre::transport::smtp::Error) -> SmtpOutcome {
    classify_fault(&SmtpFault {
        timeout: e.is_timeout(),
        response: e.is_response(),
        transient: e.is_transient(),
        permanent: e.is_permanent(),
        tls: e.is_tls(),
        client: e.is_client(),
        code: e.status().map(|c| sanitize_code(&c.to_string())),
    })
}

/// Build the SMTP envelope: `MAIL FROM:<from>` plus one `RCPT TO` per
/// address in `to` (the caller combines To + Cc + BCC — BCC rides the
/// envelope only, never the headers, per the enqueue path). Pure: no
/// network, so bad addresses fail here as [`SmtpOutcome::Permanent`]
/// without ever dialing. Bare addr-specs only (`user@host` — display names
/// fail closed as unaddressable rather than being guessed at).
pub fn build_envelope(envelope_from: &str, envelope_to: &[String]) -> Result<Envelope, String> {
    let from: Address = envelope_from.trim().parse().map_err(|_| {
        "the sender address is invalid — the message will not be retried \
         automatically"
            .to_string()
    })?;
    let mut rcpts: Vec<Address> = Vec::with_capacity(envelope_to.len());
    for addr in envelope_to {
        let trimmed = addr.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rcpt: Address = trimmed.parse().map_err(|_| {
            "a recipient address is invalid — the message will not be \
             retried automatically"
                .to_string()
        })?;
        rcpts.push(rcpt);
    }
    if rcpts.is_empty() {
        return Err("there is no recipient left to send to — the message \
                    will not be retried automatically"
            .to_string());
    }
    // De-duplicate exact repeats (To+Cc overlap): a second RCPT TO for the
    // same mailbox is harmless on most servers but refused on some.
    rcpts.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
    rcpts.dedup_by(|a, b| a.to_string() == b.to_string());
    Envelope::new(Some(from), rcpts).map_err(|e| {
        format!(
            "the send envelope could not be addressed ({e}) — the message \
             will not be retried automatically"
        )
    })
}

/// Lettre-backed [`SmtpTransport`]: sync SMTP over fail-closed STARTTLS with
/// a per-account client pool (lettre recycles pooled connections across
/// sends; a fresh instance per send would reconnect every time).
pub struct SmtpTransporter {
    pool: Mutex<HashMap<String, lettre::SmtpTransport>>,
    timeout_secs: u64,
}

impl Default for SmtpTransporter {
    fn default() -> Self {
        Self::new()
    }
}

impl SmtpTransporter {
    pub fn new() -> Self {
        Self {
            pool: Mutex::new(HashMap::new()),
            timeout_secs: SMTP_COMMAND_TIMEOUT_SECS,
        }
    }

    /// Pool identity shared with the IMAP side (`host:port:username` —
    /// [`crate::imap::manager::SessionManager::account_key`] shape).
    pub fn account_key(host: &str, port: u16, username: &str) -> String {
        format!("{host}:{port}:{username}")
    }

    /// Fail-closed client for `account`: STARTTLS upgrade required
    /// (`starttls_relay` errors when the server will not upgrade — that
    /// error surfaces as a hard failure, never a plaintext send). Cached by
    /// account key so repeat sends reuse the pooled connection.
    fn client_for(&self, account: &SmtpAccount) -> Result<lettre::SmtpTransport, String> {
        let key = account.key();
        if let Some(cached) = self.pool.lock().expect("smtp pool lock").get(&key) {
            return Ok(cached.clone());
        }
        // Constructor failure here means the TLS parameters themselves are
        // unusable (unparseable host) — a configuration problem, retryable
        // only by fixing the address, and never a reason to drop to
        // plaintext. Host:port is fine to name (Phase 10 discipline);
        // credentials are never formatted.
        let transport = lettre::SmtpTransport::starttls_relay(&account.host)
            .map_err(|_| {
                format!(
                    "could not set up an encrypted connection to {}:{} — \
                     check the server address",
                    account.host, account.port
                )
            })?
            .port(account.port)
            .credentials(Credentials::new(
                account.username.clone(),
                account.password.clone(),
            ))
            .timeout(Some(Duration::from_secs(self.timeout_secs)))
            .build();
        self.pool
            .lock()
            .expect("smtp pool lock")
            .insert(key, transport.clone());
        Ok(transport)
    }

    #[cfg(test)]
    fn pool_len(&self) -> usize {
        self.pool.lock().expect("smtp pool lock").len()
    }
}

// Manual Debug: the pool holds authenticated clients — print depth only,
// never hosts, users, or anything credential-adjacent.
impl std::fmt::Debug for SmtpTransporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpTransporter")
            .field(
                "pooled_clients",
                &self.pool.lock().map(|p| p.len()).unwrap_or(0),
            )
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl SmtpTransport for SmtpTransporter {
    fn send_raw(
        &self,
        account: &SmtpAccount,
        envelope_from: &str,
        envelope_to: &[String],
        bytes: &[u8],
    ) -> SmtpOutcome {
        // Envelope first: addressing mistakes fail before any dial.
        let envelope = match build_envelope(envelope_from, envelope_to) {
            Ok(env) => env,
            Err(message) => return SmtpOutcome::Permanent { message },
        };
        let transport = match self.client_for(account) {
            Ok(t) => t,
            Err(message) => {
                return SmtpOutcome::Transient { message };
            }
        };
        match lettre::Transport::send_raw(&transport, &envelope, bytes) {
            Ok(_) => SmtpOutcome::Sent,
            Err(e) => map_lettre_error(&e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fault() -> SmtpFault {
        SmtpFault::default()
    }

    #[test]
    fn fail_closed_starttls_never_plaintext() {
        // T-13-05 tripwire (mirrors the NAMESPACE parser tripwire): the only
        // approved constructor is the fail-closed STARTTLS one. Needles are
        // built, not written, so this very test never self-matches.
        const SRC: &str = include_str!("smtp.rs");
        let fail_closed = ["starttls", "_relay"].join("");
        assert!(
            SRC.contains(&fail_closed),
            "transport must build through the fail-closed STARTTLS constructor"
        );
        for needle in [
            ["Tls::Opport", "unistic"].join(""),
            ["builder_danger", "ous"].join(""),
            ["unencrypted_local", "host"].join(""),
            ["Tls::", "None"].join(""),
            ["Transport::", "relay("].join(""),
        ] {
            for (i, line) in SRC.lines().enumerate() {
                assert!(
                    !line.contains(&needle),
                    "smtp.rs line {} uses a fail-open/plaintext constructor: {line:?}",
                    i + 1
                );
            }
        }
    }

    #[test]
    fn data_timeout_and_mid_send_break_map_to_uncertain() {
        // The core T-13-07 guarantee: ambiguity never resolves to success
        // and never to a blind retry.
        let mut f = fault();
        f.timeout = true;
        assert!(
            matches!(classify_fault(&f), SmtpOutcome::Uncertain { .. }),
            "DATA-timeout must reconcile, never blind-retry"
        );
        // All-false = connection-class failure (connect refusal through
        // drop-after-send): provably-ambiguous, so uncertain too.
        let f = fault();
        assert!(
            matches!(classify_fault(&f), SmtpOutcome::Uncertain { .. }),
            "mid-send connection break must reconcile, never blind-retry"
        );
        // Unclassed server reply: uncertain as well.
        let mut f = fault();
        f.response = true;
        f.code = Some("250".to_string());
        assert!(
            matches!(classify_fault(&f), SmtpOutcome::Uncertain { .. }),
            "unclassed reply must reconcile"
        );
    }

    #[test]
    fn reply_class_drives_transient_vs_permanent() {
        let mut f = fault();
        f.response = true;
        f.transient = true;
        f.code = Some("421".to_string());
        match classify_fault(&f) {
            SmtpOutcome::Transient { message } => assert!(message.contains("421")),
            other => panic!("4xx must be transient, got {other:?}"),
        }
        let mut f = fault();
        f.response = true;
        f.permanent = true;
        f.code = Some("550".to_string());
        match classify_fault(&f) {
            SmtpOutcome::Permanent { message } => assert!(message.contains("550")),
            other => panic!("5xx must be permanent, got {other:?}"),
        }
        let mut f = fault();
        f.tls = true;
        match classify_fault(&f) {
            SmtpOutcome::Transient { message } => {
                assert!(message.contains("encrypted"), "{message}")
            }
            other => panic!("TLS failure must be transient-never-plaintext, got {other:?}"),
        }
        let mut f = fault();
        f.client = true;
        assert!(
            matches!(classify_fault(&f), SmtpOutcome::Permanent { .. }),
            "client/envelope errors must be terminal"
        );
    }

    #[test]
    fn verdicts_never_leak_secrets() {
        // T-13-06: battery every arm with hostile values present; the
        // password must never surface (host:port naming is allowed).
        let password = "s3cret-pw-xyz";
        let username = "alice@utfpr.edu.br";
        let mut cases: Vec<SmtpOutcome> = Vec::new();
        for mut f in [
            {
                let mut f = fault();
                f.timeout = true;
                f
            },
            {
                let mut f = fault();
                f.response = true;
                f.permanent = true;
                f.code = Some(format!("550 {password}"));
                f
            },
            {
                let mut f = fault();
                f.response = true;
                f.transient = true;
                f.code = Some("421".to_string());
                f
            },
            {
                let mut f = fault();
                f.tls = true;
                f
            },
            {
                let mut f = fault();
                f.client = true;
                f
            },
            fault(),
        ] {
            // Sanitize like the real boundary does before classifying.
            f.code = f.code.map(|c| sanitize_code(&c));
            cases.push(classify_fault(&f));
        }
        cases.push(build_envelope(password, &[username.to_string()]).map_or_else(
            |message| SmtpOutcome::Permanent { message },
            |_| SmtpOutcome::Sent,
        ));
        for outcome in &cases {
            let text = format!("{outcome:?}");
            assert!(!text.contains(password), "secret leaked: {text}");
        }
        // Reply-code sanitizer keeps digits only.
        assert_eq!(sanitize_code("550"), "550");
        assert_eq!(sanitize_code("550-5.1.1 foo"), "550");
        assert_eq!(sanitize_code(password), "3");
    }

    #[test]
    fn envelope_parses_trims_and_dedupes_without_network() {
        let env = build_envelope(
            " eu@utfpr.edu.br ",
            &[
                "amigo@example.com".to_string(),
                " copia@example.com".to_string(),
                "amigo@example.com".to_string(),
            ],
        )
        .expect("valid envelope must build with no network");
        assert_eq!(env.to().len(), 2, "exact duplicate RCPT must collapse");
        assert!(build_envelope("eu@utfpr.edu.br", &[]).is_err());
        assert!(build_envelope("eu@utfpr.edu.br", &[" ".to_string()]).is_err());
        assert!(build_envelope("not-an-address", &["a@x.com".to_string()]).is_err());
        assert!(build_envelope("eu@utfpr.edu.br", &["bad@@".to_string()]).is_err());
    }

    #[test]
    fn pool_caches_one_client_per_account_key() {
        // Lazy build: no socket is opened, so this runs with no network.
        let tx = SmtpTransporter::new();
        let acc = SmtpAccount {
            host: "smtp.utfpr.edu.br".to_string(),
            port: 587,
            username: "alice".to_string(),
            password: "pw".to_string(),
        };
        assert_eq!(SmtpTransporter::account_key("h", 587, "u"), "h:587:u");
        assert_eq!(acc.key(), "smtp.utfpr.edu.br:587:alice");
        tx.client_for(&acc).expect("client build is lazy");
        tx.client_for(&acc).expect("second build reuses the pool");
        assert_eq!(tx.pool_len(), 1, "same account must reuse one client");
        let bob = SmtpAccount {
            host: "smtp.utfpr.edu.br".to_string(),
            port: 587,
            username: "bob".to_string(),
            password: "pw".to_string(),
        };
        tx.client_for(&bob).expect("second account builds");
        assert_eq!(tx.pool_len(), 2, "distinct accounts pool separately");
        // Debug impls never carry the password.
        assert!(!format!("{acc:?}").contains("pw"));
        assert!(!format!("{tx:?}").contains("smtp.utfpr.edu.br"));
    }
}
