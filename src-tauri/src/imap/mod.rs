//! IMAP connection core (Phase 1, Plan 01-02).
//!
//! Three security modes over `async-imap 0.11` + `async-native-tls` (system CA
//! store, so university certs validate without custom roots), driven on a
//! dedicated blocking thread via `async_std::task::block_on` — never on the
//! Tauri tokio runtime threads.
//!
//! M1 read-only invariant: this module issues only SELECT plus read-only
//! probes (CAPABILITY, NAMESPACE, LIST, STATUS). No flag writes, no
//! full-message fetch — bodies arrive in later phases and use peek-only
//! fetches, so M1 never sets `\Seen` on the server.

pub mod bodies;
pub mod errors;
pub mod headers;
pub mod probe;
pub mod session;

pub use errors::ImapError;

use futures::TryStreamExt;

/// Connection (and whole-probe) timeout, seconds. Locked by CONTEXT D-timeout.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

/// IMAP transport security. Frontend `SecuritySelector` values map 1:1 to
/// these variants (`implicit_tls` / `starttls` / `plain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityMode {
    /// Implicit TLS on connect (default, port 993).
    ImplicitTls,
    /// Plain connect upgraded with STARTTLS (port 143).
    StartTls,
    /// Unencrypted, localhost only, explicit confirm required.
    PlainLocal,
}

impl SecurityMode {
    /// Parse a frontend security string. Unknown values are a protocol-level
    /// client error, never a silent default.
    pub fn parse(s: &str) -> Result<Self, ImapError> {
        match s {
            "implicit_tls" | "implicit-tls" | "ssl" => Ok(SecurityMode::ImplicitTls),
            "starttls" | "start-tls" => Ok(SecurityMode::StartTls),
            "plain" | "plain-local" => Ok(SecurityMode::PlainLocal),
            other => Err(ImapError::Protocol {
                detail: format!(
                    "unknown security mode {other:?} — expected implicit_tls, starttls, or plain"
                ),
            }),
        }
    }

    /// Canonical wire value shared with the frontend selector.
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityMode::ImplicitTls => "implicit_tls",
            SecurityMode::StartTls => "starttls",
            SecurityMode::PlainLocal => "plain",
        }
    }
}

/// True for loopback hosts allowed to use unencrypted IMAP.
///
/// Allowlist is deliberately narrow (fail-closed): only `localhost`,
/// `127.0.0.1`, and `::1` (bracketed or bare) pass. The rest of `127.0.0.0/8`,
/// `::ffff:127.0.0.1`, and dotted forms like `localhost.` are refused — a
/// stub on `127.0.0.2` must use one of the listed names. Input should already
/// be trimmed (see [`normalize_host`]).
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_matches(['[', ']']);
    h.eq_ignore_ascii_case("localhost") || h == "127.0.0.1" || h == "::1"
}

/// Normalize a user-supplied hostname before it reaches TCP dial, DNS, or
/// TLS SNI/hostname verification.
///
/// Trims surrounding whitespace and splits off a trailing `:port` the user
/// may have pasted into the server field (e.g. `mail.utfpr.edu.br:993` —
/// without this the dial layer would treat the whole string as a hostname).
/// An explicitly pasted port is dropped in favor of the separate port
/// argument rather than silently overriding it. Bare IPv6 literals (multiple
/// colons, no brackets) are left intact for [`socket_addr`]-style bracketing
/// downstream.
pub fn normalize_host(raw: &str) -> Result<String, ImapError> {
    let host = raw.trim();
    if host.is_empty() {
        return Err(ImapError::Protocol {
            detail: "invalid host: server address must not be empty".to_string(),
        });
    }
    if let Some(stripped) = host.strip_prefix('[') {
        // Bracketed literal: "[::1]" or "[::1]:993".
        match stripped.find(']') {
            Some(end) => {
                let inner = &stripped[..end];
                let rest = &stripped[end + 1..];
                if !rest.is_empty()
                    && !(rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit()))
                {
                    return Err(ImapError::Protocol {
                        detail: format!("invalid host: malformed bracketed address {raw:?}"),
                    });
                }
                if inner.is_empty() {
                    return Err(ImapError::Protocol {
                        detail: "invalid host: server address must not be empty".to_string(),
                    });
                }
                return Ok(inner.to_string());
            }
            None => {
                return Err(ImapError::Protocol {
                    detail: format!("invalid host: unbalanced bracket in {raw:?}"),
                });
            }
        }
    }
    if host.contains(':') && !host.contains("::") && host.matches(':').count() == 1 {
        let (name, port_part) = host.rsplit_once(':').expect("single colon");
        if !port_part.is_empty()
            && port_part.chars().all(|c| c.is_ascii_digit())
            && !name.trim().is_empty()
        {
            // Trailing numeric port: use the hostname half; the numeric half
            // is intentionally dropped — the separate port argument wins.
            return Ok(name.trim().to_string());
        }
    }
    Ok(host.to_string())
}

/// Everything needed to open one IMAP INBOX session.
///
/// `password` is held in memory only for the session: never logged (the
/// manual [`fmt::Debug`] impl redacts it), never persisted here — keyring
/// persistence lands in Plan 01-03 behind remember-me consent. WR-10: the
/// bytes are [`zeroize::Zeroizing`] so allocator memory is scrubbed on drop
/// (cheap insurance against core-dump/swap exposure). Residual risk: copies
/// briefly exist inside TLS/IMAP plumbing and in the frontend JS string —
/// both outside Rust's reach — so this narrows, not eliminates, exposure.
#[derive(Clone)]
pub struct AccountConfig {
    pub host: String,
    pub port: u16,
    pub security: SecurityMode,
    pub username: String,
    pub password: zeroize::Zeroizing<String>,
    /// One-time cert exception requested by explicit user override. The
    /// backend NEVER silently bypasses verification for this flag (see
    /// [`probe::run_probe`]): it is logged, surfaced, and refused.
    pub allow_untrusted: bool,
    /// Plain-local requires explicit confirmation (CLI flag / localhost rule).
    pub plain_local_confirmed: bool,
}

impl std::fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &"***")
            .field("allow_untrusted", &self.allow_untrusted)
            .field("plain_local_confirmed", &self.plain_local_confirmed)
            .finish()
    }
}

/// Mailbox summary returned after a successful INBOX SELECT. Field names and
/// types are the IPC contract shared with `connect_account`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxSummary {
    pub selected_mailbox: String,
    pub uid_validity: u32,
    pub exists: u32,
    pub uid_next: Option<u32>,
}

/// A boxed async stream satisfying async-imap's transport bound —
/// `AsyncRead + AsyncWrite + Unpin + Debug + Send`.  Used to erase
/// TLS/TCP stream types behind `Client<BoxedStream>`.
///
/// Trait objects cannot contain multiple non-auto traits, so we bundle
/// the async-imap bound behind a sealed marker trait and erase to
/// `Box<dyn AsyncStream>`.
pub trait AsyncStream: futures::AsyncRead + futures::AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<T: futures::AsyncRead + futures::AsyncWrite + Unpin + std::fmt::Debug + Send> AsyncStream for T {}

/// A fully typed IMAP session over a [`BoxedStream`] — the concrete type
/// the sync engine owns and drives through [`SyncSession`].
pub type BoxedStream = Box<dyn AsyncStream>;
pub type BoxedSession = async_imap::Session<BoxedStream>;

/// Pinned boxed future alias consumed by [`SyncSession`] trait methods.
/// The trait stays object-safe because every method returns this
/// concrete `Pin<Box<dyn Future …>>` rather than `impl Future`.
pub type PinBox<'a, T> = std::pin::Pin<std::boxed::Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Errors from the sync-path IMAP layer (Phase 2+).
///
/// `SyncError` is intentionally coarse at this layer — most variants carry
/// a diagnostic string that the IPC layer maps back to a structured
/// `SyncError` enum for the frontend.
#[derive(Debug)]
pub enum SyncError {
    /// Protocol-level failure (FETCH/SELECT/LOGOUT returned an error).
    Protocol(String),
    /// I/O failure during a body fetch or attachment write.
    Io(String),
    /// Parse failure (envelope, bodystructure, or RFC822 parse).
    Parse(String),
    /// Invariants violation (e.g. missing UIDVALIDITY, unexpected state).
    State(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Protocol(s) => write!(f, "IMAP protocol error: {s}"),
            SyncError::Io(s) => write!(f, "IMAP I/O error: {s}"),
            SyncError::Parse(s) => write!(f, "IMAP parse error: {s}"),
            SyncError::State(s) => write!(f, "IMAP state error: {s}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<crate::store::StoreError> for SyncError {
    fn from(e: crate::store::StoreError) -> Self {
        SyncError::Protocol(format!("store: {e}"))
    }
}

impl From<async_imap::error::Error> for SyncError {
    fn from(e: async_imap::error::Error) -> Self {
        SyncError::Protocol(format!("{e}"))
    }
}

/// Object-safe trait abstracting a read-only IMAP INBOX session.
///
/// Implemented by:
/// - [`BoxedSession`] — real IMAP session (Phase 1 connection core)
/// - `MockSession` (in `sync::worker::tests`) — deterministic fixture
///
/// All methods are read-only (ENVELOPE sweeps, `BODY.PEEK[]` bodies,
/// SELECT, LOGOUT). `STORE`, `EXPUNGE`, and `APPEND` are **never**
/// exposed, honoring the M1 read-only invariant (D-flags: T-02-01).
pub trait SyncSession: Unpin + Send {
    /// `SELECT INBOX` — validates the mailbox is selectable and returns
    /// UIDVALIDITY / UIDNEXT / exists counts.
    fn select_inbox(&mut self) -> PinBox<'_, Result<MailboxSummary, SyncError>>;

    /// `UID FETCH <range> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)`
    /// over a ≤200-UID range.  Returns parsed headers (never raw `Fetch`).
    fn fetch_envelopes<'a>(
        &'a mut self,
        range: &'a str,
    ) -> PinBox<'a, Result<Vec<headers::MessageHeader>, SyncError>>;

    /// `UID FETCH <uid> BODY.PEEK[]` — fetch the full RFC822 message
    /// bytes for a single UID. Never sets `\\Seen`.
    fn fetch_body(&mut self, uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>>;

    /// Graceful `LOGOUT`.  Idempotent on error.
    fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>>;
}

impl SyncSession for BoxedSession {
    fn select_inbox(&mut self) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
        Box::pin(async move {
            let mailbox = self
                .select("INBOX")
                .await
                .map_err(|e| SyncError::Protocol(format!("SELECT: {e}")))?;
            Ok(MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: mailbox
                    .uid_validity
                    .ok_or_else(|| SyncError::State("server did not return UIDVALIDITY".into()))?,
                uid_next: mailbox.uid_next,
                exists: mailbox.exists,
            })
        })
    }

    fn fetch_envelopes<'a>(
        &'a mut self,
        range: &'a str,
    ) -> PinBox<'a, Result<Vec<headers::MessageHeader>, SyncError>> {
        let range_owned = range.to_string();
        Box::pin(async move {
            let attrs = "UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE";
            let mut stream = self
                .uid_fetch(&range_owned, attrs)
                .await
                .map_err(|e| SyncError::Protocol(format!("UID FETCH: {e}")))?;
            let mut out = Vec::new();
            while let Some(fetch) = stream.try_next().await? {
                let uid = fetch
                    .uid
                    .ok_or_else(|| SyncError::Protocol("missing UID in FETCH response".into()))?;
                out.push(headers::parse_fetch_item(&fetch, uid));
            }
            Ok(out)
        })
    }

    fn fetch_body(&mut self, uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
        let uid_str = uid.to_string();
        Box::pin(async move {
            let mut stream = self
                .uid_fetch(&uid_str, "BODY.PEEK[]")
                .await
                .map_err(|e| SyncError::Protocol(format!("UID FETCH body: {e}")))?;
            while let Some(fetch) = stream.try_next().await? {
                if let Some(body) = fetch.body() {
                    return Ok(body.to_vec());
                }
            }
            Ok(Vec::new())
        })
    }

    fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            self.logout()
                .await
                .map_err(|e| SyncError::Protocol(format!("LOGOUT: {e}")))?;
            Ok(())
        })
    }
}

/// Append-only command/response transcript for the probe fixture.
///
/// Secrets never enter the transcript (only CAPABILITY/NAMESPACE/LIST/STATUS/
/// SELECT exchanges are echoed), and [`Transcript::render`] redacts any
/// accidental occurrence of the username/password as belt and suspenders.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Vec<String>,
}

impl Transcript {
    pub fn new() -> Self {
        Transcript { lines: Vec::new() }
    }

    /// Client-side note (connect line, warnings). Must never contain secrets.
    pub fn note(&mut self, line: impl Into<String>) {
        self.lines.push(format!("# {}", line.into()));
    }

    /// Echo of a client command (never a LOGIN line — auth is issued inside
    /// async-imap and never echoed here).
    pub fn client(&mut self, line: impl Into<String>) {
        self.lines.push(format!("C: {}", line.into()));
    }

    /// One server response line.
    pub fn server(&mut self, line: impl Into<String>) {
        self.lines.push(format!("S: {}", line.into()));
    }

    /// Render with password occurrences replaced by `***`.
    ///
    /// Only the password is scrubbed: it is the actual secret, and it never
    /// enters the transcript by construction (LOGIN is issued inside
    /// async-imap and never echoed), so this is belt and suspenders. The
    /// username is intentionally left intact — short usernames would
    /// annihilate readable text, and the username already appears in
    /// user-facing error strings by design. Passwords shorter than 3 chars
    /// are skipped to avoid the same annihilation problem.
    pub fn render(&self, cfg: &AccountConfig) -> String {
        let mut out = self.lines.join("\n");
        if cfg.password.len() >= 3 {
            out = out.replace(cfg.password.as_str(), "***");
        }
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_constant_is_30s() {
        assert_eq!(CONNECT_TIMEOUT_SECS, 30);
    }

    #[test]
    fn security_mode_roundtrips() {
        for mode in [
            SecurityMode::ImplicitTls,
            SecurityMode::StartTls,
            SecurityMode::PlainLocal,
        ] {
            assert_eq!(SecurityMode::parse(mode.as_str()).unwrap(), mode);
        }
    }

    #[test]
    fn security_mode_rejects_junk() {
        assert!(SecurityMode::parse("ssl-tls-maybe").is_err());
        assert!(SecurityMode::parse("").is_err());
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("LOCALHOST"));
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("::1"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("mail.utfpr.edu.br"));
        assert!(!is_loopback(""));
    }

    #[test]
    fn normalize_host_trims_and_strips_pasted_port() {
        // WR-03: the value reaching dial/TLS SNI must be clean.
        assert_eq!(
            normalize_host("  mail.utfpr.edu.br  ").unwrap(),
            "mail.utfpr.edu.br"
        );
        assert_eq!(
            normalize_host("mail.utfpr.edu.br:993").unwrap(),
            "mail.utfpr.edu.br"
        );
        assert_eq!(
            normalize_host("  mail.utfpr.edu.br:993  ").unwrap(),
            "mail.utfpr.edu.br"
        );
        assert_eq!(normalize_host("[::1]").unwrap(), "::1");
        assert_eq!(normalize_host("[::1]:143").unwrap(), "::1");
        // Bare IPv6 is left intact for downstream bracketing.
        assert_eq!(normalize_host("::1").unwrap(), "::1");
        assert!(normalize_host("   ").is_err());
        assert!(normalize_host("[::1").is_err());
    }

    #[test]
    fn transcript_redacts_secrets() {
        let cfg = AccountConfig {
            host: "mail.example".into(),
            port: 993,
            security: SecurityMode::ImplicitTls,
            username: "user1".into(),
            password: zeroize::Zeroizing::new("s3cret-pw".to_string()),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let mut t = Transcript::new();
        t.server("* OK ready");
        // Simulate a leaked line; render must still scrub the password.
        // (The username is intentionally NOT scrubbed: short names would
        // annihilate readable text, and it appears in error strings anyway.)
        t.server("leaked s3cret-pw here");
        let rendered = t.render(&cfg);
        assert!(
            !rendered.contains("s3cret-pw"),
            "password leaked: {rendered}"
        );
        assert!(rendered.contains("***"));
    }

    #[test]
    fn account_debug_redacts_password() {
        let cfg = AccountConfig {
            host: "h".into(),
            port: 993,
            security: SecurityMode::ImplicitTls,
            username: "u".into(),
            password: zeroize::Zeroizing::new("pw123".to_string()),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("pw123"), "password in Debug: {dbg}");
    }
}
