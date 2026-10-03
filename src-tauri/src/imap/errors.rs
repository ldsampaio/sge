//! Typed IMAP errors mapped to plain-language strings.
//!
//! Every variant names the failing part (host vs credentials vs TLS) so the
//! login UX can surface it verbatim with a manual Retry button. No variant
//! carries secret material — usernames appear, passwords never do.

use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImapError {
    #[error(
        "cannot reach {host}:{port} — check the server address and your network connection ({detail})"
    )]
    Unreachable {
        host: String,
        port: u16,
        detail: String,
    },

    #[error(
        "connection to {host}:{port} timed out after {secs} seconds — the server is not responding; check host and port, then retry"
    )]
    Timeout { host: String, port: u16, secs: u64 },

    #[error("login rejected for {username} — check the username and password, then retry")]
    AuthRejected { username: String },

    #[error(
        "TLS verification failed for {host} — the certificate is untrusted, expired, or issued for a different name ({detail}); nothing was sent"
    )]
    TlsUntrusted { host: String, detail: String },

    #[error(
        "certificate exception requested but refused — one-time exceptions require manual verification and are never applied silently; connection closed"
    )]
    CertExceptionRefused,

    #[error("mail server protocol error — the server did not answer as IMAP expects ({detail})")]
    Protocol { detail: String },

    #[error("INBOX could not be selected ({detail})")]
    SelectFailed { detail: String },
}

/// Pure classifier for transport error messages, so TLS vs network failures
/// name the right part even when the OS error kind is generic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoClass {
    Tls,
    Unreachable,
    Other,
}

pub(crate) fn sniff_io_message(msg: &str) -> IoClass {
    let m = msg.to_lowercase();
    let tls_markers = [
        "certificate",
        "tls",
        "ssl",
        "handshake",
        "alert unknown ca",
        "self signed",
        "verify failed",
    ];
    if tls_markers.iter().any(|mark| m.contains(mark)) {
        return IoClass::Tls;
    }
    let net_markers = [
        "refused",
        "no route",
        "network is unreachable",
        "failed to lookup",
        "nodename nor servname",
        "name or service not known",
        "name resolution",
        "dns",
        "timed out",
        "connection reset",
        "broken pipe",
        "connection aborted",
    ];
    if net_markers.iter().any(|mark| m.contains(mark)) {
        return IoClass::Unreachable;
    }
    IoClass::Other
}

/// Map a transport I/O error to the user-facing variant.
pub(crate) fn io_to_imap(err: io::Error, host: &str, port: u16) -> ImapError {
    let detail = err.to_string();
    match err.kind() {
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::NotConnected
        | io::ErrorKind::AddrNotAvailable
        | io::ErrorKind::AddrInUse
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::NotFound
        | io::ErrorKind::InvalidInput => ImapError::Unreachable {
            host: host.to_string(),
            port,
            detail,
        },
        io::ErrorKind::TimedOut => ImapError::Timeout {
            host: host.to_string(),
            port,
            secs: super::CONNECT_TIMEOUT_SECS,
        },
        _ => match sniff_io_message(&detail) {
            IoClass::Tls => ImapError::TlsUntrusted {
                host: host.to_string(),
                detail,
            },
            IoClass::Unreachable => ImapError::Unreachable {
                host: host.to_string(),
                port,
                detail,
            },
            IoClass::Other => ImapError::Protocol { detail },
        },
    }
}

/// Any `native-tls` handshake failure is, by construction, a TLS-layer
/// failure (TCP already connected) — always a verification problem, never
/// misreported as host or credentials.
pub(crate) fn tls_to_imap(err: native_tls::Error, host: &str) -> ImapError {
    ImapError::TlsUntrusted {
        host: host.to_string(),
        detail: err.to_string(),
    }
}

/// Map an `async-imap` error raised by LOGIN to the user-facing variant.
/// A `NO` answer to LOGIN is always an authentication rejection; transport
/// and parse failures keep their own names.
pub(crate) fn login_err(
    err: async_imap::error::Error,
    username: &str,
    host: &str,
    port: u16,
) -> ImapError {
    use async_imap::error::Error as Up;
    match err {
        Up::No(_) => ImapError::AuthRejected {
            username: username.to_string(),
        },
        Up::Bad(detail) => ImapError::Protocol { detail },
        Up::Io(io) => io_to_imap(io, host, port),
        Up::ConnectionLost => ImapError::Protocol {
            detail: "connection lost during login".to_string(),
        },
        Up::Parse(_) | Up::Validate(_) | Up::Append => ImapError::Protocol {
            detail: format!("login exchange failed ({})", short_upstream(&err)),
        },
        // async-imap Error is #[non_exhaustive]: future variants stay a
        // protocol error rather than misnaming the failing part.
        _ => ImapError::Protocol {
            detail: "login exchange failed".to_string(),
        },
    }
}

/// Map an `async-imap` error raised after login (LIST/STATUS/SELECT).
pub(crate) fn session_err(err: async_imap::error::Error, host: &str, port: u16) -> ImapError {
    use async_imap::error::Error as Up;
    match err {
        Up::No(detail) => ImapError::SelectFailed { detail },
        Up::Bad(detail) => ImapError::Protocol { detail },
        Up::Io(io) => io_to_imap(io, host, port),
        Up::ConnectionLost => ImapError::Protocol {
            detail: "connection lost".to_string(),
        },
        _ => ImapError::Protocol {
            detail: "mailbox command failed".to_string(),
        },
    }
}

fn short_upstream(err: &async_imap::error::Error) -> &'static str {
    use async_imap::error::Error as Up;
    match err {
        Up::Parse(_) => "unparseable server response",
        Up::Validate(_) => "invalid characters in credentials",
        Up::Append => "append rejected",
        _ => "unexpected response",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io(kind: io::ErrorKind, msg: &str) -> io::Error {
        io::Error::new(kind, msg)
    }

    #[test]
    fn refused_names_the_host() {
        let err = io_to_imap(
            io(io::ErrorKind::ConnectionRefused, "connection refused"),
            "mail.utfpr.edu.br",
            993,
        );
        let msg = err.to_string();
        assert!(matches!(err, ImapError::Unreachable { .. }), "{msg}");
        assert!(msg.contains("mail.utfpr.edu.br"), "{msg}");
        assert!(msg.contains("cannot reach"), "{msg}");
    }

    #[test]
    fn dns_failure_is_unreachable_not_tls() {
        let err = io_to_imap(
            io(io::ErrorKind::Other, "failed to lookup address information"),
            "bad.host",
            993,
        );
        assert!(matches!(err, ImapError::Unreachable { .. }), "{err}");
    }

    #[test]
    fn os_timeout_maps_to_timeout() {
        let err = io_to_imap(io(io::ErrorKind::TimedOut, "timed out"), "h", 143);
        let msg = err.to_string();
        assert!(matches!(err, ImapError::Timeout { .. }), "{msg}");
        assert!(msg.contains("timed out"), "{msg}");
    }

    #[test]
    fn cert_message_sniffs_to_tls() {
        assert_eq!(
            sniff_io_message("certificate verify failed (self signed certificate)"),
            IoClass::Tls
        );
        assert_eq!(sniff_io_message("tls handshake eof"), IoClass::Tls);
    }

    #[test]
    fn tls_error_names_tls_not_host() {
        let err = io_to_imap(
            io(io::ErrorKind::Other, "certificate verify failed"),
            "mail.utfpr.edu.br",
            993,
        );
        let msg = err.to_string();
        assert!(matches!(err, ImapError::TlsUntrusted { .. }), "{msg}");
        assert!(msg.contains("TLS verification failed"), "{msg}");
    }

    #[test]
    fn login_no_is_auth_rejection() {
        let err = login_err(
            async_imap::error::Error::No("AUTHENTICATIONFAILED bad user".into()),
            "alice",
            "h",
            993,
        );
        let msg = err.to_string();
        assert!(matches!(err, ImapError::AuthRejected { .. }), "{msg}");
        assert!(msg.contains("username and password"), "{msg}");
        assert!(!msg.contains("s3cret"), "must never echo secrets");
    }

    #[test]
    fn login_connection_lost_is_protocol() {
        let err = login_err(async_imap::error::Error::ConnectionLost, "alice", "h", 993);
        assert!(matches!(err, ImapError::Protocol { .. }), "{err}");
    }

    #[test]
    fn post_login_no_is_select_failure() {
        let err = session_err(
            async_imap::error::Error::No("mailbox not found".into()),
            "h",
            993,
        );
        assert!(matches!(err, ImapError::SelectFailed { .. }), "{err}");
    }
}
