//! `sync_diag` — diagnose a live INBOX sync against a real IMAP server.
//!
//! Runs the exact sync-path code (`connect_sync` + `select_inbox` +
//! `search_uids` + `fetch_envelopes`) and prints COUNTS plus a small
//! header sample, so we can see where rows vanish when a sync reports
//! "0 messages" on a non-empty mailbox.
//!
//! `--raw-fetch` goes one level deeper: it sends a hand-built FETCH command
//! and prints EVERY server response, including a tagged NO/BAD completion.
//! (async-imap's `uid_fetch` stream ends silently on NO/BAD, which looks
//! exactly like "0 headers" — this mode exposes the server's reason.)
//!
//! Credentials come from the environment only (argv is visible via `ps`).
//! Only the first few subjects/senders are printed (truncated) — enough to
//! prove FETCH works, without dumping the mailbox.
//!
//! Exit codes: 0 ok, 1 sync-path failure, 2 usage error.
//!
//! EXAMPLES:
//!     SGE_IMAP_PASSWORD=secret sync_diag --host mail.utfpr.edu.br --username alice
//!     SGE_IMAP_PASSWORD=secret sync_diag --host mail.utfpr.edu.br --username alice \
//!         --raw-fetch "29291" --attrs "UID"

use async_imap::imap_proto::Response;
use sge_lib::imap::session::connect_sync;
use sge_lib::imap::{AccountConfig, SecurityMode, SyncSession};
use std::process::ExitCode;

const USAGE: &str = r#"sync_diag — run sync-path SELECT + SEARCH + FETCH sample

USAGE:
    sync_diag --host HOST --username USER [--port PORT] [--mode MODE]

OPTIONS:
    --host HOST        IMAP server hostname (required)
    --port PORT        Server port (default: 993 for implicit_tls)
    --mode MODE        implicit_tls (default) | starttls | plain
    --username USER    Login username (required)
    --sample N         How many sample headers to fetch (default 5, max 20)
    --raw-fetch SET    Send a raw `UID FETCH <SET> <ATTRS>` and print every
                       server response (proves NO/BAD vs empty OK)
    --attrs ATTRS      Attrs for --raw-fetch (default "UID FLAGS INTERNALDATE
                       ENVELOPE BODYSTRUCTURE")

ENV:
    SGE_IMAP_PASSWORD  Password (env-only: argv is visible via ps)
"#;

struct Args {
    host: Option<String>,
    port: Option<u16>,
    mode: String,
    username: Option<String>,
    sample: usize,
    raw_fetch: Option<String>,
    attrs: String,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        host: None,
        port: None,
        mode: "implicit_tls".to_string(),
        username: None,
        sample: 5,
        raw_fetch: None,
        attrs: "UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE".to_string(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--host" => a.host = Some(it.next().ok_or("--host needs a value")?),
            "--port" => {
                let v = it.next().ok_or("--port needs a value")?;
                a.port = Some(v.parse::<u16>().map_err(|_| format!("bad --port {v:?}"))?);
            }
            "--mode" => a.mode = it.next().ok_or("--mode needs a value")?,
            "--username" | "-u" => a.username = Some(it.next().ok_or("--username needs a value")?),
            "--sample" => {
                let v = it.next().ok_or("--sample needs a value")?;
                let n: usize = v
                    .parse()
                    .map_err(|_| format!("bad --sample {v:?}"))?;
                a.sample = n.min(20);
            }
            "--raw-fetch" => a.raw_fetch = Some(it.next().ok_or("--raw-fetch needs a value")?),
            "--attrs" => a.attrs = it.next().ok_or("--attrs needs a value")?,
            "--password" | "-p" => {
                eprintln!("refusing: pass the password via SGE_IMAP_PASSWORD (argv is visible via ps)");
                std::process::exit(2);
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
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
    let raw_host = match args.host {
        Some(h) if !h.trim().is_empty() => h,
        _ => {
            eprintln!("usage error: --host is required\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let host = match sge_lib::imap::normalize_host(&raw_host) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("usage error: {e}");
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
    let Some(username) = args.username else {
        eprintln!("usage error: --username is required\n\n{USAGE}");
        return ExitCode::from(2);
    };
    let Some(password) = std::env::var("SGE_IMAP_PASSWORD").ok() else {
        eprintln!("usage error: set SGE_IMAP_PASSWORD in the environment\n\n{USAGE}");
        return ExitCode::from(2);
    };

    let cfg = AccountConfig {
        host,
        port,
        security: mode,
        username,
        password: zeroize::Zeroizing::new(password),
        allow_untrusted: false,
        plain_local_confirmed: false,
    };

    async_std::task::block_on(async {
        let mut session = match connect_sync(&cfg).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("DIAG connect/login failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        let summary = match session.select_inbox().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("DIAG SELECT INBOX failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!(
            "DIAG SELECT: exists={} uid_validity={} uid_next={:?}",
            summary.exists, summary.uid_validity, summary.uid_next
        );

        let uids = match session.search_uids().await {
            Ok(u) => u,
            Err(e) => {
                eprintln!("DIAG UID SEARCH ALL failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!(
            "DIAG SEARCH: {} uids min={:?} max={:?}",
            uids.len(),
            uids.first(),
            uids.last()
        );
        if uids.is_empty() {
            println!("DIAG RESULT: SEARCH empty while SELECT exists={} — server inconsistency", summary.exists);
            let _ = session.logout().await;
            return ExitCode::SUCCESS;
        }

        // Raw mode: hand-built FETCH with full wire transcript. This is
        // the decisive experiment — it shows whether the server answers
        // untagged FETCH lines, an immediate OK (empty), or NO/BAD + reason.
        // Prefix the set with `SEQ:` to send plain `FETCH` (sequence numbers)
        // instead of `UID FETCH` — the fallback if UID variant is rejected.
        if let Some(set) = args.raw_fetch.clone() {
            let cmd = if let Some(seq) = set.strip_prefix("SEQ:") {
                format!("FETCH {seq} {}", args.attrs)
            } else {
                format!("UID FETCH {} {}", set, args.attrs)
            };
            println!("DIAG RAW C: {cmd}");
            let id = match session.run_command(&cmd).await {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("DIAG RAW send failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let mut untagged = 0usize;
            loop {
                let rd = match session.read_response().await {
                    Ok(Some(rd)) => rd,
                    Ok(None) => {
                        eprintln!("DIAG RAW: connection closed mid-command");
                        return ExitCode::FAILURE;
                    }
                    Err(e) => {
                        eprintln!("DIAG RAW read failed: {e}");
                        return ExitCode::FAILURE;
                    }
                };
                match rd.parsed() {
                    Response::Done { tag, status, code, information } => {
                        println!("DIAG RAW S(done): tag={tag:?} status={status:?} code={code:?} info={information:?}");
                        if *tag == id {
                            break;
                        }
                    }
                    other => {
                        untagged += 1;
                        // Truncate: ENVELOPE lines can be long; 600 chars is
                        // enough to identify the message + attribute set.
                        let line = format!("{other:?}");
                        let short: String = line.chars().take(600).collect();
                        println!("DIAG RAW S[{untagged}]: {short}");
                        if untagged >= 8 {
                            println!("DIAG RAW: ... (truncated after 8 untagged responses)");
                            // Drain to the tagged completion without printing.
                            loop {
                                let rd2 = match session.read_response().await {
                                    Ok(Some(r)) => r,
                                    _ => break,
                                };
                                if let Response::Done { tag, status, .. } = rd2.parsed() {
                                    println!("DIAG RAW S(done): tag={tag:?} status={status:?}");
                                    if *tag == id {
                                        break;
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
            println!("DIAG RAW RESULT: {untagged} untagged responses before completion");
            let _ = session.logout().await;
            return ExitCode::SUCCESS;
        }

        // Sample the FIRST few UIDs (oldest) with the exact FETCH attrs the
        // worker uses, to prove ENVELOPE parsing works on this server.
        let take = args.sample.min(uids.len());
        let range: Vec<String> = uids[..take].iter().map(|u| u.to_string()).collect();
        let range_str = range.join(",");
        match session.fetch_envelopes(&range_str).await {
            Ok(headers) => {
                println!("DIAG FETCH [{range_str}]: {} headers", headers.len());
                for h in headers.iter().take(take) {
                    let subj: String = h.subject.chars().take(60).collect();
                    let from: String = h.from_addr.chars().take(60).collect();
                    println!(
                        "DIAG MSG uid={} date={:?} from={:?} subject={:?}",
                        h.uid, h.date_utc, from, subj
                    );
                }
            }
            Err(e) => {
                eprintln!("DIAG UID FETCH failed for range {range_str}: {e}");
                return ExitCode::FAILURE;
            }
        }
        let _ = session.logout().await;
        ExitCode::SUCCESS
    })
}
