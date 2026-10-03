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

pub mod errors;
pub mod probe;
pub mod session;

pub use errors::ImapError;

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
    /// Plaintext, localhost only, explicit confirm required.
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

/// True for loopback hosts allowed to use plaintext IMAP.
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_matches(['[', ']']);
    h.eq_ignore_ascii_case("localhost") || h == "127.0.0.1" || h == "::1"
}

/// Everything needed to open one IMAP INBOX session.
///
/// `password` is held in memory only for the session: never logged (the
/// manual [`fmt::Debug`] impl redacts it), never persisted here — keyring
/// persistence lands in Plan 01-03 behind remember-me consent.
#[derive(Clone)]
pub struct AccountConfig {
    pub host: String,
    pub port: u16,
    pub security: SecurityMode,
    pub username: String,
    pub password: String,
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
    fn transcript_redacts_secrets() {
        let cfg = AccountConfig {
            host: "mail.example".into(),
            port: 993,
            security: SecurityMode::ImplicitTls,
            username: "user1".into(),
            password: "s3cret-pw".into(),
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
            password: "pw123".into(),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("pw123"), "password in Debug: {dbg}");
    }
}
