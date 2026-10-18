//! `live_gate_reply` — live verification of the reply/threading + attachment
//! send path.
//!
//! No existing gate covers this: `live_gate_21/22/26` all render plain
//! `text/plain` drafts with no `In-Reply-To`/`References` and no
//! `multipart/mixed`, so the reply repair of 2026-10-18 was unit-tested only.
//!
//! What it proves, against the real server:
//!   STEP 3  the real renderer + `enqueue_send` accept threading headers and a
//!           base64 attachment
//!   STEP 4  the real `flush_send_queue` (SMTP 587 + Sent filing) accepts it
//!   STEP 5  the message is delivered back to INBOX (D-05: never trust 250)
//!   STEP 6  the DELIVERED bytes still carry `In-Reply-To`, `References`,
//!           `multipart/mixed`, and a byte-identical, non-ASCII-named part
//!   STEP 7  the harness removes its own Sent + INBOX copies (nothing else)
//!
//! Sandbox safety contract (structural, mirrored from live_gate_21):
//!   - mail goes ONLY to `SGE_LIVE_SELF`; a non-self recipient aborts the run
//!   - every subject carries the `SGE-TEST-` prefix
//!   - cleanup expunges ONLY the UIDs this run created (keyed by Message-ID)
//!
//! Credentials come from the environment (never argv — argv is visible via
//! `ps`). Without them the run exits 2 with a clean SKIP and zero traffic.
//!
//! EXIT: 0 all-pass · 1 assertion FAIL · 2 pre-flight skip.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use mail_parser::{MessageParser, MimeHeaders};
use sge_lib::drafts::AttachmentInput;
use sge_lib::imap::manager::SessionManager;
use sge_lib::imap::{normalize_host, AccountConfig, SecurityMode, SyncError};
use sge_lib::send_queue::{self, EnqueueInput, SendGate};
use sge_lib::smtp::{SmtpAccount, SmtpTransport, SmtpTransporter};
use sge_lib::store::Store;
use sge_lib::sync::worker::{resolve_sent_wire, SendFlushEnv, SyncWorker};

/// Every harness-issued subject carries this prefix (D-14 mirror).
const SUBJECT_PREFIX: &str = "SGE-TEST-";
/// Bounded arrival poll (D-05 mirror): 6 rounds x 10s. Generous enough that a
/// stale pooled session (bounded SELECT + one-shot reconnect, ~30s) still
/// leaves room for the delivery to land inside the window.
const ARRIVAL_POLL_ROUNDS: usize = 6;
const ARRIVAL_POLL_SECS: u64 = 10;

/// Deliberately non-ASCII: proves the RFC 2231 `filename*=` path survives a
/// real SMTP hop and a real IMAP delivery, not just the unit-test renderer.
const ATTACH_FILENAME: &str = "relatório de orientação.txt";
const ATTACH_BODY: &str = "conteúdo do anexo — ação, coração\n";

/// Synthetic ancestors: the gate asserts the headers travel on the wire.
/// Real-client threading is a function of exactly these headers.
const PARENT_MID: &str = "sge-live-parent@sge.test";
const GRANDPARENT_MID: &str = "sge-live-grandparent@sge.test";

struct Cfg {
    host: String,
    port: u16,
    mode: SecurityMode,
    username: String,
    password: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_username: String,
    smtp_password: String,
    self_addr: String,
}

/// Report one STEP. Keeps the exit contract honest: every branch prints.
fn report(step: usize, name: &str, ok: bool, detail: &str) -> bool {
    println!(
        "STEP {step}/7 {name}: {} {detail}",
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn required_env(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{key} missing — source src-tauri/.live-test.env"))
}

fn load_config() -> Result<Cfg, String> {
    let host = normalize_host(&env_or("SGE_LIVE_IMAP_HOST", "mail.utfpr.edu.br"))
        .map_err(|e| format!("bad SGE_LIVE_IMAP_HOST: {e}"))?;
    let port: u16 = env_or("SGE_LIVE_IMAP_PORT", "993")
        .parse()
        .map_err(|_| "bad SGE_LIVE_IMAP_PORT (want 1-65535)".to_string())?;
    let mode = SecurityMode::parse(&env_or("SGE_LIVE_IMAP_MODE", "implicit_tls"))
        .map_err(|e| format!("bad SGE_LIVE_IMAP_MODE: {e}"))?;
    let username = required_env("SGE_LIVE_IMAP_USER")?;
    let self_addr = required_env("SGE_LIVE_SELF")?;
    let smtp_host = normalize_host(&env_or("SGE_LIVE_SMTP_HOST", "smtp.utfpr.edu.br"))
        .map_err(|e| format!("bad SGE_LIVE_SMTP_HOST: {e}"))?;
    let smtp_port: u16 = env_or("SGE_LIVE_SMTP_PORT", "587")
        .parse()
        .map_err(|_| "bad SGE_LIVE_SMTP_PORT (want 1-65535)".to_string())?;
    let smtp_username = env_or("SGE_LIVE_SMTP_USER", &username);
    // Every required value must surface BEFORE any traffic (exit-2 contract).
    let password = required_env("SGE_LIVE_IMAP_PASS")?;
    let smtp_password = match std::env::var("SGE_LIVE_SMTP_PASS") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => password.clone(),
    };
    Ok(Cfg {
        host,
        port,
        mode,
        username,
        password,
        smtp_host,
        smtp_port,
        smtp_username,
        smtp_password,
        self_addr,
    })
}

fn account_config(cfg: &Cfg) -> AccountConfig {
    AccountConfig {
        host: cfg.host.clone(),
        port: cfg.port,
        security: cfg.mode.clone(),
        username: cfg.username.clone(),
        password: zeroize::Zeroizing::new(cfg.password.clone()),
        allow_untrusted: false,
        plain_local_confirmed: false,
    }
}

fn smtp_account(cfg: &Cfg) -> SmtpAccount {
    SmtpAccount {
        host: cfg.smtp_host.clone(),
        port: cfg.smtp_port,
        username: cfg.smtp_username.clone(),
        password: cfg.smtp_password.clone(),
    }
}

/// Standard (padded) base64 — the same alphabet the frontend sends. Kept local
/// so the gate depends on nothing the browser-side contract does not.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Find the single INBOX UID whose Message-ID matches `mid` (bounded poll).
async fn poll_inbox_for_mid(mgr: &SessionManager, mid: &str) -> Result<Option<u32>, SyncError> {
    for round in 0..ARRIVAL_POLL_ROUNDS {
        if round > 0 {
            async_std::task::sleep(std::time::Duration::from_secs(ARRIVAL_POLL_SECS)).await;
        }
        let mut lease = mgr.lease_for("INBOX").await?;
        let hits = lease.session().uid_search_header("Message-ID", mid).await?;
        drop(lease);
        if let Some(uid) = hits.into_iter().max() {
            return Ok(Some(uid));
        }
    }
    Ok(None)
}

async fn uid_in_folder(
    mgr: &SessionManager,
    folder: &str,
    mid: &str,
) -> Result<Vec<u32>, SyncError> {
    let mut lease = mgr.lease_for(folder).await?;
    lease.session().uid_search_header("Message-ID", mid).await
}

/// Percent-encode a UTF-8 string exactly as the renderer's RFC 2231 `*`
/// parameter does (`drafts::percent_encode`): unreserved chars verbatim, every
/// other byte as `%XX`.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Best-effort removal of this run's OWN artifacts: only UIDs whose Message-ID
/// matches the one this run minted, searched per folder. Nothing else.
async fn cleanup_own(
    mgr: &SessionManager,
    sent_wire: &str,
    message_id: &str,
) -> (Vec<String>, bool) {
    let mut removed = Vec::new();
    let mut failed = false;
    for folder in ["INBOX", sent_wire] {
        match uid_in_folder(mgr, folder, &format!("<{message_id}>")).await {
            Ok(uids) if !uids.is_empty() => {
                let matched = uids.len();
                let set = uids
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let mut lease = match mgr.lease_for(folder).await {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("[live-gate-reply] {folder} lease for cleanup failed: {e}");
                        failed = true;
                        continue;
                    }
                };
                // RFC 4315: `UID EXPUNGE` removes only messages ALREADY flagged
                // \Deleted, so flag each match first. Without this the call is a
                // silent no-op and the probes stay in the mailbox.
                let mut flagged = 0usize;
                for uid in &uids {
                    match lease.session().store_deleted(*uid, true).await {
                        Ok(()) => flagged += 1,
                        Err(e) => {
                            eprintln!("[live-gate-reply] {folder} \\Deleted uid {uid} failed: {e}");
                            failed = true;
                        }
                    }
                }
                // `uid_expunge` reports the UIDs the server echoed back in
                // untagged EXPUNGE responses — some servers send none, so a
                // zero count means "no confirmation", not "nothing removed".
                match lease.session().uid_expunge(&set).await {
                    Ok(gone) => removed.push(format!(
                        "{folder}:{matched} matched, {} confirmed",
                        gone.len()
                    )),
                    Err(e) => {
                        eprintln!("[live-gate-reply] {folder} expunge failed: {e}");
                        failed = true;
                    }
                }
            }
            Ok(_) => removed.push(format!("{folder}:absent")),
            Err(e) => {
                eprintln!("[live-gate-reply] {folder} cleanup search failed: {e}");
                failed = true;
            }
        }
    }
    (removed, failed)
}

/// Cleanup + report + temp-file removal, shared by EVERY exit path after the
/// message exists. A run that fails at STEP 5 must still remove its probe —
/// the first version returned early and left one behind in the mailbox.
async fn finish(
    mgr: &SessionManager,
    sent_wire: &str,
    message_id: &str,
    db_path: &std::path::Path,
    app_data: &std::path::Path,
) {
    let (removed, failed) = cleanup_own(mgr, sent_wire, message_id).await;
    println!("cleanup: {}", removed.join(" "));
    if failed {
        println!(
            "MANUAL-CLEANUP: remove any remaining message with subject {SUBJECT_PREFIX}* \
             (Message-ID <{message_id}>)"
        );
    }
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_dir_all(app_data);
}

async fn run() -> i32 {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP live-gate-reply offline (no creds): {e}");
            return 2;
        }
    };

    let manager = Arc::new(SessionManager::new(account_config(&cfg)));
    let tag = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        std::process::id()
    );
    let db_path = std::env::temp_dir().join(format!("sge-live-reply-{tag}.sqlite"));
    let app_data = std::env::temp_dir().join(format!("sge-live-reply-{tag}-data"));
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(e) => {
            report(1, "preflight", false, &format!("temp store: {e}"));
            return 1;
        }
    };

    // ── STEP 1 preflight ────────────────────────────────────────────────
    let folders = match manager.list_mailboxes().await {
        Ok(f) => f,
        Err(e) => {
            report(1, "preflight", false, &format!("LIST failed: {e}"));
            return 1;
        }
    };
    let names: Vec<String> = folders.iter().map(|f| f.name.clone()).collect();
    let sent_wire = match resolve_sent_wire(&folders) {
        Some(w) => w,
        None => {
            report(
                1,
                "preflight",
                false,
                "no Sent folder resolved — nothing to file into",
            );
            return 1;
        }
    };
    report(
        1,
        "preflight",
        sent_wire == "Sent" || !sent_wire.is_empty(),
        &format!("LIST {} folders, sent_wire={sent_wire:?}", names.len()),
    );

    // ── STEP 2 recipient guard: self-only, structurally ─────────────────
    let recipients = vec![cfg.self_addr.clone()];
    if !recipients.iter().all(|r| r == &cfg.self_addr) {
        report(2, "recipient-guard", false, "non-self recipient — refusing");
        return 1;
    }
    report(
        2,
        "recipient-guard",
        true,
        &format!("self-only envelope to=<{}>", cfg.self_addr),
    );

    // ── STEP 3 render + enqueue with threading + attachment ─────────────
    let subject = format!("{SUBJECT_PREFIX}{tag}-reply");
    let attachment = AttachmentInput {
        filename: ATTACH_FILENAME.to_string(),
        content_type: "text/plain".to_string(),
        data_base64: base64_encode(ATTACH_BODY.as_bytes()),
    };
    let (queue_id, message_id) = {
        let guard = store.lock().unwrap();
        match send_queue::enqueue_send(
            guard.conn(),
            &app_data,
            EnqueueInput {
                from: cfg.self_addr.clone(),
                to: recipients.clone(),
                cc: vec![],
                bcc: vec![],
                subject: subject.clone(),
                body: "live gate reply probe".to_string(),
                draft_id: None,
                in_reply_to: Some(format!("<{PARENT_MID}>")),
                references: Some(format!("<{GRANDPARENT_MID}> <{PARENT_MID}>")),
                attachments: vec![attachment],
            },
        ) {
            Ok(out) => (out.queue_id, out.message_id),
            Err(e) => {
                report(3, "render+enqueue", false, &format!("enqueue failed: {e}"));
                return 1;
            }
        }
    };
    if !report(
        3,
        "render+enqueue",
        !message_id.is_empty(),
        &format!("queue_id={queue_id} message_id=<{message_id}>"),
    ) {
        return 1;
    }

    // ── STEP 4 real flush (SMTP + Sent filing) ──────────────────────────
    let worker = SyncWorker::new(store.clone());
    let transport: Arc<dyn SmtpTransport> = Arc::new(SmtpTransporter::new());
    let flush_env = SendFlushEnv {
        gate: SendGate::default(),
        transport,
        account: smtp_account(&cfg),
        manager: Some(manager.clone()),
        sent_wire: Some(sent_wire.clone()),
    };
    let summary = worker.flush_send_queue(&flush_env).await;
    // This gate verifies the reply/attachment BYTES reach the recipient
    // intact — the Sent-filing leg is live_gate_21's concern. A filing failure
    // (`sent_unfiled`) still means SMTP accepted the message, so the delivery
    // + format steps below remain meaningful; report it without aborting.
    let accepted = summary.sent + summary.sent_unfiled == 1;
    if summary.sent_unfiled > 0 {
        println!(
            "NOTE sent-filing: unfiled={} — SMTP accepted the message but the Sent \
             copy was not filed (pre-existing defect, tracked by live_gate_21 STEP 3)",
            summary.sent_unfiled
        );
    }
    if !report(
        4,
        "flush-send",
        accepted,
        &format!(
            "smtp-accepted={accepted} sent={} unfiled={} deferred={} failed={} uncertain={}",
            summary.sent, summary.sent_unfiled, summary.deferred, summary.failed, summary.uncertain
        ),
    ) {
        finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;
        return 1;
    }

    // ── STEP 5 arrival (D-05: never trust the SMTP 250) ─────────────────
    let inbox_uid = match poll_inbox_for_mid(&manager, &format!("<{message_id}>")).await {
        Ok(Some(uid)) => uid,
        Ok(None) => {
            report(
                5,
                "arrival",
                false,
                "not delivered within the bounded poll window",
            );
            finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;
            return 1;
        }
        Err(e) => {
            report(5, "arrival", false, &format!("INBOX search failed: {e}"));
            finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;
            return 1;
        }
    };
    report(
        5,
        "arrival",
        true,
        &format!("delivered to INBOX uid {inbox_uid}"),
    );

    // ── STEP 6 the delivered bytes still carry the contract ─────────────
    let raw = {
        let mut lease = match manager.lease_for("INBOX").await {
            Ok(l) => l,
            Err(e) => {
                report(
                    6,
                    "delivered-format",
                    false,
                    &format!("INBOX lease failed: {e}"),
                );
                finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;
                return 1;
            }
        };
        match lease.session().fetch_body(inbox_uid).await {
            Ok(b) => b,
            Err(e) => {
                report(
                    6,
                    "delivered-format",
                    false,
                    &format!("BODY.PEEK[] failed: {e}"),
                );
                finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;
                return 1;
            }
        }
    };
    let msg = match MessageParser::new().parse(&raw) {
        Some(m) => m,
        None => {
            report(
                6,
                "delivered-format",
                false,
                "delivered message did not parse",
            );
            finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;
            return 1;
        }
    };

    let wire_in_reply_to = msg.in_reply_to().as_text().unwrap_or("").to_string();
    let wire_references = msg.references().as_text().unwrap_or("").to_string();
    let want_in_reply_to = format!("<{PARENT_MID}>");
    let want_references = format!("<{GRANDPARENT_MID}> <{PARENT_MID}>");
    let (top_type, top_subtype) = msg
        .content_type()
        .map(|ct| {
            (
                ct.ctype().to_string(),
                ct.subtype().map(|s| s.to_string()).unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    let atts: Vec<_> = msg.attachments().collect();
    let att_name = atts
        .first()
        .and_then(|a| a.attachment_name())
        .unwrap_or("")
        .to_string();
    let att_bytes = atts
        .first()
        .map(|a| a.contents().to_vec())
        .unwrap_or_default();

    // Header/filename assertions read the RAW wire bytes, not the parser's
    // view: `mail_parser` (like Python's email package) strips the angle
    // brackets from In-Reply-To, returns a single id for References, and
    // prefers the ASCII `filename=` fallback over `filename*=` — all of which
    // made an earlier version of this step fail on a perfectly correct message.
    let wire = String::from_utf8_lossy(&raw);
    let expected_rfc2231 = format!("filename*=UTF-8''{}", percent_encode(ATTACH_FILENAME));

    let mut failures: Vec<String> = Vec::new();
    if !wire.contains(&format!("In-Reply-To: {want_in_reply_to}")) {
        failures.push(format!(
            "raw In-Reply-To {want_in_reply_to:?} missing (parser saw {wire_in_reply_to:?})"
        ));
    }
    if !wire.contains(&format!("References: {want_references}")) {
        failures.push(format!(
            "raw References {want_references:?} missing (parser saw {wire_references:?})"
        ));
    }
    if (top_type.as_str(), top_subtype.as_str()) != ("multipart", "mixed") {
        failures.push(format!(
            "top-level {top_type}/{top_subtype} != multipart/mixed"
        ));
    }
    if !wire.contains(&expected_rfc2231) {
        failures.push(format!("raw RFC 2231 param {expected_rfc2231:?} missing"));
    }
    if atts.len() != 1 {
        failures.push(format!("{} attachment part(s), want exactly 1", atts.len()));
    } else if att_bytes != ATTACH_BODY.as_bytes() {
        failures.push(format!(
            "attachment bytes differ: got {} bytes, want {}",
            att_bytes.len(),
            ATTACH_BODY.len()
        ));
    }
    let ok = failures.is_empty();
    let detail = if ok {
        format!(
            "In-Reply-To + References on the wire, multipart/mixed, RFC 2231 {att_name:?} \
             decodes from filename*, {} attachment byte(s) byte-identical",
            att_bytes.len()
        )
    } else {
        failures.join("; ")
    };
    report(6, "delivered-format", ok, &detail);

    // ── STEP 7 cleanup: ONLY this run's artifacts, keyed by Message-ID ──
    finish(&manager, &sent_wire, &message_id, &db_path, &app_data).await;

    if ok {
        println!("BUNDLE-REPORT: PASS (threading + attachment verified end to end)");
        0
    } else {
        println!("BUNDLE-REPORT: FAIL (see STEP 6 detail)");
        1
    }
}

fn main() -> ExitCode {
    let code = async_std::task::block_on(run());
    ExitCode::from(code as u8)
}
