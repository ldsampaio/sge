//! `live_gate_22` — Phase 22 tracer: sandboxed live proof of the Phase 12
//! one-copy draft round-trip against the real server.
//!
//! The bin owns ONE [`SessionManager`] plus one temp-file [`Store`] for the
//! whole run — never a second IMAP connection alongside the manager. Every
//! step prints `STEP n/N <name>: PASS|FAIL <detail>`; the process exits
//! 0 all-pass / 1 assertion FAIL / 2 pre-flight-skip-or-env-missing.
//!
//! Safety contract (D-01..D-04, D-13/D-14):
//! - Mutating ops target ONLY the per-run sandbox folder `SGE-Test-<ts>-<pid>`
//!   at account root. Every mutating call site asserts the sandbox prefix in
//!   code before touching the session.
//! - Credentials travel as env vars only (`--password` argv is refused, same
//!   as `sync_diag`). `--seed-only` writes the env password to the OS keyring
//!   once; the run itself prefers keyring-loaded values (D-14).
//! - Content asserts read through the PEEK-only body fetch (never a
//!   flag-setting fetch section) so asserts never disturb Seen state.
//!
//! Without `src-tauri/.live-test.env` (or the equivalent env vars) the bin
//! exits 2 with a clean SKIP and zero IMAP traffic.

use std::process::ExitCode;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use sge_lib::creds::{CredentialStore, KeyringStore, SavedCredentials, ServerConfig};
use sge_lib::drafts::{new_message_id, render_draft_rfc5322, DraftFields};
use sge_lib::imap::headers::MessageHeader;
use sge_lib::imap::manager::SessionManager;
use sge_lib::imap::{
    normalize_host, AccountConfig, MailboxInfo, MailboxStatus, MailboxSummary, PinBox,
    SecurityMode, SyncError, SyncSession,
};
use sge_lib::store::{queries, Store};

/// Sandbox folder prefix (D-01). Every mutating call asserts it in code.
pub const SANDBOX_PREFIX: &str = "SGE-Test-";

/// Subject prefix marking every test draft (D-16: only-self test mail).
pub const SUBJECT_PREFIX: &str = "SGE-TEST-";

/// Step count for this tracer slice (grows as later tasks add steps).
pub const TOTAL_STEPS: usize = 4;

/// Cleanup discipline (D-03, discretion): 3 retries with backoff, then exact
/// manual steps — never an unbounded destructive loop.
const CLEANUP_BACKOFF_SECS: [u64; 3] = [5, 15, 45];

/// Per-run sandbox folder at account root (D-01): unique timestamp + pid, so
/// concurrent runs never share a folder and a present name always means a
/// prior failed cleanup (never reused).
pub fn sandbox_name() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{SANDBOX_PREFIX}{ts}-{}", std::process::id())
}

/// Pre-flight prefix guard (D-02, T-22-01): refuse any wire name outside the
/// sandbox before EVERY mutating call.
pub fn assert_sandbox_prefix(wire: &str) -> Result<(), SyncError> {
    if wire.starts_with(SANDBOX_PREFIX) {
        Ok(())
    } else {
        Err(SyncError::Refused(format!(
            "live-gate: refusing out-of-sandbox target {wire:?} (prefix {SANDBOX_PREFIX:?})"
        )))
    }
}

/// Stable Message-ID for one live compose session (D-16 unique per run).
pub fn run_message_id(run_tag: &str) -> String {
    new_message_id(&format!("live22-{run_tag}"))
}

/// Render one test draft via the prod codec (never string-concatenated MIME).
/// Subject always carries the `SGE-TEST-` prefix (D-16 audit hook).
pub fn render_test_draft(
    from_addr: &str,
    to_self: &str,
    subject_suffix: &str,
    body: &str,
    message_id: &str,
    date_rfc2822: &str,
) -> Vec<u8> {
    let fields = DraftFields {
        from: from_addr.to_string(),
        to: vec![to_self.to_string()],
        cc: vec![],
        bcc: vec![],
        subject: format!("{SUBJECT_PREFIX}{subject_suffix}"),
        body: body.to_string(),
        message_id: message_id.to_string(),
    };
    render_draft_rfc5322(&fields, date_rfc2822)
}

/// Max-wins reconcile (D-06): several staged copies collapse to the newest.
/// Single source for the live path and the offline doubles.
pub fn max_wins(mut hits: Vec<u32>) -> Option<u32> {
    hits.sort_unstable();
    hits.into_iter().max()
}

/// One-copy assertion (D-05): SEARCH must return exactly one UID AND it must
/// equal the tracked `server_uid` (count + identity, never count alone).
pub fn assert_one_copy(hits: &[u32], tracked: u32) -> Result<u32, String> {
    match hits {
        [single] if *single == tracked => Ok(tracked),
        [single] => Err(format!(
            "one-copy identity mismatch: server holds uid {single}, tracked {tracked}"
        )),
        _ => Err(format!(
            "one-copy count violation: server holds {} copies {hits:?}, tracked {tracked}",
            hits.len()
        )),
    }
}

/// Sorted wire-name set of a LIST snapshot (D-02 mid-run diff basis).
pub fn sorted_names(infos: &[MailboxInfo]) -> Vec<String> {
    let mut names: Vec<String> = infos.iter().map(|i| i.name.clone()).collect();
    names.sort();
    names
}

/// LIST deltas other than the sandbox itself (D-02): any entry here aborts
/// the run (external interference or a wrong-folder write).
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

// ── App-level offline toggle (D-09) ──────────────────────────────────────
// Harness-injected transport gate: a `SyncSession` decorator holding a
// borrowed live session. Reads forward untouched; every mutating verb fails
// with `SyncError::Protocol` while closed. Zero prod-code changes — the
// queued-then-replay flow goes through the real `replay_*` entries on
// reopen. `AtomicBool` behind `Arc` so the run can flip the gate while a
// lease-held session is wrapped.

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

// ── Config (env-only, D-13) ──────────────────────────────────────────────

struct LiveConfig {
    host: String,
    port: u16,
    mode: SecurityMode,
    username: String,
    password: zeroize::Zeroizing<String>,
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
    // D-14 keyring-first: the seed step wrote the password once; prefer it.
    let keyring_hit: Option<String> = KeyringStore::new()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.username == username)
        .map(|c| c.password);
    let password = match keyring_hit {
        Some(p) => {
            eprintln!("[live-gate] auth: keyring hit (real Phase 1/5 path)");
            zeroize::Zeroizing::new(p)
        }
        None => {
            eprintln!("[live-gate] auth: keyring miss — env password (run --seed-only once)");
            zeroize::Zeroizing::new(required_env("SGE_LIVE_IMAP_PASS")?)
        }
    };
    Ok(LiveConfig {
        host,
        port,
        mode,
        username,
        password,
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

/// `--seed-only` (D-14): write the env password to the OS keyring once, plus
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

async fn search_uids_in(mgr: &SessionManager, wire: &str) -> Result<Vec<u32>, SyncError> {
    let mut lease = mgr.lease_for(wire).await?;
    lease.session().search_uids().await
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

fn manual_cleanup_steps(sandbox: &str) -> String {
    format!(
        "MANUAL-CLEANUP folder={sandbox}: webmail (or any client) → folder list → \
         {sandbox} → delete; then re-run preflight (LIST must not contain it)"
    )
}

// ── Run ──────────────────────────────────────────────────────────────────

async fn run() -> i32 {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => {
            // Exit 2: clean SKIP, reached before any IMAP traffic.
            println!("SKIP live-gate offline (no creds): {e}");
            return 2;
        }
    };
    let sandbox = sandbox_name();
    if let Err(e) = assert_sandbox_prefix(&sandbox) {
        step(1, "preflight", false, &e.to_string());
        return 1;
    }

    let manager = SessionManager::new(account_config(&cfg));
    let db_path = std::env::temp_dir().join(format!(
        "sge-live22-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(e) => {
            step(1, "preflight", false, &format!("temp store: {e}"));
            return 1;
        }
    };
    let _ = self_addr_touch(&cfg);

    // 1. preflight — LIST snapshot; the sandbox must be absent (present =
    // prior failed cleanup → abort with manual steps, never reuse).
    let before = match list_snapshot(&manager).await {
        Ok(names) => names,
        Err(e) => {
            step(1, "preflight", false, &format!("LIST failed: {e}"));
            return 1;
        }
    };
    if before.iter().any(|n| n == &sandbox) {
        step(
            1,
            "preflight",
            false,
            &format!("sandbox present (prior failed cleanup) — {}", manual_cleanup_steps(&sandbox)),
        );
        return 1;
    }
    if !step(1, "preflight", true, &format!("LIST {} folders, sandbox absent", before.len())) {
        return 1;
    }

    // 2. create — CREATE via create_mailbox_in (its implicit INBOX lease is
    // SELECT-only: the single accepted exception, D-04); re-LIST must show
    // exactly one delta: the sandbox.
    if let Err(e) = assert_sandbox_prefix(&sandbox) {
        step(2, "create", false, &e.to_string());
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    if let Err(e) = manager.create_mailbox_in(&sandbox).await {
        step(2, "create", false, &format!("CREATE failed: {e}"));
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    let after_create = match list_snapshot(&manager).await {
        Ok(names) => names,
        Err(e) => {
            step(2, "create", false, &format!("re-LIST failed: {e}"));
            return cleanup(&manager, &db_path, &sandbox, &before).await;
        }
    };
    let delta = unexpected_list_delta(&before, &after_create, &sandbox);
    let created_ok =
        after_create.iter().any(|n| n == &sandbox) && delta.is_empty();
    if !step(
        2,
        "create",
        created_ok,
        &format!("sandbox created, unexpected deltas: {delta:?}"),
    ) {
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    // Seed the temp store's mailbox row + epoch (what sync Step 1 does) so
    // later replay entries find their rows.
    let epoch = match current_validity(&manager, &sandbox).await {
        Ok(v) => v,
        Err(e) => {
            step(2, "create", false, &format!("validity read failed: {e}"));
            return cleanup(&manager, &db_path, &sandbox, &before).await;
        }
    };
    {
        let guard = store.lock().unwrap();
        if let Err(e) = (|| -> Result<(), String> {
            queries::ensure_mailbox(guard.conn(), &sandbox).map_err(|e| e.to_string())?;
            queries::set_sync_state(guard.conn(), &sandbox, epoch, 0).map_err(|e| e.to_string())?;
            Ok(())
        })() {
            step(2, "create", false, &format!("store seed failed: {e}"));
            return cleanup(&manager, &db_path, &sandbox, &before).await;
        }
    }

    // 3. guard-empty — fresh sandbox SEARCHes empty before first APPEND.
    let empty = match search_uids_in(&manager, &sandbox).await {
        Ok(uids) => uids,
        Err(e) => {
            step(3, "guard-empty", false, &format!("SEARCH failed: {e}"));
            return cleanup(&manager, &db_path, &sandbox, &before).await;
        }
    };
    if !step(
        3,
        "guard-empty",
        empty.is_empty(),
        &format!("sandbox holds {} uids (want 0)", empty.len()),
    ) {
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }

    // 4. cleanup — ALWAYS runs: expunge leftovers, DELETE the sandbox with
    // backoff (D-03), re-LIST must equal the pre-run snapshot.
    cleanup(&manager, &db_path, &sandbox, &before).await
}

/// Touch helper so the self-address field is not dead config (D-16 audit
/// hook lives here from Task 2; the tracer keeps the address plumbed).
fn self_addr_touch(cfg: &LiveConfig) -> Result<(), String> {
    if cfg.self_addr.trim().is_empty() {
        return Err("SGE_LIVE_SELF must not be empty".to_string());
    }
    Ok(())
}

/// Cleanup (D-03): expunge-all-in-sandbox reusing `discard_server_copy_in`,
/// then DELETE with retry; post-LIST must equal the pre-run snapshot.
async fn cleanup(
    mgr: &SessionManager,
    db_path: &std::path::Path,
    sandbox: &str,
    before: &[String],
) -> i32 {
    let mut ok = true;
    let leftovers = search_uids_in(mgr, sandbox).await.unwrap_or_default();
    for uid in leftovers {
        if assert_sandbox_prefix(sandbox).is_err() {
            ok = false;
            break;
        }
        if let Err(e) = mgr.discard_server_copy_in(sandbox, uid, None).await {
            eprintln!("[live-gate] cleanup expunge uid {uid} failed ({e}) — continuing");
        }
    }
    let mut deleted = false;
    for (attempt, secs) in CLEANUP_BACKOFF_SECS.iter().enumerate() {
        if assert_sandbox_prefix(sandbox).is_err() {
            ok = false;
            break;
        }
        match mgr.delete_mailbox_in(sandbox).await {
            Ok(()) => {
                deleted = true;
                break;
            }
            Err(e) => {
                eprintln!(
                    "[live-gate] cleanup DELETE attempt {} failed ({e}) — backoff {secs}s",
                    attempt + 1
                );
                std::thread::sleep(std::time::Duration::from_secs(*secs));
            }
        }
    }
    if !deleted {
        ok = false;
    }
    let after = list_snapshot(mgr).await.unwrap_or_default();
    let clean = deleted
        && !after.iter().any(|n| n == sandbox)
        && unexpected_list_delta(before, &after, sandbox).is_empty();
    if !clean {
        ok = false;
    }
    step(
        TOTAL_STEPS,
        "cleanup",
        ok && clean,
        &if clean {
            "sandbox deleted, LIST matches pre-run snapshot".to_string()
        } else {
            manual_cleanup_steps(sandbox)
        },
    );
    let _ = std::fs::remove_file(db_path);
    if ok && clean { 0 } else { 1 }
}

const USAGE: &str = r#"live_gate_22 — Phase 22 sandboxed live gate (tracer)

USAGE:
    live_gate_22 [--seed-only]

OPTIONS:
    --seed-only    Write SGE_LIVE_IMAP_* password + server config to the OS
                   keyring once, then exit (no IMAP traffic)

ENV (never argv — argv is visible via ps):
    SGE_LIVE_IMAP_HOST  default mail.utfpr.edu.br
    SGE_LIVE_IMAP_PORT  default 993
    SGE_LIVE_IMAP_MODE  default implicit_tls
    SGE_LIVE_IMAP_USER  required
    SGE_LIVE_IMAP_PASS  required unless the keyring already holds it
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
mod live_gate_22_preflight_tests {
    use super::*;

    #[test]
    fn live_gate_22_prefix_accepts_sandbox_names() {
        assert!(assert_sandbox_prefix("SGE-Test-123-456").is_ok());
        assert!(sandbox_name().starts_with(SANDBOX_PREFIX));
    }

    #[test]
    fn live_gate_22_prefix_refuses_real_folders() {
        for wire in ["INBOX", "Drafts", "Sent", "SGE-Test", "xSGE-Test-1", ""] {
            assert!(
                assert_sandbox_prefix(wire).is_err(),
                "must refuse {wire:?}"
            );
        }
    }

    #[test]
    fn live_gate_22_list_delta_ignores_only_the_sandbox() {
        let before = vec!["INBOX".to_string(), "Drafts".to_string()];
        let after = vec![
            "INBOX".to_string(),
            "Drafts".to_string(),
            "SGE-Test-1".to_string(),
        ];
        assert!(unexpected_list_delta(&before, &after, "SGE-Test-1").is_empty());
        let tampered = vec![
            "INBOX".to_string(),
            "Drafts".to_string(),
            "SGE-Test-1".to_string(),
            "Evil".to_string(),
        ];
        assert_eq!(
            unexpected_list_delta(&before, &tampered, "SGE-Test-1"),
            vec!["Evil".to_string()]
        );
        let removed = vec!["Drafts".to_string(), "SGE-Test-1".to_string()];
        assert_eq!(
            unexpected_list_delta(&before, &removed, "SGE-Test-1"),
            vec!["INBOX".to_string()]
        );
    }

    #[test]
    fn live_gate_22_one_copy_counts_and_identity() {
        assert_eq!(assert_one_copy(&[41], 41), Ok(41));
        assert!(assert_one_copy(&[], 41).is_err());
        assert!(assert_one_copy(&[41, 42], 42).is_err());
        assert!(assert_one_copy(&[7], 9).is_err());
    }

    #[test]
    fn live_gate_22_max_wins_picks_newest() {
        assert_eq!(max_wins(vec![3, 1, 2]), Some(3));
        assert_eq!(max_wins(vec![]), None);
    }

    #[test]
    fn live_gate_22_rendered_draft_carries_prefix_and_parses() {
        let msg_id = run_message_id("unittest");
        let bytes = render_test_draft(
            "me@example.com",
            "me@example.com",
            "round-trip",
            "hello body",
            &msg_id,
            "Tue, 13 Oct 2026 12:00:00 +0000",
        );
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(&format!("Subject: {SUBJECT_PREFIX}round-trip")));
        assert!(text.contains(&format!("Message-ID: {msg_id}")));
        let parsed = mail_parser::MessageParser::new()
            .parse(&bytes)
            .expect("rendered bytes must parse");
        assert_eq!(parsed.subject(), Some(format!("{SUBJECT_PREFIX}round-trip").as_str()));
        assert_eq!(parsed.body_text(0).unwrap().to_string(), "hello body");
        assert_eq!(parsed.message_id(), Some(msg_id.trim_matches(&['<', '>'][..])));
    }
}
