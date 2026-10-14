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

use std::collections::HashSet;
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
    SecurityMode, SyncError, SyncSession, DRAFT_FLAGS,
};
use sge_lib::send_queue::rfc2822_now;
use sge_lib::store::{queries, Store};
use sge_lib::sync::worker::SyncWorker;
use sge_lib::sync::SyncCallback;

/// Sandbox folder prefix (D-01). Every mutating call asserts it in code.
pub const SANDBOX_PREFIX: &str = "SGE-Test-";

/// Subject prefix marking every test draft (D-16: only-self test mail).
pub const SUBJECT_PREFIX: &str = "SGE-TEST-";

/// Step count for the full live run (22-01 tracer 1-3 + 22-02 expansion).
pub const TOTAL_STEPS: usize = 13;

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
pub fn unexpected_list_delta(before: &[String], after: &[String], sandbox: &str) -> Vec<String> {    let has = |set: &[String], n: &str| set.iter().any(|x| x == n);
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

// ── Expansion helpers (22-02: replay / resurrection / Seen / safety) ──────
// Pure predicates shared by the live legs below and the offline doubles in
// `tests/live_gate_22_safety.rs` (included from this file — one
// implementation, two callers).

/// Structural pending indicator (D-10): queued Seen toggles plus queued
/// delete/move ops for one mailbox. Mirrors the `sync_status` pending sum
/// the existing Phase 10/12 UI badge reads, so pending-until-acked is
/// asserted on the number with no new UI surface.
pub fn pending_depth(conn: &rusqlite::Connection, mailbox_id: u64) -> i64 {
    let seen = queries::outbox_count(conn, mailbox_id).unwrap_or(0);
    let imap = queries::imap_outbox_count(conn, mailbox_id).unwrap_or(0);
    seen.saturating_add(imap)
}

/// Transcript-order predicate (D-10): an `imap replay` line must precede the
/// first sweep line. The worker emits `[SGE sync] imap replay: ...` at Step
/// 4b before the sweep code runs, so file order in the captured log is the
/// evidence; this is the checkable shape of that rule.
pub fn replay_logged_before_sweep(lines: &[&str]) -> bool {
    let mut replay_at: Option<usize> = None;
    let mut sweep_at: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if replay_at.is_none() && line.contains("imap replay") {
            replay_at = Some(i);
        }
        if sweep_at.is_none() && line.contains("sweep") {
            sweep_at = Some(i);
        }
    }
    match (replay_at, sweep_at) {
        (Some(r), Some(s)) => r < s,
        _ => false,
    }
}

/// Fixed-substring secret scan predicate (D-15, T-22-04): the index of the
/// first secret found in `text`, or `None` when clean. Callers map the index
/// to an artifact name — never the secret itself. Empty secrets never match
/// (an empty password would flag every artifact).
pub fn scan_text_for_secrets(text: &str, secrets: &[&str]) -> Option<usize> {
    secrets
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .find(|(_, s)| text.contains(*s))
        .map(|(i, _)| i)
}

/// Only-test-mail audit predicate (D-16, T-22-04): every subject carries the
/// `SGE-TEST-` prefix and every recipient equals the run's self address.
pub fn audit_test_mail(
    subjects: &[&str],
    recipients: &[&str],
    self_addr: &str,
) -> Result<(), String> {
    for subject in subjects {
        if !subject.contains(SUBJECT_PREFIX) {
            return Err(format!("unprefixed subject fails D-16 audit: {subject:?}"));
        }
    }
    for rcpt in recipients {
        if *rcpt != self_addr {
            return Err("non-self recipient fails D-16 audit (only mail to self)".to_string());
        }
    }
    Ok(())
}

/// Cleanup retry schedule (D-03, discretion): 3 attempts, 5s/15s/45s.
pub fn cleanup_backoff_secs() -> [u64; 3] {
    CLEANUP_BACKOFF_SECS
}

/// SQLite dump writer (D-15): renders every user table of the run's temp
/// Store as text so the wrapper can grep the dump for credential
/// substrings. Read-only open — the live Store keeps its own connection.
/// FTS5 virtual tables are skipped (`SELECT *` is unreliable on them and
/// they mirror `messages` content, which carries no credential surface —
/// passwords only ever live in keyring/env, never mail rows).
pub fn write_sqlite_dump_text(
    db_path: &std::path::Path,
    dump_path: &std::path::Path,
) -> Result<usize, String> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("dump open: {e}"))?;
    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' \
                 AND name NOT LIKE 'sqlite_%' AND sql NOT LIKE '%VIRTUAL TABLE%' \
                 ORDER BY name",
            )
            .map_err(|e| format!("dump tables: {e}"))?;
        let names = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("dump tables: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("dump tables: {e}"))?;
        names
    };
    let mut out = String::new();
    for table in &tables {
        out.push_str(&format!("-- table {table}\n"));
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .map_err(|e| format!("dump {table}: {e}"))?;
        let width = stmt.column_count();
        let rows: Vec<String> = {
            let mapped = stmt
                .query_map([], |row| {
                    let mut vals = Vec::with_capacity(width);
                    for i in 0..width {
                        vals.push(format!("{:?}", row.get::<_, rusqlite::types::Value>(i)?));
                    }
                    Ok(vals.join(" | "))
                })
                .map_err(|e| format!("dump {table}: {e}"))?;
            mapped
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("dump {table}: {e}"))?
        };
        for row in rows {
            out.push_str(&row);
            out.push('\n');
        }
    }
    std::fs::write(dump_path, &out).map_err(|e| format!("dump write: {e}"))?;
    Ok(out.len())
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

    fn select_condstore(&mut self) -> PinBox<'_, Result<bool, SyncError>> {
        let blocked = self.is_closed();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("select_condstore"));
            }
            self.inner.select_condstore().await
        })
    }

    fn condstore_fetch_changedsince<'a>(
        &'a mut self,
        range: &'a str,
        modseq: u64,
    ) -> PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
        let blocked = self.is_closed();
        let range = range.to_string();
        Box::pin(async move {
            if blocked {
                return Err(gate_err("condstore_fetch_changedsince"));
            }
            self.inner.condstore_fetch_changedsince(&range, modseq).await
        })
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

// ── Live steps (22-02 expansion; 22-01 deferred one-copy legs included) ───
// Every step re-asserts the sandbox prefix before mutating and re-DIFFs LIST
// against the pre-run snapshot (D-02). UIDs are always re-derived via UID
// SEARCH Message-ID (async-imap APPEND returns unit, no APPENDUID) with a
// fresh UIDVALIDITY where the entry point is epoch-gated. Each returns the
// PASS detail or the FAIL reason; run() prints the STEP line.

type StoreRef = Arc<Mutex<Store>>;

/// Rendered subjects/bodies are module constants so the D-16 audit (step 12)
/// checks exactly what the save legs rendered — never a divergent literal.
const SFX_APPEND: &str = "append-1";
const BODY_V1: &str = "v1 body";
const SFX_ONECOPY: &str = "one-copy";
const BODY_V2: &str = "v2 body";
const SFX_KILL: &str = "kill-reconcile";
const BODY_V3: &str = "v3 body";
const SFX_OFFLINE: &str = "offline-edit";
const BODY_V4: &str = "v4 latest body";
const SFX_REPLAY: &str = "replay-move";
const BODY_V5: &str = "replay body";

fn noop_cb() -> SyncCallback {
    Arc::new(|_| {})
}

async fn guard_list_clean(
    mgr: &SessionManager,
    before: &[String],
    sandbox: &str,
    extra_ok: &[&str],
) -> Result<(), String> {
    let after = list_snapshot(mgr)
        .await
        .map_err(|e| format!("re-LIST failed: {e}"))?;
    let mut delta = unexpected_list_delta(before, &after, sandbox);
    delta.retain(|n| !extra_ok.iter().any(|e| e == n));
    if delta.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected LIST delta mid-run: {delta:?}"))
    }
}

async fn search_msg_uids(
    mgr: &SessionManager,
    wire: &str,
    msg_id: &str,
) -> Result<Vec<u32>, SyncError> {
    let mut lease = mgr.lease_for(wire).await?;
    lease.session().uid_search_header("Message-ID", msg_id).await
}

fn draft_bytes(self_addr: &str, suffix: &str, body: &str, msg_id: &str) -> Vec<u8> {
    render_test_draft(self_addr, self_addr, suffix, body, msg_id, &rfc2822_now())
}

async fn flag_has_seen(mgr: &SessionManager, wire: &str, uid: u32) -> Result<bool, SyncError> {
    let mut lease = mgr.lease_for(wire).await?;
    let headers = lease.session().fetch_envelopes(&uid.to_string()).await?;
    Ok(headers
        .iter()
        .any(|h| h.uid == uid && h.flags.contains("\\Seen")))
}

/// STEP 4 (D-05): first sandbox save reconciles to exactly its own UID.
async fn live_append_first(
    mgr: &SessionManager,
    wire: &str,
    self_addr: &str,
    msg_id: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    let epoch = current_validity(mgr, wire)
        .await
        .map_err(|e| format!("validity read failed: {e}"))?;
    let bytes = draft_bytes(self_addr, SFX_APPEND, BODY_V1, msg_id);
    let uid = mgr
        .save_draft_copy_in(wire, msg_id, &bytes, None, Some(epoch))
        .await
        .map_err(|e| format!("APPEND-new failed: {e}"))?;
    let hits = search_msg_uids(mgr, wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    assert_one_copy(&hits, uid).map_err(|e| format!("one-copy: {e}"))?;
    Ok(format!("uid {uid}, SEARCH == [uid]"))
}

/// STEP 5 (D-05): second save supersedes — still exactly one copy, the new UID.
async fn live_one_copy_second(
    mgr: &SessionManager,
    wire: &str,
    self_addr: &str,
    msg_id: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    let old: u32 = {
        let hits = search_msg_uids(mgr, wire, msg_id)
            .await
            .map_err(|e| e.to_string())?;
        if hits.len() != 1 {
            return Err(format!("precondition: want 1 copy before 2nd save, got {hits:?}"));
        }
        hits[0]
    };
    let epoch = current_validity(mgr, wire)
        .await
        .map_err(|e| format!("validity read failed: {e}"))?;
    let bytes = draft_bytes(self_addr, SFX_ONECOPY, BODY_V2, msg_id);
    let uid = mgr
        .save_draft_copy_in(wire, msg_id, &bytes, Some(old), Some(epoch))
        .await
        .map_err(|e| format!("APPEND-new + expunge-old failed: {e}"))?;
    let hits = search_msg_uids(mgr, wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    assert_one_copy(&hits, uid).map_err(|e| format!("one-copy: {e}"))?;
    Ok(format!("old {old} superseded by uid {uid}, SEARCH == [uid]"))
}

/// STEP 6 (D-06): kill between APPEND-new and expunge-old, then max-wins
/// reconcile collapses to one copy. The kill signal is dropping the lease
/// mid-sequence (the manager owns reconnect internally — every `*_in`
/// retries once after its own reconnect); the next lease re-establishes the
/// session and the SEARCH + scoped-expunge legs resume the MJ-03 shape.
async fn live_kill_reconcile(
    mgr: &SessionManager,
    wire: &str,
    self_addr: &str,
    msg_id: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    let bytes = draft_bytes(self_addr, SFX_KILL, BODY_V3, msg_id);
    {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        lease
            .session()
            .append_message(wire, DRAFT_FLAGS, &bytes)
            .await
            .map_err(|e| format!("APPEND-new leg failed: {e}"))?;
    } // lease dropped here: expunge-old never ran — the kill.
    let mut hits = search_msg_uids(mgr, wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    hits.sort_unstable();
    let winner =
        max_wins(hits.clone()).ok_or_else(|| "reconcile found zero copies".to_string())?;
    let epoch = current_validity(mgr, wire)
        .await
        .map_err(|e| format!("validity read failed: {e}"))?;
    for uid in &hits {
        if *uid != winner {
            mgr.discard_server_copy_in(wire, *uid, Some(epoch))
                .await
                .map_err(|e| format!("expunge-old uid {uid} failed: {e}"))?;
        }
    }
    let after = search_msg_uids(mgr, wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    assert_one_copy(&after, winner).map_err(|e| format!("reconcile: {e}"))?;
    Ok(format!("{} staged copies collapsed to uid {winner}", hits.len()))
}

/// STEP 7 (D-07): offline edit queues a dirty draft; the reconnect flush
/// (real `sync_with_borrowed` pass, dirty-draft legs — never a hand-rolled
/// APPEND) leaves one copy with the latest content winning.
async fn live_offline_edit(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    mailbox_id: u64,
    self_addr: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    let msg_id = run_message_id("offline-edit");
    {
        let guard = store.lock().unwrap();
        queries::upsert_draft(
            guard.conn(),
            "live22-offline",
            mailbox_id,
            &msg_id,
            &format!("{SUBJECT_PREFIX}{SFX_OFFLINE}"),
            BODY_V4,
            self_addr,
            "",
            "",
        )
        .map_err(|e| format!("queue dirty draft: {e}"))?;
    }
    let worker = SyncWorker::new(store.clone());
    {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("reconnect pass failed: {e}"))?;
    }
    let hits = search_msg_uids(mgr, wire, &msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if hits.len() != 1 {
        return Err(format!("offline edit: want 1 copy, got {hits:?}"));
    }
    let bytes = {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        lease.session().fetch_body(hits[0]).await.map_err(|e| e.to_string())?
    };
    let parsed = mail_parser::MessageParser::new()
        .parse(&bytes)
        .ok_or_else(|| "flushed copy unparseable".to_string())?;
    let body = parsed.body_text(0).map(|b| b.to_string()).unwrap_or_default();
    if body != BODY_V4 {
        return Err("offline edit: flushed content is not the latest".to_string());
    }
    Ok(format!("uid {}, latest content won", hits[0]))
}

/// STEP 8 (D-08): PEEK-only content assert — subject AND body equal the last
/// save. `fetch_body` is `UID FETCH BODY.PEEK[]`: asserts never disturb
/// Seen state (M1 invariant).
async fn live_content(
    mgr: &SessionManager,
    wire: &str,
    msg_id: &str,
) -> Result<String, String> {
    let hits = search_msg_uids(mgr, wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if hits.len() != 1 {
        return Err(format!("content assert: want 1 copy, got {hits:?}"));
    }
    let bytes = {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        lease.session().fetch_body(hits[0]).await.map_err(|e| e.to_string())?
    };
    let parsed = mail_parser::MessageParser::new()
        .parse(&bytes)
        .ok_or_else(|| "surviving copy unparseable".to_string())?;
    let subject = parsed.subject().unwrap_or("").to_string();
    let body = parsed.body_text(0).map(|b| b.to_string()).unwrap_or_default();
    let want_subject = format!("{SUBJECT_PREFIX}{SFX_KILL}");
    if subject != want_subject || body != BODY_V3 {
        return Err(format!(
            "content mismatch: subject {subject:?} body {body:?}"
        ));
    }
    Ok(format!("uid {} subject+body match last save", hits[0]))
}

/// STEP 9 (D-10): queued move replays pre-sweep through the real full pass
/// (worker Step 4b runs before the header sweep — structural, verified in
/// RESEARCH). Pending depth 1+ → 0 with the UID moved, never resurrected
/// same-pass. The archive folder is a second sandbox (`{wire}-arc`, prefix
/// holds) so no real folder is ever a move target.
async fn live_replay_order(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    mailbox_id: u64,
    self_addr: &str,
) -> Result<(String, u32), String> {
    let arc = format!("{wire}-arc");
    assert_sandbox_prefix(&arc).map_err(|e| e.to_string())?;
    mgr.create_mailbox_in(&arc)
        .await
        .map_err(|e| format!("CREATE arc failed: {e}"))?;
    let msg_id = run_message_id("replay-move");
    let epoch = current_validity(mgr, wire)
        .await
        .map_err(|e| format!("validity read failed: {e}"))?;
    let bytes = draft_bytes(self_addr, SFX_REPLAY, BODY_V5, &msg_id);
    let uid = mgr
        .save_draft_copy_in(wire, &msg_id, &bytes, None, Some(epoch))
        .await
        .map_err(|e| format!("seed APPEND failed: {e}"))?;
    let depth_before: i64 = {
        let guard = store.lock().unwrap();
        queries::enqueue_imap_outbox(
            guard.conn(),
            mailbox_id,
            uid,
            queries::IMAP_OP_MOVE,
            Some(&arc),
            epoch,
        )
        .map_err(|e| format!("enqueue move op: {e}"))?;
        pending_depth(guard.conn(), mailbox_id)
    };
    if depth_before <= 0 {
        return Err("pending indicator never visible (depth 0 right after enqueue)".to_string());
    }
    let worker = SyncWorker::new(store.clone());
    {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("replay pass failed: {e}"))?;
    }
    let depth_after: i64 = {
        let guard = store.lock().unwrap();
        pending_depth(guard.conn(), mailbox_id)
    };
    if depth_after != 0 {
        return Err(format!("queue not drained: depth {depth_after}"));
    }
    let src_hits = search_msg_uids(mgr, wire, &msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if !src_hits.is_empty() {
        return Err(format!("moved UID resurrected same pass: {src_hits:?}"));
    }
    let dst_hits = search_msg_uids(mgr, &arc, &msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if dst_hits.len() != 1 {
        return Err(format!("moved copy missing in arc: {dst_hits:?}"));
    }
    Ok((arc, dst_hits[0]))
}

/// STEP 10 (D-11): offline delete via the real discard path, then TWO full
/// syncs — the UID stays absent from SEARCH and the local row is gone both
/// passes (tombstoned, never resurrected by backfill). Tears the arc folder
/// down so LIST returns to the sandbox-only delta.
async fn live_delete_double_sync(
    mgr: &SessionManager,
    store: &StoreRef,
    arc_wire: &str,
    arc_uid: u32,
) -> Result<String, String> {
    assert_sandbox_prefix(arc_wire).map_err(|e| e.to_string())?;
    let epoch = current_validity(mgr, arc_wire)
        .await
        .map_err(|e| format!("validity read failed: {e}"))?;
    mgr.discard_server_copy_in(arc_wire, arc_uid, Some(epoch))
        .await
        .map_err(|e| format!("real delete path failed: {e}"))?;
    let worker = SyncWorker::new(store.clone());
    for pass in [1u8, 2] {
        {
            let mut lease = mgr.lease_for(arc_wire).await.map_err(|e| e.to_string())?;
            worker
                .sync_with_borrowed(lease.session(), arc_wire, noop_cb())
                .await
                .map_err(|e| format!("sync #{pass} failed: {e}"))?;
        }
        let uids = search_uids_in(mgr, arc_wire)
            .await
            .map_err(|e| e.to_string())?;
        if uids.contains(&arc_uid) {
            return Err(format!("sync #{pass}: UID resurrected by backfill"));
        }
        let guard = store.lock().unwrap();
        let arc_id = queries::ensure_mailbox(guard.conn(), arc_wire)
            .map_err(|e| e.to_string())?;
        let local = queries::all_local_uids(guard.conn(), arc_id)
            .map_err(|e| e.to_string())?;
        if local.contains(&arc_uid) {
            return Err(format!("sync #{pass}: local row survives"));
        }
    }
    mgr.delete_mailbox_in(arc_wire)
        .await
        .map_err(|e| format!("arc teardown failed: {e}"))?;
    Ok("UID absent both passes, local row gone, arc removed".to_string())
}

/// STEP 11 (D-12): Seen round-trips both directions — online via the real
/// `set_seen_in` verbs, then queued-and-replayed via `replay_outbox` with
/// the epoch-mismatch and absent-UID drop rules armed (current epoch, live
/// UID set). FLAGS asserts read through `fetch_envelopes` only (PEEK-safe).
async fn live_seen_both_ways(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    mailbox_id: u64,
    msg_id: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    let hits = search_msg_uids(mgr, wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if hits.len() != 1 {
        return Err(format!("seen target: want 1 copy, got {hits:?}"));
    }
    let uid = hits[0];
    mgr.set_seen_in(wire, uid, true)
        .await
        .map_err(|e| format!("online read STORE failed: {e}"))?;
    if !flag_has_seen(mgr, wire, uid).await.map_err(|e| e.to_string())? {
        return Err("online read: \\Seen absent after STORE".to_string());
    }
    mgr.set_seen_in(wire, uid, false)
        .await
        .map_err(|e| format!("online unread STORE failed: {e}"))?;
    if flag_has_seen(mgr, wire, uid).await.map_err(|e| e.to_string())? {
        return Err("online unread: \\Seen present after un-STORE".to_string());
    }
    let worker = SyncWorker::new(store.clone());
    for seen in [true, false] {
        let epoch = current_validity(mgr, wire)
            .await
            .map_err(|e| format!("validity read failed: {e}"))?;
        {
            let guard = store.lock().unwrap();
            queries::enqueue_outbox(guard.conn(), mailbox_id, uid, seen, epoch)
                .map_err(|e| format!("enqueue seen={seen}: {e}"))?;
            if pending_depth(guard.conn(), mailbox_id) <= 0 {
                return Err("pending indicator never visible for Seen op".to_string());
            }
        }
        let live: HashSet<u32> = search_uids_in(mgr, wire)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();
        {
            let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
            let summary = worker
                .replay_outbox(lease.session(), mailbox_id, epoch, Some(&live))
                .await
                .map_err(|e| format!("replay seen={seen} failed: {e}"))?;
            if summary.acked != 1 {
                return Err(format!(
                    "replay seen={seen}: acked={} dropped={} failed={}",
                    summary.acked, summary.dropped, summary.failed
                ));
            }
        }
        let has = flag_has_seen(mgr, wire, uid).await.map_err(|e| e.to_string())?;
        if has != seen {
            return Err(format!("replayed seen={seen}: FLAGS reads {has}"));
        }
    }
    Ok(format!("uid {uid}: read+unread online and replayed"))
}

/// STEP 12 (D-16): no-unprefixed-send audit over the temp send_queue/outbox
/// tables plus every subject rendered in-run. This phase sends no SMTP; any
/// queued row must still be self-only, and every subject must carry the
/// `SGE-TEST-` prefix.
async fn live_audit_no_unprefixed(
    store: &StoreRef,
    subjects: &[String],
    self_addr: &str,
) -> Result<String, String> {
    let subject_refs: Vec<&str> = subjects.iter().map(|s| s.as_str()).collect();
    audit_test_mail(&subject_refs, &[self_addr], self_addr)?;
    let guard = store.lock().unwrap();
    let conn = guard.conn();
    let draft_subjects: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT subject FROM drafts")
            .map_err(|e| e.to_string())?;
        let listed = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        listed
    };
    let draft_refs: Vec<&str> = draft_subjects.iter().map(|s| s.as_str()).collect();
    audit_test_mail(&draft_refs, &[], self_addr)?;
    let queued: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT to_addrs, cc_addrs, bcc_addrs FROM send_queue")
            .map_err(|e| e.to_string())?;
        let listed = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        listed
    };
    for (to, cc, bcc) in &queued {
        for cell in [to, cc, bcc] {
            for addr in split_addrs_cell(cell)? {
                if addr != self_addr {
                    return Err(
                        "non-self recipient fails D-16 audit (only mail to self)".to_string()
                    );
                }
            }
        }
    }
    Ok(format!(
        "{} in-run subjects + {} draft rows prefixed, {} send rows all self-only",
        subjects.len(),
        draft_subjects.len(),
        queued.len()
    ))
}

/// Strict parser for the JSON-array address cells (`to_addrs` et al.).
/// Anything unparseable fails the audit — never lenient on recipients.
fn split_addrs_cell(cell: &str) -> Result<Vec<String>, String> {
    let t = cell.trim();
    if t.is_empty() || t == "[]" {
        return Ok(vec![]);
    }
    if !(t.starts_with('[') && t.ends_with(']')) {
        return Err(format!("unparseable addrs cell fails D-16 audit: {cell:?}"));
    }
    let mut out = Vec::new();
    for part in t[1..t.len() - 1].split(',') {
        let addr = part.trim().trim_matches('"').trim_matches('\'').trim();
        if !addr.is_empty() {
            out.push(addr.to_string());
        }
    }
    Ok(out)
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
    let mb_id: u64 = {
        let guard = store.lock().unwrap();
        match (|| -> Result<u64, String> {
            let id =
                queries::ensure_mailbox(guard.conn(), &sandbox).map_err(|e| e.to_string())?;
            queries::set_sync_state(guard.conn(), &sandbox, epoch, 0).map_err(|e| e.to_string())?;
            Ok(id)
        })() {
            Ok(id) => id,
            Err(e) => {
                step(2, "create", false, &format!("store seed failed: {e}"));
                return cleanup(&manager, &db_path, &sandbox, &before).await;
            }
        }
    };

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

    // In-run subjects for the D-16 audit — pushed from the same constants
    // the save legs render, so the audit checks exactly what was sent.
    let msg_main = run_message_id("main");
    let mut subjects: Vec<String> = Vec::new();

    // Small sequencer: FAIL prints the STEP line and always runs cleanup.
    macro_rules! live_step {
        ($n:expr, $name:expr, $expr:expr) => {
            match $expr {
                Ok(detail) => {
                    step($n, $name, true, &detail);
                }
                Err(reason) => {
                    step($n, $name, false, &reason);
                    return cleanup(&manager, &db_path, &sandbox, &before).await;
                }
            }
        };
    }

    // 4. append-1 — first sandbox save, one copy (D-05).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(4, "append-1", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(
        4,
        "append-1",
        live_append_first(&manager, &sandbox, &cfg.self_addr, &msg_main).await
    );
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_APPEND}"));

    // 5. one-copy — second save supersedes, still exactly one (D-05).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(5, "one-copy", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(
        5,
        "one-copy",
        live_one_copy_second(&manager, &sandbox, &cfg.self_addr, &msg_main).await
    );
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_ONECOPY}"));

    // 6. kill-reconcile — lease dropped mid-sequence, max-wins collapses (D-06).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(6, "kill-reconcile", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(
        6,
        "kill-reconcile",
        live_kill_reconcile(&manager, &sandbox, &cfg.self_addr, &msg_main).await
    );
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_KILL}"));

    // 7. offline-edit — dirty draft flush, latest wins (D-07).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(7, "offline-edit", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(
        7,
        "offline-edit",
        live_offline_edit(&manager, &store, &sandbox, mb_id, &cfg.self_addr).await
    );
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_OFFLINE}"));

    // 8. content — PEEK-only subject+body assert (D-08).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(8, "content", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(8, "content", live_content(&manager, &sandbox, &msg_main).await);

    // 9. replay-order — queued move replays pre-sweep, depth 1+ to 0 (D-10).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(9, "replay-order", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    let (arc_wire, arc_uid) =
        match live_replay_order(&manager, &store, &sandbox, mb_id, &cfg.self_addr).await {
            Ok(pair) => {
                step(
                    9,
                    "replay-order",
                    true,
                    &format!(
                        "depth 1+ to 0, uid {} moved to {} (order evidence: worker Step 4b log before sweep, checked by wrapper)",
                        pair.1, pair.0
                    ),
                );
                pair
            }
            Err(reason) => {
                step(9, "replay-order", false, &reason);
                return cleanup(&manager, &db_path, &sandbox, &before).await;
            }
        };
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_REPLAY}"));

    // 10. delete-double-sync — UID stays gone across two syncs (D-11).
    // The arc folder is an expected LIST delta until this step removes it.
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[&arc_wire]).await {
        step(10, "delete-double-sync", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(
        10,
        "delete-double-sync",
        live_delete_double_sync(&manager, &store, &arc_wire, arc_uid).await
    );

    // 11. seen-both-ways — online and replayed, read and unread (D-12).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(11, "seen-both-ways", false, &e);
        return cleanup(&manager, &db_path, &sandbox, &before).await;
    }
    live_step!(
        11,
        "seen-both-ways",
        live_seen_both_ways(&manager, &store, &sandbox, mb_id, &msg_main).await
    );

    // 12. no-unprefixed-send — drafts + send_queue + in-run subjects (D-16).
    live_step!(
        12,
        "no-unprefixed-send",
        live_audit_no_unprefixed(&store, &subjects, &cfg.self_addr).await
    );

    // 13. cleanup — ALWAYS runs: SQLite dump for the secret scan, then
    // expunge leftovers, DELETE the sandbox with backoff (D-03), re-LIST
    // must equal the pre-run snapshot.
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

/// DELETE-with-backoff shared by `cleanup` and `--cleanup-only` (D-03):
/// 3 attempts, 5s/15s/45s, then the caller prints exact manual steps —
/// never an unbounded destructive loop.
async fn delete_with_backoff(mgr: &SessionManager, sandbox: &str) -> bool {
    for (attempt, secs) in CLEANUP_BACKOFF_SECS.iter().enumerate() {
        if assert_sandbox_prefix(sandbox).is_err() {
            return false;
        }
        match mgr.delete_mailbox_in(sandbox).await {
            Ok(()) => return true,
            Err(e) => {
                eprintln!(
                    "[live-gate] cleanup DELETE attempt {} failed ({e}) — backoff {secs}s",
                    attempt + 1
                );
                std::thread::sleep(std::time::Duration::from_secs(*secs));
            }
        }
    }
    false
}

/// `--cleanup-only <sandbox>`: retry just the DELETE + LIST-diff for a
/// folder a previous run left behind (the wrapper invokes this on the same
/// backoff schedule after a MANUAL-CLEANUP before surfacing manual steps).
async fn cleanup_only(sandbox: &str) -> i32 {
    if let Err(e) = assert_sandbox_prefix(sandbox) {
        eprintln!("CLEANUP-ONLY refused: {e}");
        return 1;
    }
    if load_config().is_err() {
        println!("SKIP live-gate offline (no creds)");
        return 2;
    }
    // load_config validated env presence; rebuild the manager the same way.
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP live-gate offline (no creds): {e}");
            return 2;
        }
    };
    let manager = SessionManager::new(account_config(&cfg));
    if delete_with_backoff(&manager, sandbox).await {
        let after = list_snapshot(&manager).await.unwrap_or_default();
        if !after.iter().any(|n| n == sandbox) {
            println!("CLEANUP-ONLY {sandbox}: DELETED, LIST clean");
            return 0;
        }
    }
    println!("CLEANUP-ONLY {sandbox}: FAIL — {}", manual_cleanup_steps(sandbox));
    1
}

/// Cleanup (D-03): SQLite dump for the secret scan, expunge-all-in-sandbox
/// reusing `discard_server_copy_in`, then DELETE with retry; post-LIST must
/// equal the pre-run snapshot. Prints the run REPORT (per-step verdicts
/// live on stdout above; REPORT condenses list/scan-surface/cleanup).
async fn cleanup(
    mgr: &SessionManager,
    db_path: &std::path::Path,
    sandbox: &str,
    before: &[String],
) -> i32 {
    let mut ok = true;
    // Dump BEFORE expunge/DELETE so the scan sees the full run state
    // (queues, drafts, sync rows). The wrapper greps this file.
    let dump_path = std::env::temp_dir().join(format!(
        "sge-live22-{}-{}.dump",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    match write_sqlite_dump_text(db_path, &dump_path) {
        Ok(n) => println!("DUMP {} {n} bytes", dump_path.display()),
        Err(e) => eprintln!("[live-gate] dump failed ({e}) — scan surface reduced"),
    }
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
    let mut deleted = delete_with_backoff(mgr, sandbox).await;
    if !deleted {
        ok = false;
    }
    // If the prefix guard refused, `delete_with_backoff` never ran — keep
    // the refusal loud and skip the LIST diff against a half-known state.
    if assert_sandbox_prefix(sandbox).is_err() {
        ok = false;
        deleted = false;
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
    let code = if ok && clean { 0 } else { 1 };
    println!(
        "REPORT steps={TOTAL_STEPS} list_restored={} cleanup={} dump={}",
        if clean { "yes" } else { "no" },
        if ok && clean {
            "deleted"
        } else {
            "MANUAL"
        },
        dump_path.display()
    );
    code
}

const USAGE: &str = r#"live_gate_22 — Phase 22 sandboxed live gate (STEP 1-13)

USAGE:
    live_gate_22 [--seed-only]
    live_gate_22 --cleanup-only <SGE-Test-folder>

OPTIONS:
    --seed-only    Write SGE_LIVE_IMAP_* password + server config to the OS
                   keyring once, then exit (no IMAP traffic)
    --cleanup-only Retry DELETE + LIST-diff for one leftover sandbox folder
                   (wrapper cleanup-retry path), then exit

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seed_only_flag = false;
    let mut cleanup_only_target: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seed-only" => seed_only_flag = true,
            "--cleanup-only" => {
                i += 1;
                if i >= args.len() {
                    eprintln!(
                        "usage error: --cleanup-only needs a sandbox folder name\n\n{USAGE}"
                    );
                    return ExitCode::from(2);
                }
                cleanup_only_target = Some(args[i].clone());
            }
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
        i += 1;
    }
    if let Some(target) = cleanup_only_target {
        return ExitCode::from(async_std::task::block_on(cleanup_only(&target)) as u8);
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
