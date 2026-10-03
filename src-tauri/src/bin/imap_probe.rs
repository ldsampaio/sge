//! `imap_probe` — CLI harness for the IMAP connection core.
//!
//! Runs the exact same [`sge_lib::imap::probe`] code as the `connect_account`
//! Tauri command: connect, CAPABILITY, NAMESPACE, LIST, STATUS, SELECT INBOX,
//! then prints the mailbox summary plus the transcript.
//!
//! Credentials come from flags or environment and are NEVER written to disk:
//! the transcript echoes only capability/mailbox probes (auth is issued
//! inside async-imap and never echoed), and the runner redacts any residual
//! username/password occurrence before printing.
//!
//! Exit codes: 0 ok, 1 connection/command failure, 2 usage error.

use sge_lib::imap::{probe, AccountConfig, SecurityMode};
use std::process::ExitCode;

const USAGE: &str = "imap_probe — probe an IMAP INBOX (same core as the SGE login)

USAGE:
    imap_probe --host HOST --username USER [OPTIONS]

OPTIONS:
    --host HOST            IMAP server hostname (required)
    --port PORT            Server port (default: 993 implicit_tls, 143 starttls)
    --mode MODE            implicit_tls (default) | starttls | plain
    --username USER        Login username (required for full probe)
    --password PASS        Login password (prefer SGE_IMAP_PASSWORD env)
    --allow-untrusted      Request a cert exception (logged, surfaced, REFUSED —
                           verification is never bypassed silently)
    --allow-plain-local    Confirm localhost-only unencrypted mode for --mode plain
    --pre-auth-only        Connect + greeting + CAPABILITY only, no login
    --help                 Print this help and exit 0

ENV:
    SGE_IMAP_PASSWORD      Password fallback when --password is absent

EXAMPLES:
    SGE_IMAP_PASSWORD=secret imap_probe --host mail.utfpr.edu.br --username alice
    imap_probe --host mail.utfpr.edu.br --mode starttls --port 143 --username alice --pre-auth-only
";

struct Args {
    host: Option<String>,
    port: Option<u16>,
    mode: String,
    username: Option<String>,
    password: Option<String>,
    allow_untrusted: bool,
    allow_plain_local: bool,
    pre_auth_only: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        host: None,
        port: None,
        mode: "implicit_tls".to_string(),
        username: None,
        password: std::env::var("SGE_IMAP_PASSWORD").ok(),
        allow_untrusted: false,
        allow_plain_local: false,
        pre_auth_only: false,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--host" => a.host = Some(it.next().ok_or("--host needs a value")?),
            "--port" => {
                let v = it.next().ok_or("--port needs a value")?;
                a.port = Some(v.parse::<u16>().map_err(|_| format!("bad --port {v:?}"))?);
            }
            "--mode" => a.mode = it.next().ok_or("--mode needs a value")?,
            "--username" | "-u" => a.username = Some(it.next().ok_or("--username needs a value")?),
            "--password" | "-p" => a.password = Some(it.next().ok_or("--password needs a value")?),
            "--allow-untrusted" => a.allow_untrusted = true,
            "--allow-plain-local" => a.allow_plain_local = true,
            "--pre-auth-only" => a.pre_auth_only = true,
            other => return Err(format!("unknown flag {other:?}\n\n{USAGE}")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("usage error: {e}");
            return ExitCode::from(2);
        }
    };
    let host = match args.host {
        Some(h) if !h.trim().is_empty() => h,
        _ => {
            eprintln!("usage error: --host is required\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mode = match SecurityMode::parse(&args.mode) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("usage error: {e}");
            return ExitCode::from(2);
        }
    };
    let port = args.port.unwrap_or(match mode {
        SecurityMode::ImplicitTls => 993,
        SecurityMode::StartTls | SecurityMode::PlainLocal => 143,
    });
    if port == 0 {
        eprintln!("usage error: --port must be in range 1-65535");
        return ExitCode::from(2);
    }
    if mode == SecurityMode::PlainLocal && !args.allow_plain_local {
        eprintln!(
            "WARNING: --mode plain sends credentials unencrypted and works for localhost only.\n\
             Re-run with --allow-plain-local to confirm, or use implicit_tls."
        );
        return ExitCode::from(2);
    }
    if args.pre_auth_only {
        let cfg = AccountConfig {
            host,
            port,
            security: mode,
            username: args.username.unwrap_or_default(),
            password: String::new(),
            allow_untrusted: args.allow_untrusted,
            plain_local_confirmed: args.allow_plain_local,
        };
        return async_std::task::block_on(async {
            match probe::run_pre_auth_probe(&cfg).await {
                Ok(transcript) => {
                    print!("{transcript}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("pre-auth probe failed: {e}");
                    ExitCode::FAILURE
                }
            }
        });
    }
    let (Some(username), Some(password)) = (args.username, args.password) else {
        eprintln!(
            "usage error: full probe needs --username and --password (or SGE_IMAP_PASSWORD)\n\n{USAGE}"
        );
        return ExitCode::from(2);
    };
    let cfg = AccountConfig {
        host,
        port,
        security: mode,
        username,
        password,
        allow_untrusted: args.allow_untrusted,
        plain_local_confirmed: args.allow_plain_local,
    };
    async_std::task::block_on(async {
        match probe::run_probe(&cfg).await {
            Ok(outcome) => {
                print!("{}", outcome.transcript);
                println!(
                    "SUMMARY selected_mailbox={} exists={} uid_validity={}",
                    outcome.summary.selected_mailbox,
                    outcome.summary.exists,
                    outcome.summary.uid_validity
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("probe failed: {e}");
                ExitCode::FAILURE
            }
        }
    })
}
