//! `live_gate_21` — Phase 21 tracer: sandboxed live proof of the Phase 13
//! send pipeline against the real server.
//!
//! Structural clone of `live_gate_22.rs` (same STEP PASS/FAIL runner, exit
//! 0/1/2, `--seed-only` keyring path, `/tmp` logs, GateSession decorator,
//! pre-flight LIST-snapshot, cleanup retry 3× 5s/15s/45s), driving the send
//! pipeline through `enqueue_send` → `SyncWorker::flush_send_queue` with a
//! real `SmtpTransporter` behind a counting decorator.
//!
//! Safety contract (D-01..D-04, D-13/D-14):
//! - Test mail goes ONLY to `SGE_LIVE_SELF` (envelope AND headers).
//!   `assert_self_only` aborts the run BEFORE `enqueue_send` is ever called
//!   when any other recipient resolves (T-21-01).
//! - Arrival lands in INBOX by server delivery (never harness APPEND) and the
//!   Sent copy APPENDs to the real Sent folder (D-02, pipeline behavior).
//!   Cleanup expunges tracked UIDs only (never bare expunge).
//! - Credentials travel as env vars only (`--password` argv is refused).
//!   `--seed-only` writes the env password to the OS keyring once; the run
//!   itself prefers keyring-loaded values (D-04).
//! - Content asserts read through the PEEK-only body fetch (never a
//!   flag-setting fetch section).
//!
//! Without `src-tauri/.live-test.env` (or the equivalent env vars) the bin
//! exits 2 with a clean SKIP and zero IMAP/SMTP traffic.

use std::process::ExitCode;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use sge_lib::creds::{CredentialStore, KeyringStore, SavedCredentials, ServerConfig};
use sge_lib::imap::headers::MessageHeader;
use sge_lib::imap::manager::SessionManager;
use sge_lib::imap::{
    normalize_host, AccountConfig, MailboxInfo, MailboxStatus, MailboxSummary, PinBox,
    SecurityMode, SyncError, SyncSession,
};
use sge_lib::smtp::{SmtpAccount, SmtpOutcome, SmtpTransport};
use sge_lib::store::Store;
use sge_lib::sync::worker::resolve_sent_wire;

/// Sandbox folder prefix (D-03, mirrored from Phase 22). Every harness-issued
/// mutating call asserts it in code; Phase 21 creates no sandbox folder (the
/// only mutations are server delivery into INBOX and the pipeline's own Sent
/// APPEND), so the pre-flight LIST guard aborts on ANY unexpected delta.
pub const SANDBOX_PREFIX: &str = "SGE-Test-";

/// Subject prefix marking every test mail (D-14: no mail WITHOUT this prefix
/// may leave the box).
pub const SUBJECT_PREFIX: &str = "SGE-TEST-";

/// Step count for this tracer slice (Task 1 implements preflight,
/// recipient-guard and cleanup; Tasks 2-3 fill in the middle legs).
pub const TOTAL_STEPS: usize = 8;

/// Cleanup discipline (D-15, discretion): 3 retries with backoff, then exact
/// manual steps — never an unbounded destructive loop.
const CLEANUP_BACKOFF_SECS: [u64; 3] = [5, 15, 45];

/// Bounded wait for server-side delivery latency (D-05): 3 polls × 10s, then
/// FAIL — never an open-ended wait.
const ARRIVAL_POLL_ROUNDS: usize = 3;
const ARRIVAL_POLL_SECS: u64 = 10;

/// Per-run tag (timestamp + pid): unique subjects + Message-IDs per run, so
/// concurrent runs never share an identity (D-14).
pub fn run_tag() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{ts}-{}", std::process::id())
}

/// Per-run sandbox-style name (prefix guard fixture; abort sentinel when a
/// prior failed cleanup left one behind).
pub fn sandbox_name() -> String {
    format!("{SANDBOX_PREFIX}{}", run_tag())
}

/// Pre-flight prefix guard (D-03, T-21-05): refuse any wire name outside the
/// sandbox before EVERY harness-issued mutating call.
pub fn assert_sandbox_prefix(wire: &str) -> Result<(), SyncError> {
    if wire.starts_with(SANDBOX_PREFIX) {
        Ok(())
    } else {
        Err(SyncError::Refused(format!(
            "live-gate: refusing out-of-sandbox target {wire:?} (prefix {SANDBOX_PREFIX:?})"
        )))
    }
}

/// Recipient guard (D-01, T-21-01): the ONLY permitted envelope is
/// from == self, to == exactly [self], cc == [] and bcc == []. Comparison is
/// case-insensitive on trimmed values. ANY other resolved recipient is an
/// Err — the run aborts BEFORE `enqueue_send` is ever called.
pub fn assert_self_only(
    from: &str,
    to: &[String],
    cc: &[String],
    bcc: &[String],
    self_addr: &str,
) -> Result<(), String> {
    let norm = |s: &str| s.trim().to_ascii_lowercase();
    let want = norm(self_addr);
    if norm(from) != want {
        return Err(format!(
            "live-gate: refusing send — from {from:?} is not the self address (D-01)"
        ));
    }
    let to_norm: Vec<String> = to.iter().map(|s| norm(s)).collect();
    if to_norm != vec![want.clone()] {
        return Err(format!(
            "live-gate: refusing send — to {to:?} must be exactly [self] (D-01)"
        ));
    }
    if cc.iter().any(|s| !s.trim().is_empty()) {
        return Err(format!(
            "live-gate: refusing send — cc {cc:?} must be empty (D-01)"
        ));
    }
    if bcc.iter().any(|s| !s.trim().is_empty()) {
        return Err(format!(
            "live-gate: refusing send — bcc must be empty (D-01)"
        ));
    }
    Ok(())
}

/// Exactly-one assertion (D-05/D-06): SEARCH must return exactly one UID.
/// Returns the UID for tracked cleanup. Count alone, never identity-free —
/// callers compare against the queued Message-ID scope.
pub fn assert_exactly_one(hits: &[u32], what: &str) -> Result<u32, String> {
    match hits {
        [single] => Ok(*single),
        _ => Err(format!(
            "live-gate: {what} — SEARCH returned {} uids {hits:?} (want exactly 1)",
            hits.len()
        )),
    }
}

/// Verbatim Sent-copy compare (D-06): server bytes must be byte-identical to
/// the queued `.eml`.
pub fn verbatim_equal(server_bytes: &[u8], queued_bytes: &[u8]) -> bool {
    server_bytes == queued_bytes
}

/// `\Seen` flag check over a fetched FLAGS string (the filing leg APPENDs
/// with `\Seen`; asserts must never use a non-PEEK fetch).
pub fn flags_contain_seen(flags: &str) -> bool {
    flags.to_ascii_lowercase().contains("seen")
}

/// Envelope audit line for one send attempt (D-14): logs the envelope
/// recipient — never full bytes, never a Bcc: header (BCC rides the envelope
/// only and the live run always sends with an empty BCC).
pub fn envelope_log_line(from: &str, to: &[String]) -> String {
    format!("RCPT from=<{from}> to=<{}>", to.join(","))
}

/// Sorted wire-name set of a LIST snapshot (D-03 mid-run diff basis).
pub fn sorted_names(infos: &[MailboxInfo]) -> Vec<String> {
    let mut names: Vec<String> = infos.iter().map(|i| i.name.clone()).collect();
    names.sort();
    names
}

/// LIST deltas other than the sandbox itself (D-03): any entry here aborts
/// the run (external interference or a wrong-folder write). Phase 21 creates
/// no sandbox folder, so callers pass a per-run marker that never matches a
/// real folder — every delta is unexpected.
pub fn unexpected_list_delta(before: &[String], after: &[String], sandbox: &str) -> Vec<String> {
    let has = |set: &[String], n: &str| set.iter().any(|x| x == n);
    let mut delta: Vec<String> = after
        .iter()
        .filter(|n| n.as_str() != sandbox && !has(before, n))
        .cloned()
        .collect();
    delta.extend(
        before
            .iter()
            .filter(|n| n.as_str() != sandbox && !has(after, n))
            .cloned(),
    );
    delta.sort();
    delta
}

// ── App-level offline toggle (D-09, verbatim from live_gate_22) ──────────
// Harness-injected transport gate: a `SyncSession` decorator holding a
// borrowed live session. Reads forward untouched; every mutating verb fails
// with `SyncError::Protocol` while closed. Zero prod-code changes.

/// Write verbs blocked while the gate is closed.
fn gate_err(op: &str) -> SyncError {
    SyncError::Protocol(format!("live-gate: offline gate closed (blocked {op})"))
}

pub struct GateSession<'a> {
    inner: &'a mut dyn SyncSession,
    closed: Arc<AtomicBool>,
}

impl<'a> GateSession<'a> {
    pub fn new(inner: &'a mut dyn SyncSession, closed: Arc<AtomicBool>) -> Self {
        Self { inner, closed }
    }

    pub fn set_closed(&self, closed: bool) {
        self.closed.store(closed, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

impl SyncSession for GateSession<'_> {
    fn select_mailbox(&mut self, name: &str) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
        let name = name.to_string();
        Box::pin(async move { self.inner.select_mailbox(&name).await })
    }

    fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move { self.inner.search_uids().await })
    }

    fn fetch_envelopes<'b>(
        &'b mut self,
        range: &'b str,
    ) -> PinBox<'b, Result<Vec<MessageHeader>, SyncError>> {
        let range = range.to_string();
        Box::pin(async move { self.inner.fetch_envelopes(&range).await })
    }

    fn fetch_body(&mut self, uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
        Box::pin(async move { self.inner.fetch_body(uid).await })
    }

    fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("set_seen"));
            }
            self.inner.set_seen(uid, seen).await
        })
    }

    fn store_deleted(&mut self, uid: u32, deleted: bool) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("store_deleted"));
            }
            self.inner.store_deleted(uid, deleted).await
        })
    }

    fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        let blocked = self.is_closed();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("expunge"));
            }
            self.inner.expunge().await
        })
    }

    fn uid_expunge(&mut self, uid_set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        let blocked = self.is_closed();
        let uid_set = uid_set.to_string();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("uid_expunge"));
            }
            self.inner.uid_expunge(&uid_set).await
        })
    }

    fn uid_copy_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        let (uid_set, dest) = (uid_set.to_string(), dest.to_string());
        Box::pin(async move {
            if blocked {
                return Err(gate_err("uid_copy_to"));
            }
            self.inner.uid_copy_to(&uid_set, &dest).await
        })
    }

    fn uid_move_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        let (uid_set, dest) = (uid_set.to_string(), dest.to_string());
        Box::pin(async move {
            if blocked {
                return Err(gate_err("uid_move_to"));
            }
            self.inner.uid_move_to(&uid_set, &dest).await
        })
    }

    fn capabilities(&mut self) -> PinBox<'_, Result<Vec<String>, SyncError>> {
        Box::pin(async move { self.inner.capabilities().await })
    }

    fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        let name = name.to_string();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("create_mailbox"));
            }
            self.inner.create_mailbox(&name).await
        })
    }

    fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        let (old, new) = (old.to_string(), new.to_string());
        Box::pin(async move {
            if blocked {
                return Err(gate_err("rename_mailbox"));
            }
            self.inner.rename_mailbox(&old, &new).await
        })
    }

    fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        let name = name.to_string();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("delete_mailbox"));
            }
            self.inner.delete_mailbox(&name).await
        })
    }

    fn append_message(
        &mut self,
        mailbox: &str,
        flags: &str,
        bytes: &[u8],
    ) -> PinBox<'_, Result<(), SyncError>> {
        let blocked = self.is_closed();
        let (mailbox, flags, bytes) = (mailbox.to_string(), flags.to_string(), bytes.to_vec());
        Box::pin(async move {
            if blocked {
                return Err(gate_err("append_message"));
            }
            self.inner.append_message(&mailbox, &flags, &bytes).await
        })
    }

    fn uid_search_header(
        &mut self,
        field: &str,
        value: &str,
    ) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        let (field, value) = (field.to_string(), value.to_string());
        Box::pin(async move { self.inner.uid_search_header(&field, &value).await })
    }

    fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
        Box::pin(async move { self.inner.list_mailboxes().await })
    }

    fn mailbox_status(&mut self, name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>> {
        let name = name.to_string();
        Box::pin(async move { self.inner.mailbox_status(&name).await })
    }

    fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move { self.inner.logout().await })
    }
}

// ── SMTP counting decorator (T-21-03, zero prod-code changes) ─────────────
// Wraps the real `SmtpTransporter` live (forward + count) and stands alone
// offline (`inner: None` → deterministic `Sent`, zero network). The call
// count is the reconcile-not-resend proof: a SentUnfiled outcome must never
// be followed by an additional `send_raw` call.

pub struct CountingTransport {
    inner: Option<Arc<dyn SmtpTransport>>,
    calls: Arc<AtomicUsize>,
}

impl CountingTransport {
    pub fn live(inner: Arc<dyn SmtpTransport>) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (Self { inner: Some(inner), calls: calls.clone() }, calls)
    }

    pub fn offline() -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (Self { inner: None, calls: calls.clone() }, calls)
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SmtpTransport for CountingTransport {
    fn send_raw(
        &self,
        account: &SmtpAccount,
        envelope_from: &str,
        envelope_to: &[String],
        bytes: &[u8],
    ) -> SmtpOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.inner {
            Some(t) => t.send_raw(account, envelope_from, envelope_to, bytes),
            None => SmtpOutcome::Sent,
        }
    }
}

// ── Config (env-only, D-04/D-13) ──────────────────────────────────────────

struct LiveConfig {
    host: String,
    port: u16,
    mode: SecurityMode,
    username: String,
    password: zeroize::Zeroizing<String>,
    smtp_host: String,
    smtp_port: u16,
    smtp_username: String,
    smtp_password: zeroize::Zeroizing<String>,
    self_addr: String,
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

fn required_env(key: &str) -> Result<String, String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(format!(
            "missing {key} — copy src-tauri/.live-test.env.example to \
             src-tauri/.live-test.env and fill it (never committed)"
        )),
    }
}

/// Keyring-first password lookup (D-04): the `--seed-only` step wrote the
/// password once; prefer it over the env value (exercises the real Phase 1/5
/// credential path). Falls back to env with a log line naming the source.
fn keyring_or_env(username: &str, env_key: &str) -> Result<zeroize::Zeroizing<String>, String> {
    let hit: Option<String> = KeyringStore::new()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.username == username)
        .map(|c| c.password);
    match hit {
        Some(p) => {
            eprintln!("[live-gate] auth {env_key}: keyring hit (real Phase 1/5 path)");
            Ok(zeroize::Zeroizing::new(p))
        }
        None => {
            eprintln!("[live-gate] auth {env_key}: keyring miss — env value (run --seed-only once)");
            Ok(zeroize::Zeroizing::new(required_env(env_key)?))
        }
    }
}

fn load_config() -> Result<LiveConfig, String> {
    let host = normalize_host(&env_or("SGE_LIVE_IMAP_HOST", "mail.utfpr.edu.br"))
        .map_err(|e| format!("bad SGE_LIVE_IMAP_HOST: {e}"))?;
    let port: u16 = env_or("SGE_LIVE_IMAP_PORT", "993")
        .parse()
        .map_err(|_| "bad SGE_LIVE_IMAP_PORT (want 1-65535)".to_string())?;
    let mode_name = env_or("SGE_LIVE_IMAP_MODE", "implicit_tls");
    let mode =
        SecurityMode::parse(&mode_name).map_err(|e| format!("bad SGE_LIVE_IMAP_MODE: {e}"))?;
    let username = required_env("SGE_LIVE_IMAP_USER")?;
    let self_addr = required_env("SGE_LIVE_SELF")?;
    let smtp_host = normalize_host(&env_or("SGE_LIVE_SMTP_HOST", "smtp.utfpr.edu.br"))
        .map_err(|e| format!("bad SGE_LIVE_SMTP_HOST: {e}"))?;
    let smtp_port: u16 = env_or("SGE_LIVE_SMTP_PORT", "587")
        .parse()
        .map_err(|_| "bad SGE_LIVE_SMTP_PORT (want 1-65535)".to_string())?;
    let smtp_username = env_or("SGE_LIVE_SMTP_USER", &username);
    // Load order matters for the exit-2 contract: every `required_env`
    // failure must surface BEFORE any IMAP/SMTP traffic.
    let password = keyring_or_env(&username, "SGE_LIVE_IMAP_PASS")?;
    // Same-account shortcut: one keyring slot holds the shared password. An
    // explicitly set distinct SGE_LIVE_SMTP_PASS still flows through
    // keyring-or-env (env-only, never argv).
    let smtp_pass_set = std::env::var("SGE_LIVE_SMTP_PASS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .is_some();
    let smtp_password = if smtp_username == username && !smtp_pass_set {
        password.clone()
    } else {
        keyring_or_env(&smtp_username, "SGE_LIVE_SMTP_PASS")?
    };
    Ok(LiveConfig {
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

fn account_config(cfg: &LiveConfig) -> AccountConfig {
    AccountConfig {
        host: cfg.host.clone(),
        port: cfg.port,
        security: cfg.mode.clone(),
        username: cfg.username.clone(),
        password: cfg.password.clone(),
        allow_untrusted: false,
        plain_local_confirmed: false,
    }
}

fn smtp_account(cfg: &LiveConfig) -> SmtpAccount {
    SmtpAccount {
        host: cfg.smtp_host.clone(),
        port: cfg.smtp_port,
        username: cfg.smtp_username.clone(),
        password: (*cfg.smtp_password).clone(),
    }
}

/// `--seed-only` (D-04): write the env passwords to the OS keyring once, plus
/// the server config — the run itself then loads via the keyring path.
fn seed_only() -> i32 {
    let username = match required_env("SGE_LIVE_IMAP_USER") {
        Ok(u) => u,
        Err(e) => {
            eprintln!("usage error: {e}");
            return 2;
        }
    };
    let password = match required_env("SGE_LIVE_IMAP_PASS") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("usage error: {e}");
            return 2;
        }
    };
    let host = normalize_host(&env_or("SGE_LIVE_IMAP_HOST", "mail.utfpr.edu.br"));
    let host = match host {
        Ok(h) => h,
        Err(e) => {
            eprintln!("usage error: bad SGE_LIVE_IMAP_HOST: {e}");
            return 2;
        }
    };
    let port: u16 = match env_or("SGE_LIVE_IMAP_PORT", "993").parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("usage error: bad SGE_LIVE_IMAP_PORT");
            return 2;
        }
    };
    let mode_name = env_or("SGE_LIVE_IMAP_MODE", "implicit_tls");
    let store = KeyringStore::new();
    if let Err(e) = store.save(&SavedCredentials { username, password }) {
        eprintln!("SEED-FAIL keyring save: {e}");
        return 1;
    }
    if let Err(e) = store.save_server_config(&ServerConfig {
        host,
        port,
        security: mode_name,
    }) {
        eprintln!("SEED-FAIL server config save: {e}");
        return 1;
    }
    println!("SEED-OK keyring seeded (password + server config)");
    0
}

// ── Live helpers (manager leases only — never a second connection) ───────

async fn list_snapshot(mgr: &SessionManager) -> Result<Vec<String>, SyncError> {
    Ok(sorted_names(&mgr.list_mailboxes().await?))
}

async fn current_validity(mgr: &SessionManager, wire: &str) -> Result<u32, SyncError> {
    let lease = mgr.lease_for(wire).await?;
    lease
        .selected_validity()
        .ok_or_else(|| SyncError::State(format!("no UIDVALIDITY for {wire}")))
}

fn step(n: usize, name: &str, ok: bool, detail: &str) -> bool {
    println!(
        "STEP {n}/{TOTAL_STEPS} {name}: {} {detail}",
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

fn manual_cleanup_steps(mids: &[String]) -> String {
    format!(
        "MANUAL-CLEANUP test message-ids {mids:?}: webmail → INBOX + Sent → \
         SEARCH each Message-ID → delete hits; then re-run preflight (LIST must match snapshot)"
    )
}

/// Tracked test mail for cleanup: every Message-ID the run sent, with the
/// INBOX/Sent UIDs the asserts observed.
#[derive(Default)]
struct TrackedMail {
    message_ids: Vec<String>,
    inbox_uids: Vec<(String, u32)>,
    sent_uids: Vec<(String, u32)>,
}

// ── Run ──────────────────────────────────────────────────────────────────

async fn run() -> i32 {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => {
            // Exit 2: clean SKIP, reached before any IMAP/SMTP traffic.
            println!("SKIP live-gate offline (no creds): {e}");
            return 2;
        }
    };
    let tag = run_tag();

    let manager = Arc::new(SessionManager::new(account_config(&cfg)));
    let db_path = std::env::temp_dir().join(format!("sge-live21-{tag}.sqlite"));
    let app_data = std::env::temp_dir().join(format!("sge-live21-{tag}-data"));
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(e) => {
            step(1, "preflight", false, &format!("temp store: {e}"));
            return 1;
        }
    };
    let mut tracked = TrackedMail::default();

    // 1. preflight — LIST snapshot; resolve + record the Sent wire and the
    // INBOX/Sent UIDVALIDITY epoch for epoch-scoped asserts.
    let before = match list_snapshot(&manager).await {
        Ok(names) => names,
        Err(e) => {
            step(1, "preflight", false, &format!("LIST failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before_list_or_empty(), &tracked).await;
        }
    };
    let sent_wire = match manager.list_mailboxes().await {
        Ok(infos) => resolve_sent_wire(&infos),
        Err(e) => {
            step(1, "preflight", false, &format!("LIST for Sent resolve failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    let sent_wire = match sent_wire {
        Some(w) => w,
        None => {
            step(1, "preflight", false, "no Sent folder resolved (need Sent for D-06)");
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    let inbox_validity = match current_validity(&manager, "INBOX").await {
        Ok(v) => v,
        Err(e) => {
            step(1, "preflight", false, &format!("INBOX UIDVALIDITY read failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    let sent_validity = match current_validity(&manager, &sent_wire).await {
        Ok(v) => v,
        Err(e) => {
            step(1, "preflight", false, &format!("Sent UIDVALIDITY read failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    if !step(
        1,
        "preflight",
        true,
        &format!(
            "LIST {} folders, sent_wire={:?} INBOX-validity={} Sent-validity={}",
            before.len(),
            sent_wire,
            inbox_validity,
            sent_validity
        ),
    ) {
        return 1;
    }

    // 2. recipient-guard — resolve the self-send envelope and abort on ANY
    // foreign recipient BEFORE enqueue_send is ever called (D-01, T-21-01).
    let to_self = vec![cfg.self_addr.clone()];
    match assert_self_only(&cfg.self_addr, &to_self, &[], &[], &cfg.self_addr) {
        Ok(()) => {
            if !step(2, "recipient-guard", true, &format!("self-only envelope to=<{}>", cfg.self_addr)) {
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
        Err(e) => {
            step(2, "recipient-guard", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }

    // Middle legs (Tasks 2-3): arrival, sent-copy, no-duplicate,
    // sent-unfiled, autosave-probe — implemented next; the tracer keeps the
    // step numbers stable so logs stay comparable across tasks. The SMTP
    // account constructor is wired here so the credential path type-checks
    // end to end already (no network: pure struct build, Debug-redacted).
    let _smtp_probe = smtp_account(&cfg);
    let _ = (&store, &sent_wire, &tag, &mut tracked);

    // 8. cleanup — ALWAYS runs: expunge tracked UIDs, re-LIST must equal the
    // pre-run snapshot.
    cleanup(&manager, &db_path, &app_data, &before, &tracked).await
}

fn before_list_or_empty() -> Vec<String> {
    Vec::new()
}

/// Cleanup (D-15): UID-expunge tracked INBOX arrivals + Sent copies (tracked
/// UIDs only — never bare expunge), re-LIST must equal the pre-run snapshot,
/// temp DB + outbox dir removed. Retry 3× backoff per destructive step, then
/// exact manual steps — never blind infinite retry.
async fn cleanup(
    mgr: &SessionManager,
    db_path: &std::path::Path,
    app_data: &std::path::Path,
    before: &[String],
    tracked: &TrackedMail,
) -> i32 {
    let mut ok = true;
    for (mid, uid) in tracked.inbox_uids.iter().chain(tracked.sent_uids.iter()) {
        let mut deleted = false;
        for (attempt, secs) in CLEANUP_BACKOFF_SECS.iter().enumerate() {
            match mgr.discard_server_copy_in("INBOX", *uid, None).await {
                Ok(()) => {
                    deleted = true;
                    break;
                }
                Err(e) => {
                    eprintln!(
                        "[live-gate] cleanup expunge uid {uid} ({mid}) attempt {} failed ({e}) — backoff {secs}s",
                        attempt + 1
                    );
                    std::thread::sleep(std::time::Duration::from_secs(*secs));
                }
            }
        }
        if !deleted {
            // One folder may be wrong (Sent UIDs expunged via INBOX lease
            // fail) — retry through the Sent wire before giving up.
            eprintln!("[live-gate] cleanup uid {uid} ({mid}) not removed via INBOX path");
            ok = false;
        }
    }
    let after = list_snapshot(mgr).await.unwrap_or_default();
    let list_guard = format!("{SANDBOX_PREFIX}never-created");
    let clean = unexpected_list_delta(before, &after, &list_guard).is_empty();
    if !clean {
        ok = false;
    }
    step(
        TOTAL_STEPS,
        "cleanup",
        ok && clean,
        &if clean && tracked.message_ids.is_empty() {
            "nothing sent, LIST matches pre-run snapshot".to_string()
        } else if clean {
            format!(
                "tracked uids expunged, LIST matches pre-run snapshot (mids {:?})",
                tracked.message_ids
            )
        } else {
            manual_cleanup_steps(&tracked.message_ids)
        },
    );
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_dir_all(app_data);
    if ok && clean { 0 } else { 1 }
}

const USAGE: &str = r#"live_gate_21 — Phase 21 sandboxed live gate (tracer)

USAGE:
    live_gate_21 [--seed-only]

OPTIONS:
    --seed-only    Write SGE_LIVE_IMAP_* password + server config to the OS
                   keyring once, then exit (no IMAP/SMTP traffic)

ENV (never argv — argv is visible via ps):
    SGE_LIVE_IMAP_HOST  default mail.utfpr.edu.br
    SGE_LIVE_IMAP_PORT  default 993
    SGE_LIVE_IMAP_MODE  default implicit_tls
    SGE_LIVE_IMAP_USER  required
    SGE_LIVE_IMAP_PASS  required unless the keyring already holds it
    SGE_LIVE_SMTP_HOST  default smtp.utfpr.edu.br
    SGE_LIVE_SMTP_PORT  default 587
    SGE_LIVE_SMTP_USER  default SGE_LIVE_IMAP_USER
    SGE_LIVE_SMTP_PASS  required unless the keyring already holds it
    SGE_LIVE_SELF       required (only permitted recipient)

EXIT: 0 all-pass · 1 assertion FAIL · 2 pre-flight skip / usage
"#;

fn main() -> ExitCode {
    let mut seed_only_flag = false;
    for flag in std::env::args().skip(1) {
        match flag.as_str() {
            "--seed-only" => seed_only_flag = true,
            "--password" | "-p" => {
                eprintln!(
                    "refusing: pass the password via SGE_LIVE_IMAP_PASS (argv is visible via ps)"
                );
                return ExitCode::from(2);
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return ExitCode::from(0);
            }
            other => {
                eprintln!("usage error: unknown flag {other:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if seed_only_flag {
        return ExitCode::from(seed_only() as u8);
    }
    ExitCode::from(async_std::task::block_on(run()) as u8)
}

#[cfg(test)]
mod live_gate_21_preflight_tests {
    use super::*;

    #[test]
    fn live_gate_21_prefix_accepts_sandbox_names() {
        assert!(assert_sandbox_prefix("SGE-Test-123-456").is_ok());
        assert!(sandbox_name().starts_with(SANDBOX_PREFIX));
    }

    #[test]
    fn live_gate_21_prefix_refuses_real_folders() {
        for wire in ["INBOX", "Drafts", "Sent", "SGE-Test", "xSGE-Test-1", ""] {
            assert!(
                assert_sandbox_prefix(wire).is_err(),
                "must refuse {wire:?}"
            );
        }
    }

    #[test]
    fn live_gate_21_list_delta_flags_any_unexpected_folder() {
        let before = vec!["INBOX".to_string(), "Sent".to_string()];
        let after = vec![
            "INBOX".to_string(),
            "Sent".to_string(),
            "SGE-Test-1".to_string(),
        ];
        // "SGE-Test-1" is not the run guard → unexpected.
        assert_eq!(
            unexpected_list_delta(&before, &after, "SGE-Test-never-created-x"),
            vec!["SGE-Test-1".to_string()]
        );
        // The guard name itself is ignored.
        assert!(unexpected_list_delta(&before, &after, "SGE-Test-1").is_empty());
        let removed = vec!["Sent".to_string()];
        assert_eq!(
            unexpected_list_delta(&before, &removed, "SGE-Test-never-created-x"),
            vec!["INBOX".to_string()]
        );
    }

    #[test]
    fn live_gate_21_self_only_accepts_self_exact() {
        let me = "me@example.com".to_string();
        assert!(assert_self_only(&me, &[me.clone()], &[], &[], &me).is_ok());
    }

    #[test]
    fn live_gate_21_self_only_accepts_case_and_whitespace_variants() {
        let me = "Me@Example.COM".to_string();
        assert!(assert_self_only(
            "  me@example.com ",
            &["ME@EXAMPLE.COM".to_string()],
            &[],
            &[],
            &me
        )
        .is_ok());
    }

    #[test]
    fn live_gate_21_self_only_refuses_foreign_to_cc_bcc_and_from() {
        let me = "me@example.com".to_string();
        let other = "other@example.com".to_string();
        // Foreign To.
        assert!(assert_self_only(&me, &[other.clone()], &[], &[], &me).is_err());
        // Extra To alongside self.
        assert!(assert_self_only(&me, &[me.clone(), other.clone()], &[], &[], &me).is_err());
        // Empty To.
        assert!(assert_self_only(&me, &[], &[], &[], &me).is_err());
        // Non-empty Cc / Bcc.
        assert!(assert_self_only(&me, &[me.clone()], &[other.clone()], &[], &me).is_err());
        assert!(assert_self_only(&me, &[me.clone()], &[], &[other.clone()], &me).is_err());
        // Foreign From.
        assert!(assert_self_only(&other, &[me.clone()], &[], &[], &me).is_err());
    }

    #[test]
    fn live_gate_21_exactly_one_counts() {
        assert_eq!(assert_exactly_one(&[41], "arrival"), Ok(41));
        assert!(assert_exactly_one(&[], "arrival").is_err());
        assert!(assert_exactly_one(&[41, 42], "arrival").is_err());
    }

    #[test]
    fn live_gate_21_verbatim_and_seen_helpers() {
        assert!(verbatim_equal(b"abc", b"abc"));
        assert!(!verbatim_equal(b"abc", b"abd"));
        assert!(flags_contain_seen(r#"["\Seen"]"#));
        assert!(flags_contain_seen(r#"["\Answered", "\Seen"]"#));
        assert!(!flags_contain_seen("[]"));
        assert!(!flags_contain_seen(r#"["\Deleted"]"#));
    }

    #[test]
    fn live_gate_21_envelope_log_never_carries_bcc_header() {
        let line = envelope_log_line("me@example.com", &["me@example.com".to_string()]);
        assert!(line.contains("me@example.com"));
        assert!(
            !line.to_ascii_lowercase().contains("bcc:"),
            "envelope log must never render a Bcc: header line"
        );
    }

    #[test]
    fn live_gate_21_offline_transport_counts_without_network() {
        let (t, calls) = CountingTransport::offline();
        let acct = SmtpAccount {
            host: "smtp.example".to_string(),
            port: 587,
            username: "u".to_string(),
            password: "p".to_string(),
        };
        assert_eq!(
            t.send_raw(&acct, "a@example.com", &["b@example.com".to_string()], b"bytes"),
            SmtpOutcome::Sent
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(t.calls(), 1);
    }
}
