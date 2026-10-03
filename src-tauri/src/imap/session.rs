//! IMAP transports plus the authenticated probe sequence.
//!
//! Mode constructors establish a [`Client`] over the right transport, then the
//! shared [`drive`] runs the read-only probe: CAPABILITY, NAMESPACE, LIST,
//! STATUS, SELECT INBOX, logout. No flag writes, no full-message fetch
//! anywhere in M1.

use async_imap::imap_proto::{Response, Status};
use async_imap::{Client, Session};
use async_std::net::TcpStream;
use futures::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use futures::{AsyncRead, AsyncWrite, TryStreamExt};

use super::errors::{io_to_imap, login_err, session_err, tls_to_imap};
use super::{AccountConfig, ImapError, MailboxSummary, SecurityMode, Transcript};

/// Bound shared by every stream handed to async-imap.
pub trait StreamBound: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send> StreamBound for T {}

/// Loopback-only unencrypted-mode rule shared by the command and the CLI.
pub fn check_plain_allowed(cfg: &AccountConfig) -> Result<(), ImapError> {
    if !super::is_loopback(&cfg.host) {
        return Err(ImapError::Protocol {
            detail: format!(
                "plain-local mode refused for non-localhost host {:?} — unencrypted IMAP must stay on localhost",
                cfg.host
            ),
        });
    }
    if !cfg.plain_local_confirmed {
        return Err(ImapError::Protocol {
            detail: "plain-local mode requires an explicit local-only confirmation".to_string(),
        });
    }
    Ok(())
}

/// Cert-exception policy: an explicit override is logged and surfaced, and
/// then REFUSED — verification is never bypassed silently (grep gate:
/// no insecure-cert bypass anywhere in this tree).
pub fn check_cert_policy(cfg: &AccountConfig) -> Result<(), ImapError> {
    if cfg.allow_untrusted {
        eprintln!(
            "WARNING: certificate exception requested for {} — refusing to bypass TLS verification",
            cfg.host
        );
        return Err(ImapError::CertExceptionRefused);
    }
    Ok(())
}

fn socket_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

async fn tcp_connect(cfg: &AccountConfig) -> Result<TcpStream, ImapError> {
    // async-std DNS + connect; the outer probe timeout bounds the whole wait.
    async_std::net::TcpStream::connect(socket_addr(&cfg.host, cfg.port).as_str())
        .await
        .map_err(|e| io_to_imap(e, &cfg.host, cfg.port))
}

async fn tls_upgrade(
    cfg: &AccountConfig,
    tcp: TcpStream,
) -> Result<async_native_tls::TlsStream<TcpStream>, ImapError> {
    async_native_tls::connect(cfg.host.as_str(), tcp)
        .await
        .map_err(|e| tls_to_imap(e, &cfg.host))
}

/// Read the mandatory server greeting after connect (implicit TLS / plain).
async fn read_greeting<S: StreamBound>(
    client: &mut Client<S>,
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<(), ImapError> {
    let rd = client
        .read_response()
        .await
        .map_err(|e| io_to_imap(e, &cfg.host, cfg.port))?
        .ok_or_else(|| ImapError::Protocol {
            detail: "server closed the connection without an IMAP greeting".to_string(),
        })?;
    t.server(format!("{:?}", rd.parsed()));
    Ok(())
}

/// Run one raw command on an authenticated session and collect every
/// response line until its tagged DONE. Fails unless the tagged completion
/// is OK. (`run_command` is Session-only API in async-imap 0.11.)
async fn raw_command<S: StreamBound>(
    session: &mut Session<S>,
    cmd: &str,
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<Vec<String>, ImapError> {
    let id = session
        .run_command(cmd)
        .await
        .map_err(|e| session_err(e, &cfg.host, cfg.port))?;
    // Auth is issued inside async-imap and never echoed: only capability and
    // mailbox probes pass through here, so echoing the command is safe.
    t.client(cmd);
    let mut out = Vec::new();
    loop {
        let rd = session
            .read_response()
            .await
            .map_err(|e| io_to_imap(e, &cfg.host, cfg.port))?
            .ok_or_else(|| ImapError::Protocol {
                detail: format!("connection closed during {cmd}"),
            })?;
        let line = format!("{:?}", rd.parsed());
        let (is_done, done_ok) = match rd.parsed() {
            Response::Done { tag, status, .. } if *tag == id => (true, *status == Status::Ok),
            _ => (false, false),
        };
        t.server(&line);
        out.push(line);
        if is_done {
            if done_ok {
                return Ok(out);
            }
            return Err(ImapError::Protocol {
                detail: format!("{cmd} rejected by server"),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Mode constructors: each returns an unauthenticated client with the greeting
// consumed, ready for the shared drive().
// ---------------------------------------------------------------------------

async fn establish_implicit(
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<Client<async_native_tls::TlsStream<TcpStream>>, ImapError> {
    let tcp = tcp_connect(cfg).await?;
    let tls = tls_upgrade(cfg, tcp).await?;
    let mut client = Client::new(tls);
    read_greeting(&mut client, cfg, t).await?;
    Ok(client)
}

async fn establish_plain(
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<Client<TcpStream>, ImapError> {
    check_plain_allowed(cfg)?;
    t.note(
        "WARNING: unencrypted IMAP — localhost only, credentials cross the loopback unencrypted",
    );
    let tcp = tcp_connect(cfg).await?;
    let mut client = Client::new(tcp);
    read_greeting(&mut client, cfg, t).await?;
    Ok(client)
}

async fn write_line<S>(
    rw: &mut BufReader<S>,
    line: &str,
    cfg: &AccountConfig,
) -> Result<(), ImapError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    rw.write_all(format!("{line}\r\n").as_bytes())
        .await
        .map_err(|e| io_to_imap(e, &cfg.host, cfg.port))?;
    rw.flush()
        .await
        .map_err(|e| io_to_imap(e, &cfg.host, cfg.port))
}

async fn read_line<S>(rw: &mut BufReader<S>, cfg: &AccountConfig) -> Result<String, ImapError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut line = String::new();
    let n = rw
        .read_line(&mut line)
        .await
        .map_err(|e| io_to_imap(e, &cfg.host, cfg.port))?;
    if n == 0 {
        return Err(ImapError::Protocol {
            detail: "server closed the connection mid-handshake".to_string(),
        });
    }
    Ok(line.trim_end().to_string())
}

async fn read_until_tag<S>(
    rw: &mut BufReader<S>,
    tag: &str,
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<Vec<String>, ImapError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut out = Vec::new();
    loop {
        let line = read_line(rw, cfg).await?;
        t.server(&line);
        let tagged = line.starts_with(&format!("{tag} "));
        out.push(line);
        if tagged {
            return Ok(out);
        }
    }
}

/// One manual `tag CAPABILITY` exchange over a buffered stream. Used for the
/// unencrypted STARTTLS check and the pre-auth-only probe, where no async-imap
/// Session exists yet.
async fn manual_capability<S>(
    rw: &mut BufReader<S>,
    tag: &str,
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<Vec<String>, ImapError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    t.client(format!("{tag} CAPABILITY"));
    write_line(rw, &format!("{tag} CAPABILITY"), cfg).await?;
    let lines = read_until_tag(rw, tag, cfg, t).await?;
    let last = lines.last().cloned().unwrap_or_default();
    if !last.starts_with(&format!("{tag} OK")) {
        return Err(ImapError::Protocol {
            detail: "server rejected CAPABILITY".to_string(),
        });
    }
    Ok(lines)
}

/// Unencrypted STARTTLS handshake up to and including the TLS upgrade.
/// Returns the TLS stream; the caller wraps it in a client. The pre-upgrade
/// CAPABILITY exchange stays in the transcript as the STARTTLS offer proof.
async fn starttls_upgrade(
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<async_native_tls::TlsStream<TcpStream>, ImapError> {
    let tcp = tcp_connect(cfg).await?;
    let mut rw = BufReader::new(tcp);

    // Mandatory unencrypted greeting.
    let greeting = read_line(&mut rw, cfg).await?;
    t.server(&greeting);

    // Capability probe on the unencrypted channel: STARTTLS must be offered.
    let caps = manual_capability(&mut rw, "a0", cfg, t).await?;
    let caps_text = caps.join("\n").to_uppercase();
    if !caps_text.split_whitespace().any(|tok| tok == "STARTTLS") {
        return Err(ImapError::Protocol {
            detail: "server does not offer STARTTLS on this port — use implicit TLS (993) instead"
                .to_string(),
        });
    }

    // Upgrade, then TLS handshake over the same connection.
    t.client("a1 STARTTLS");
    write_line(&mut rw, "a1 STARTTLS", cfg).await?;
    let resp = read_until_tag(&mut rw, "a1", cfg, t).await?;
    let last = resp.last().cloned().unwrap_or_default();
    if !last.starts_with("a1 OK") {
        return Err(ImapError::Protocol {
            detail: format!("server rejected STARTTLS ({last})"),
        });
    }
    let tcp = rw.into_inner();
    tls_upgrade(cfg, tcp).await
}

async fn establish_starttls(
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<Client<async_native_tls::TlsStream<TcpStream>>, ImapError> {
    let tls = starttls_upgrade(cfg, t).await?;
    // RFC 3501: no new greeting is required after STARTTLS; login's response
    // loop skips any stray untagged greeting the server does send.
    Ok(Client::new(tls))
}

// ---------------------------------------------------------------------------
// Shared authenticated drive: CAPABILITY, NAMESPACE, LIST, STATUS, SELECT.
// ---------------------------------------------------------------------------

async fn drive<S: StreamBound>(
    client: Client<S>,
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<MailboxSummary, ImapError> {
    login_and_select(client, cfg, t).await
}

async fn login_and_select<S: StreamBound>(
    client: Client<S>,
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<MailboxSummary, ImapError> {
    // LOGIN is issued inside async-imap; the password never touches the
    // transcript or any log line on this path.
    let mut session = match client
        .login(cfg.username.clone(), cfg.password.clone())
        .await
    {
        Ok(s) => s,
        Err((e, _)) => return Err(login_err(e, &cfg.username, &cfg.host, cfg.port)),
    };

    // Capability set right after login (authenticated view, includes AUTH=).
    // NOTE: no NAMESPACE command is issued here even though the plan names
    // it: imap-proto 0.16 has no RFC 2342 NAMESPACE response parser, and
    // async-imap permanently closes a session's read side after ANY response
    // parse failure (ImapStream read_closed). Issuing NAMESPACE against a
    // server that supports it would poison this session. The namespace
    // profile is derived from CAPABILITY + LIST instead; if the regression
    // test `namespace_response_is_unparseable` starts failing, the parser
    // learned NAMESPACE and the command can be re-enabled.
    t.note("CAPABILITY (post-login)");
    let caps_lines = raw_command(&mut session, "CAPABILITY", cfg, t).await?;
    let caps_text = caps_lines.join("\n").to_uppercase();
    let advertises_namespace = caps_text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|tok| tok == "NAMESPACE");
    t.note(format!(
        "NAMESPACE (not issued: unparseable by imap-proto 0.16 — server advertises NAMESPACE: {advertises_namespace})"
    ));

    let stream = session
        .list(Some(""), Some("*"))
        .await
        .map_err(|e| session_err(e, &cfg.host, cfg.port))?;
    let names: Vec<async_imap::types::Name> = stream
        .try_collect()
        .await
        .map_err(|e| session_err(e, &cfg.host, cfg.port))?;
    t.client("LIST \"\" *");
    for name in &names {
        t.server(format!(
            "LIST: {:?} delim={:?}",
            name.name(),
            name.delimiter()
        ));
    }
    let delimiters: Vec<String> = {
        let mut ds: Vec<String> = names
            .iter()
            .filter_map(|n| n.delimiter().map(str::to_string))
            .collect();
        ds.sort();
        ds.dedup();
        ds
    };
    t.note(format!(
        "namespace profile (LIST-derived): delimiters={delimiters:?}; personal namespace assumed \"\" (NAMESPACE command unsupported by parser)"
    ));

    // STATUS before SELECT: the recommended order (STATUS on the selected
    // mailbox is legal but discouraged).
    let status = session
        .status("INBOX", "(MESSAGES UIDVALIDITY UIDNEXT)")
        .await
        .map_err(|e| session_err(e, &cfg.host, cfg.port))?;
    t.client("STATUS INBOX (MESSAGES UIDVALIDITY UIDNEXT)");
    t.server(format!(
        "STATUS: exists={} uid_validity={:?}",
        status.exists, status.uid_validity
    ));

    let mailbox = session
        .select("INBOX")
        .await
        .map_err(|e| session_err(e, &cfg.host, cfg.port))?;
    t.client("SELECT INBOX");
    t.server(format!(
        "SELECT: exists={} uid_validity={:?}",
        mailbox.exists, mailbox.uid_validity
    ));
    let uid_validity = mailbox
        .uid_validity
        .ok_or_else(|| ImapError::SelectFailed {
            detail: "server did not return UIDVALIDITY for INBOX".to_string(),
        })?;

    // Best-effort goodbye; probe data is already captured.
    let _ = session.logout().await;

    Ok(MailboxSummary {
        selected_mailbox: "INBOX".to_string(),
        uid_validity,
        exists: mailbox.exists,
    })
}

/// Full probe: establish per mode, then run the authenticated drive.
pub async fn open_inbox(
    cfg: &AccountConfig,
    t: &mut Transcript,
) -> Result<MailboxSummary, ImapError> {
    check_cert_policy(cfg)?;
    match cfg.security {
        SecurityMode::ImplicitTls => {
            let client = establish_implicit(cfg, t).await?;
            drive(client, cfg, t).await
        }
        SecurityMode::StartTls => {
            let client = establish_starttls(cfg, t).await?;
            drive(client, cfg, t).await
        }
        SecurityMode::PlainLocal => {
            let client = establish_plain(cfg, t).await?;
            drive(client, cfg, t).await
        }
    }
}

/// Pre-auth probe: greeting plus CAPABILITY on a throwaway connection, no
/// login. Used for TLS diagnostics and the live server-profile header.
/// (`Client` has no pre-auth command API — `run_command` is Session-only —
/// so this path speaks raw lines and closes the connection after.)
pub async fn pre_auth_caps(cfg: &AccountConfig, t: &mut Transcript) -> Result<(), ImapError> {
    check_cert_policy(cfg)?;
    match cfg.security {
        SecurityMode::ImplicitTls => {
            let tcp = tcp_connect(cfg).await?;
            let tls = tls_upgrade(cfg, tcp).await?;
            let mut rw = BufReader::new(tls);
            let greeting = read_line(&mut rw, cfg).await?;
            t.server(&greeting);
            manual_capability(&mut rw, "a0", cfg, t).await?;
        }
        SecurityMode::StartTls => {
            // The handshake already records greeting + pre-upgrade CAPABILITY;
            // run the post-upgrade CAPABILITY too for the TLS-state profile.
            let tls = starttls_upgrade(cfg, t).await?;
            let mut rw = BufReader::new(tls);
            manual_capability(&mut rw, "a1", cfg, t).await?;
        }
        SecurityMode::PlainLocal => {
            check_plain_allowed(cfg)?;
            let tcp = tcp_connect(cfg).await?;
            let mut rw = BufReader::new(tcp);
            let greeting = read_line(&mut rw, cfg).await?;
            t.server(&greeting);
            manual_capability(&mut rw, "a0", cfg, t).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: SecurityMode, host: &str, confirmed: bool) -> AccountConfig {
        AccountConfig {
            host: host.to_string(),
            port: 143,
            security: mode,
            username: "u".into(),
            password: "p".into(),
            allow_untrusted: false,
            plain_local_confirmed: confirmed,
        }
    }

    #[test]
    fn plain_remote_always_refused() {
        let err = check_plain_allowed(&cfg(SecurityMode::PlainLocal, "mail.example", true));
        assert!(matches!(err, Err(ImapError::Protocol { .. })));
        assert!(err.unwrap_err().to_string().contains("localhost"));
    }

    #[test]
    fn plain_loopback_needs_confirm() {
        assert!(check_plain_allowed(&cfg(SecurityMode::PlainLocal, "127.0.0.1", true)).is_ok());
        assert!(check_plain_allowed(&cfg(SecurityMode::PlainLocal, "localhost", false)).is_err());
    }

    #[test]
    fn cert_override_is_refused_loudly() {
        let mut c = cfg(SecurityMode::ImplicitTls, "h", false);
        assert!(check_cert_policy(&c).is_ok());
        c.allow_untrusted = true;
        let err = check_cert_policy(&c).unwrap_err();
        assert!(
            matches!(err, ImapError::CertExceptionRefused),
            "unexpected: {err}"
        );
    }

    /// Canned-byte stream for parser-limit regression tests.
    #[derive(Debug)]
    struct Replay {
        data: Vec<u8>,
        pos: usize,
    }

    impl futures::io::AsyncRead for Replay {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if self.pos >= self.data.len() {
                return std::task::Poll::Ready(Ok(0));
            }
            let n = std::cmp::min(buf.len(), self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            std::task::Poll::Ready(Ok(n))
        }
    }

    impl futures::io::AsyncWrite for Replay {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// Regression tripwire for the NAMESPACE parser gap.
    ///
    /// imap-proto 0.16 cannot parse RFC 2342 NAMESPACE responses, and
    /// async-imap permanently closes a session's read side after ANY response
    /// parse failure (`ImapStream::read_closed`: every later read returns
    /// `None` instantly while writes keep working — a silent death cascade).
    /// That is why the probe derives the namespace profile from CAPABILITY +
    /// LIST instead of issuing NAMESPACE.
    ///
    /// IF THIS TEST STARTS FAILING, imap-proto learned NAMESPACE responses:
    /// re-enable the NAMESPACE probe step in `login_and_select`.
    #[test]
    fn namespace_response_is_unparseable() {
        use async_imap::Client;
        let bytes =
            b"* OK stub ready.\r\n* NAMESPACE ((\"\" \"/\")) NIL NIL\r\nA0 OK done.\r\n".to_vec();
        async_std::task::block_on(async {
            let mut client = Client::new(Replay {
                data: bytes,
                pos: 0,
            });
            // Greeting parses fine.
            let greeting = client.read_response().await.unwrap();
            assert!(greeting.is_some());
            // The NAMESPACE response does not parse...
            let ns = client.read_response().await;
            assert!(ns.is_err(), "parser learned NAMESPACE: {ns:?}");
            // ...and the session's read side is now permanently closed.
            let after = client.read_response().await.unwrap();
            assert!(after.is_none(), "expected poisoned reads, got: {after:?}");
        });
    }
}
