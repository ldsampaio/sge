//! `live_gate_21` — Phase 21 tracer: sandboxed live proof of the Phase 13
//! send pipeline against the real server.
//!
//! Structural clone of `live_gate_22.rs` (same STEP PASS/FAIL runner, exit
//! 0/1/2, `--seed-only` keyring path, `/tmp` logs, GateSession decorator,
//! pre-flight LIST-snapshot, cleanup retry 3× 5s/15s/45s), driving the send
//! pipeline through `enqueue_send` → `SyncWorker::flush_send_queue` with a
//! real `SmtpTransporter` behind a counting decorator.
//!
//! 12 STEPs: 1 preflight, 2 recipient-guard, 3 arrival, 4 sent-copy,
//! 5 no-duplicate, 6 sent-unfiled, 7 offline-flush (D-09), 8 uncertain-
//! reconcile (D-10), 9 crash-recover (D-11), 10 draft-send (D-16, binary
//! verdict), 11 autosave-probe, 12 cleanup (D-15) + REPORT.
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
use sge_lib::drafts::new_message_id;
use sge_lib::send_queue::{self, EnqueueInput, SendGate};
use sge_lib::smtp::{SmtpAccount, SmtpOutcome, SmtpTransport, SmtpTransporter};
use sge_lib::store::{queries, Store};
use sge_lib::sync::worker::{resolve_sent_wire, FlushSummary, SendFlushEnv, SyncWorker};

/// Sandbox folder prefix (D-03, mirrored from Phase 22). Every harness-issued
/// mutating call asserts it in code; Phase 21 creates no sandbox folder (the
/// only mutations are server delivery into INBOX and the pipeline's own Sent
/// APPEND), so the pre-flight LIST guard aborts on ANY unexpected delta.
pub const SANDBOX_PREFIX: &str = "SGE-Test-";

/// Subject prefix marking every test mail (D-14: no mail WITHOUT this prefix
/// may leave the box).
pub const SUBJECT_PREFIX: &str = "SGE-TEST-";

/// Step count for the full gate (21-02 expansion: failure-mode legs 7-9,
/// draft-send leg 10, probe 11, cleanup 12).
pub const TOTAL_STEPS: usize = 12;

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

/// Shared-counter SMTP decorator: forwards to the inner transport while
/// counting into a caller-owned counter that outlives one flush pass. The
/// never-resend proof (D-07) spans legs — one counter across the arrival,
/// no-duplicate and sent-unfiled flushes — so per-env counters would hide a
/// second `send_raw`.
pub struct SharedCountingTransport {
    inner: Arc<dyn SmtpTransport>,
    calls: Arc<AtomicUsize>,
}

impl SharedCountingTransport {
    pub fn new(inner: Arc<dyn SmtpTransport>, calls: Arc<AtomicUsize>) -> Self {
        Self { inner, calls }
    }
}

impl SmtpTransport for SharedCountingTransport {
    fn send_raw(
        &self,
        account: &SmtpAccount,
        envelope_from: &str,
        envelope_to: &[String],
        bytes: &[u8],
    ) -> SmtpOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.send_raw(account, envelope_from, envelope_to, bytes)
    }
}

// ── DATA-timeout fault decorator (D-10, zero prod-code changes) ──────────
// Wraps the real transport: while `fault_armed`, returns
// `SmtpOutcome::Uncertain` WITHOUT calling the inner transport (zero SMTP
// bytes leave the box on the faulted attempt — the deterministic analogue of
// a DATA-timeout, cf. RESEARCH decorator recommendation). Disarmed, forwards
// and counts the real call. `inner_calls` counts only forwarded calls, so a
// reconcile-not-resend proof reads it across `reconcile_uncertain_sends`
// (a SEARCH hit must confirm with the count UNCHANGED).

pub struct TimeoutTransport {
    inner: Arc<dyn SmtpTransport>,
    armed: Arc<AtomicBool>,
    inner_calls: Arc<AtomicUsize>,
}

impl TimeoutTransport {
    pub fn new(inner: Arc<dyn SmtpTransport>) -> (Self, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let armed = Arc::new(AtomicBool::new(false));
        let inner_calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                inner,
                armed: armed.clone(),
                inner_calls: inner_calls.clone(),
            },
            armed,
            inner_calls,
        )
    }
}

impl SmtpTransport for TimeoutTransport {
    fn send_raw(
        &self,
        account: &SmtpAccount,
        envelope_from: &str,
        envelope_to: &[String],
        bytes: &[u8],
    ) -> SmtpOutcome {
        if self.armed.load(Ordering::SeqCst) {
            return SmtpOutcome::Uncertain {
                reason: "live-gate: injected DATA timeout".to_string(),
            };
        }
        self.inner_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.send_raw(account, envelope_from, envelope_to, bytes)
    }
}

/// Harness-issued APPEND scoping (D-03, T-21-05): a manual APPEND is allowed
/// ONLY to an explicitly passed allow-list — the preflight-resolved Sent wire
/// (uncertain-leg simulated server-accept, mirroring the filing leg's APPEND)
/// or the per-run sandbox / recorded Drafts fallback (draft leg). INBOX is
/// never appendable by the harness (arrival lands by server delivery only),
/// so it can never appear in the allow-list.
pub fn assert_harness_append_target(wire: &str, allowed: &[String]) -> Result<(), SyncError> {
    if allowed.iter().any(|a| a == wire) {
        Ok(())
    } else {
        Err(SyncError::Refused(format!(
            "live-gate: refusing harness APPEND to {wire:?} (allowed: {allowed:?})"
        )))
    }
}

// ── Send-audit helpers (D-14, T-21-01 — pure, offline-testable) ──────────
// The live legs print one AUDIT-ROW / AUDIT-EML / AUDIT-CALLS / AUDIT-MID
// line per item; `scripts/live-21.sh` re-verifies the five rules from the
// wrapper log (defense in depth — the bin aborts on any violation first).

/// Rule 1: every queue row's recipients are exactly self-only (mirrors
/// `assert_self_only`; columns arrive as the stored JSON arrays).
pub fn audit_recipients_ok(to_json: &str, cc_json: &str, bcc_json: &str, self_addr: &str) -> bool {
    let want = self_addr.trim().to_ascii_lowercase();
    let to: Vec<String> = serde_json::from_str(to_json).unwrap_or_default();
    let cc: Vec<String> = serde_json::from_str(cc_json).unwrap_or_default();
    let bcc: Vec<String> = serde_json::from_str(bcc_json).unwrap_or_default();
    to.len() == 1
        && to[0].trim().to_ascii_lowercase() == want
        && cc.iter().all(|s| s.trim().is_empty())
        && bcc.iter().all(|s| s.trim().is_empty())
}

/// Rule 2: every queued `.eml` Subject carries the sandbox prefix (A-09).
pub fn audit_subject_prefixed(subject: &str) -> bool {
    subject.starts_with(SUBJECT_PREFIX)
}

/// Rule 3: no `Bcc:` header line in the rendered bytes (envelope-only
/// invariant, re-asserted live on the actual sent bytes; D-12: live BCC
/// sends are never attempted). Only the header block is scanned — a "Bcc:"
/// mention in the body is NOT a leak. Folded continuation lines (leading
/// whitespace) can never start a header.
pub fn eml_has_bcc_header(eml_bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(eml_bytes);
    let mut lines = text.lines();
    for line in &mut lines {
        let stripped = line.strip_suffix('\r').unwrap_or(line);
        if stripped.is_empty() {
            break; // end of the header block — body follows, unscanned
        }
        if stripped.to_ascii_lowercase().starts_with("bcc:") {
            return true;
        }
    }
    false
}

/// Rule 4: per-run unique Message-IDs — first duplicate found, if any.
pub fn audit_first_duplicate_mid<'a>(mids: &[&'a str]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    for m in mids {
        if !seen.insert(*m) {
            return Some((*m).to_string());
        }
    }
    None
}

/// Rule 5 expectation per leg: how many transport calls a leg may add.
/// Reconcile / re-flush / wait-poll legs must add ZERO (catches stray
/// resends — the reconcile legs prove reconcile-not-resend).
pub fn expected_calls_delta(leg: &str) -> Option<usize> {
    match leg {
        "arrival" | "no-duplicate" | "sent-unfiled" | "offline-flush" | "draft-send"
        | "crash-final-flush" => Some(1),
        "sent-unfiled-reflush" | "uncertain-armed-flush" | "uncertain-reconcile"
        | "crash-triage-flush" | "crash-reconcile" | "crash-backoff-poll" => Some(0),
        _ => None,
    }
}

// ── Secret-scan helper (D-13, T-21-04 — pure, offline-testable) ──────────
// Fixed-substring match (passwords contain regex metachars — never regex).
// Empty secrets never match. Returns true on ANY hit; the caller names WHICH
// artifact matched, never the secret itself.

pub fn scan_text_has_secret(text: &str, secrets: &[&str]) -> bool {
    secrets
        .iter()
        .any(|s| !s.is_empty() && text.contains(s))
}

/// Full SQLite dump for the secret scan (D-13, T-21-04): renders EVERY
/// table of the temp store. Runs in-process BEFORE cleanup removes the DB
/// (post-run the file is gone, so the wrapper cannot dump it). Returns
/// (table names, total rows, dump text). The caller greps the dump for the
/// password values as fixed substrings and names WHICH artifact matched,
/// never the secret itself.
pub fn dump_store_for_scan(conn: &rusqlite::Connection) -> (Vec<String>, usize, String) {
    let mut tables = Vec::new();
    let mut rows = 0usize;
    let mut dump = String::new();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))
                .map(|it| it.filter_map(|n| n.ok()).collect())
        })
        .unwrap_or_default();
    for name in names {
        tables.push(name.clone());
        let sql = format!("SELECT * FROM \"{}\"", name.replace('"', ""));
        if let Ok(mut s2) = conn.prepare(&sql) {
            let coln = s2.column_count();
            if let Ok(mapped) = s2.query_map([], |r| {
                let mut cells = Vec::with_capacity(coln);
                for i in 0..coln {
                    let v: rusqlite::types::Value = r.get(i)?;
                    cells.push(format!("{:?}", v));
                }
                Ok(cells.join("|"))
            }) {
                for line in mapped.filter_map(|l| l.ok()) {
                    rows += 1;
                    dump.push_str(&line);
                    dump.push('\n');
                }
            }
        }
    }
    (tables, rows, dump)
}

/// Backoff-schedule check (D-09): a deferred row's `next_retry_at` must fall
/// inside `backoff_bounds(attempts)` measured from `now_epoch`.
/// `slack_secs` absorbs the seconds between the schedule write and this
/// read (the check runs after network I/O, so strict `>= lo` would flake).
pub fn retry_within_bounds(
    next_retry_at: &Option<String>,
    attempts: u32,
    now_epoch: i64,
    slack_secs: i64,
) -> bool {
    let ts = match next_retry_at {
        Some(s) => s,
        None => return false,
    };
    let parsed = match chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt.and_utc().timestamp(),
        Err(_) => return false,
    };
    let (lo, hi) = send_queue::backoff_bounds(attempts);
    let diff = parsed - now_epoch;
    diff >= lo - slack_secs && diff <= hi
}

/// Pure shape of the deferred-backoff audit: `(row id, attempts,
/// next_retry_at)` triples checked against the bounds at `now_epoch`.
/// Empty input passes vacuously (a clean first-try send defers nothing —
/// the bounds VALUES are pinned by offline tests either way).
pub fn deferred_backoff_ok(
    rows: &[(String, i64, Option<String>)],
    now_epoch: i64,
) -> (bool, String) {
    if rows.is_empty() {
        return (
            true,
            "no deferred rows (clean first-try send); bounds shape pinned offline \
             (backoff_bounds(0)==(24,36), cap (720,1080))"
                .to_string(),
        );
    }
    let mut bad = Vec::new();
    for (id, attempts, next) in rows {
        let attempts_u = (*attempts).max(0) as u32;
        if !retry_within_bounds(next, attempts_u, now_epoch, 15) {
            bad.push(id.clone());
        }
    }
    if bad.is_empty() {
        (true, format!("{} deferred row(s) inside backoff_bounds", rows.len()))
    } else {
        (false, format!("deferred rows outside backoff_bounds: {:?}", bad))
    }
}

// ── Canonical STEP outcome log (REPORT backbone + exit-code contract) ────
// Every canonical STEP verdict is recorded here (in print order). `cleanup`
// fails the run when ANY logged step failed — a mid-run FAIL can no longer
// exit 0 through a clean cleanup (the exit 0/1 contract in USAGE).

#[derive(Clone)]
pub struct LoggedStep {
    pub n: usize,
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub fn log_step(tracked: &TrackedMail, n: usize, name: &str, ok: bool, detail: &str) -> bool {
    if let Ok(mut log) = tracked.step_log.lock() {
        log.push(LoggedStep {
            n,
            name: name.to_string(),
            ok,
            detail: detail.to_string(),
        });
    }
    step(n, name, ok, detail)
}

pub fn record_calls(tracked: &TrackedMail, leg: &str, before: usize, after: usize) {
    if let Ok(mut log) = tracked.calls_log.lock() {
        log.push((leg.to_string(), before, after));
    }
}

pub fn logged_outcome(tracked: &TrackedMail, n: usize, name: &str) -> Option<bool> {
    tracked
        .step_log
        .lock()
        .ok()
        .and_then(|log| log.iter().find(|s| s.n == n && s.name == name).map(|s| s.ok))
}

// ── Live IMAP helpers for the failure-mode legs ──────────────────────────

/// Bounded poll for server-side delivery (D-05): 3×10s, then Err — never an
/// open-ended wait. Epoch-guarded (aborts loud on UIDVALIDITY change).
async fn poll_inbox_for_mid(
    mgr: &SessionManager,
    mid: &str,
    validity: u32,
) -> Result<u32, String> {
    let mut detail = String::new();
    for round in 0..ARRIVAL_POLL_ROUNDS {
        if round > 0 {
            async_std::task::sleep(std::time::Duration::from_secs(ARRIVAL_POLL_SECS)).await;
        }
        let mut lease = mgr
            .lease_for("INBOX")
            .await
            .map_err(|e| format!("INBOX lease failed: {e}"))?;
        match lease.selected_validity() {
            Some(v) if v == validity => {}
            other => {
                return Err(format!(
                    "INBOX epoch change mid-run (want {validity:?}, got {other:?}) — aborting loud"
                ));
            }
        }
        match lease.session().uid_search_header("Message-ID", mid).await {
            Ok(hits) => {
                if hits.len() == 1 {
                    return Ok(hits[0]);
                }
                detail = format!("round {}: {} INBOX hits (want 1)", round + 1, hits.len());
            }
            Err(e) => return Err(format!("INBOX SEARCH failed: {e}")),
        }
    }
    Err(detail)
}

/// PEEK-only header confirm over any folder wire (never BODY[] — the fetch
/// must not set `\Seen`; RESEARCH Pitfall 3).
async fn peek_confirms_mid(mgr: &SessionManager, wire: &str, uid: u32, mid: &str) -> bool {
    let mut lease = match mgr.lease_for(wire).await {
        Ok(l) => l,
        Err(_) => return false,
    };
    match lease.session().fetch_body(uid).await {
        Ok(bytes) => match mail_parser::MessageParser::new().parse(&bytes) {
            Some(m) => {
                let subj_ok = m
                    .subject()
                    .map(|s| s.starts_with(SUBJECT_PREFIX))
                    .unwrap_or(false);
                let bare = mid.trim_matches(&['<', '>'][..]);
                let mid_ok = m.message_id().map(|id| id == bare).unwrap_or(false);
                subj_ok && mid_ok
            }
            None => false,
        },
        Err(_) => false,
    }
}

/// One-shot Message-ID SEARCH over the Sent wire.
async fn search_sent_for_mid(
    mgr: &SessionManager,
    sent_wire: &str,
    mid: &str,
) -> Result<Vec<u32>, String> {
    let mut lease = mgr
        .lease_for(sent_wire)
        .await
        .map_err(|e| format!("Sent lease failed: {e}"))?;
    lease
        .session()
        .uid_search_header("Message-ID", mid)
        .await
        .map_err(|e| format!("Sent SEARCH failed: {e}"))
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

/// Re-LIST after a leg and abort on any unexpected delta (D-03): the
/// harness creates no folders, so every delta is external interference or a
/// wrong-folder write. Server delivery into INBOX never trips this (LIST
/// sees folders, not contents).
async fn check_list_stable(
    mgr: &SessionManager,
    before: &[String],
    guard_marker: &str,
    tracked: &TrackedMail,
    n: usize,
    name: &str,
) -> bool {
    match list_snapshot(mgr).await {
        Ok(after) => {
            let delta = unexpected_list_delta(before, &after, guard_marker);
            log_step(tracked, n, name, delta.is_empty(), &format!("LIST deltas: {:?}", delta))
        }
        Err(e) => log_step(tracked, n, name, false, &format!("re-LIST failed: {}", e)),
    }
}

fn step(n: usize, name: &str, ok: bool, detail: &str) -> bool {
    println!(
        "STEP {n}/{TOTAL_STEPS} {name}: {} {detail}",
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

/// Expunge one tracked UID through its own folder wire (T-21-05: tracked
/// UIDs only, never bare expunge), retry 3x with backoff, then give up to
/// manual steps — never blind infinite retry.
async fn expunge_tracked(mgr: &SessionManager, wire: &str, uid: u32, mid: &str) -> bool {
    for (attempt, secs) in CLEANUP_BACKOFF_SECS.iter().enumerate() {
        match mgr.discard_server_copy_in(wire, uid, None).await {
            Ok(()) => return true,
            Err(e) => {
                eprintln!(
                    "[live-gate] cleanup expunge {} uid {} ({}) attempt {} failed ({}) — backoff {}s",
                    wire,
                    uid,
                    mid,
                    attempt + 1,
                    e,
                    secs
                );
                std::thread::sleep(std::time::Duration::from_secs(*secs));
            }
        }
    }
    false
}

fn manual_cleanup_steps(mids: &[String]) -> String {
    format!(
        "MANUAL-CLEANUP test message-ids {mids:?}: webmail → INBOX + Sent → \
         SEARCH each Message-ID → delete hits; then re-run preflight (LIST must match snapshot)"
    )
}

/// Tracked test mail for cleanup: every Message-ID the run sent, with the
/// INBOX/Sent UIDs the asserts observed. `unfiled` marks Message-IDs whose
/// Sent APPEND leg failed (SentUnfiled): the autosave probe tolerates 0 or 1
/// Sent UIDs for those (0 = no autosave, 1 = server autosaved), while filed
/// legs must hold exactly 1 — and no leg may ever hold 2 (twin).
#[derive(Default)]
pub struct TrackedMail {
    message_ids: Vec<String>,
    inbox_uids: Vec<(String, u32)>,
    sent_uids: Vec<(String, u32)>,
    unfiled: Vec<String>,
    /// Sent folder wire, set at preflight — cleanup expunges Sent UIDs via
    /// this wire (UIDs are per-folder; INBOX expunge of a Sent UID would
    /// delete the WRONG mail — T-21-05).
    sent_wire: Option<String>,
    /// Canonical STEP outcomes, in print order (REPORT backbone; cleanup
    /// fails the run when any entry failed).
    pub step_log: Mutex<Vec<LoggedStep>>,
    /// Per-leg transport call deltas (leg, calls_before, calls_after) for
    /// the never-resend proof (AUDIT-CALLS + REPORT).
    pub calls_log: Mutex<Vec<(String, usize, usize)>>,
    /// Autosave-probe end state per Message-ID: (mid, sent_uids, is_unfiled).
    pub probe_counts: Vec<(String, usize, bool)>,
    /// Verbatim probe detail line (REPORT autosave-branch evidence).
    pub probe_detail: String,
    /// Draft leg (D-16): sandbox folder created for the server draft copy
    /// (None when the real-Drafts fallback ran), fallback wire, draft MID,
    /// local drafts row id, and the server copy UID for tracked cleanup.
    pub draft_sandbox: Option<String>,
    pub draft_used_real_drafts: bool,
    pub draft_real_drafts_wire: Option<String>,
    pub draft_mid: Option<String>,
    pub draft_id: Option<String>,
    pub draft_server_uid: Option<u32>,
    /// Credential values for the in-bin secret scan (D-13): cloned from the
    /// keyring/env config at run start, used ONLY as fixed-substring scan
    /// needles over the temp-DB dump — never printed.
    pub scan_secrets: Vec<zeroize::Zeroizing<String>>,
}

impl TrackedMail {
    /// Blank tracker for unit tests driving the outcome log (REPORT backbone).
    pub fn empty() -> Self {
        Self::default()
    }
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
    let mut tracked = TrackedMail::default();
    tracked.scan_secrets = vec![cfg.password.clone(), cfg.smtp_password.clone()];
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(e) => {
            log_step(&tracked, 1, "preflight", false, &format!("temp store: {e}"));
            return 1;
        }
    };

    // 1. preflight — LIST snapshot; resolve + record the Sent wire and the
    // INBOX/Sent UIDVALIDITY epoch for epoch-scoped asserts.
    let before = match list_snapshot(&manager).await {
        Ok(names) => names,
        Err(e) => {
            log_step(&tracked, 1, "preflight", false, &format!("LIST failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before_list_or_empty(), &tracked).await;
        }
    };
    let sent_wire = match manager.list_mailboxes().await {
        Ok(infos) => resolve_sent_wire(&infos),
        Err(e) => {
            log_step(&tracked, 1, "preflight", false, &format!("LIST for Sent resolve failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    let sent_wire = match sent_wire {
        Some(w) => w,
        None => {
            log_step(&tracked, 1, "preflight", false, "no Sent folder resolved (need Sent for D-06)");
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    tracked.sent_wire = Some(sent_wire.clone());
    let inbox_validity = match current_validity(&manager, "INBOX").await {
        Ok(v) => v,
        Err(e) => {
            log_step(&tracked, 1, "preflight", false, &format!("INBOX UIDVALIDITY read failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    let sent_validity = match current_validity(&manager, &sent_wire).await {
        Ok(v) => v,
        Err(e) => {
            log_step(&tracked, 1, "preflight", false, &format!("Sent UIDVALIDITY read failed: {e}"));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    if !log_step(&tracked,
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
            if !log_step(&tracked, 2, "recipient-guard", true, &format!("self-only envelope to=<{}>", cfg.self_addr)) {
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
        Err(e) => {
            log_step(&tracked, 2, "recipient-guard", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }

    // Middle legs: 3 arrival, 4 sent-copy, 5 no-duplicate, 6 sent-unfiled,
    // 7 offline-flush, 8 uncertain-reconcile, 9 crash-recover, 10 draft-send,
    // 11 autosave-probe. Step numbers stay stable so logs stay comparable.
    // 5 no-duplicate, 6 sent-unfiled (Task 3). Step numbers stay stable so
    // logs stay comparable across tasks.
    let list_guard = format!("{}never-created-{}", SANDBOX_PREFIX, tag);

    // 3. arrival — enqueue the self-send, flush through the production path
    // (enqueue_send -> flush_send_queue, the commands/sync.rs queue_send
    // order), prove delivery via INBOX SEARCH (D-05 — never SMTP 250 alone).
    let arrival_subject = format!("{}{}-arrival", SUBJECT_PREFIX, tag);
    let arrival_mid = {
        let guard = store.lock().unwrap();
        match send_queue::enqueue_send(
            guard.conn(),
            &app_data,
            EnqueueInput {
                from: cfg.self_addr.clone(),
                to: vec![cfg.self_addr.clone()],
                cc: vec![],
                bcc: vec![],
                subject: arrival_subject.clone(),
                body: "live gate arrival probe".to_string(),
                draft_id: None,
            },
        ) {
            Ok(out) => {
                if out.deduped {
                    log_step(&tracked, 3, "arrival", false, "fresh run_tag unexpectedly deduped");
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
                out.message_id
            }
            Err(e) => {
                log_step(&tracked, 3, "arrival", false, &format!("enqueue_send failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    // A-01 (observable-state, existing OutboxBadgeWired + SendStatus — no
    // new UI per 21-UI-SPEC.md): pending visible before any network.
    {
        let guard = store.lock().unwrap();
        match send_queue::send_status_snapshot(guard.conn()) {
            Ok(snap) => {
                if !(snap.pending_count >= 1 && snap.queued >= 1) {
                    log_step(&tracked,
                        3,
                        "arrival",
                        false,
                        &format!(
                            "A-01 precondition failed: pending={} queued={}",
                            snap.pending_count, snap.queued
                        ),
                    );
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            Err(e) => {
                log_step(&tracked, 3, "arrival", false, &format!("send_status snapshot failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Production flush: one pooled SmtpTransporter behind the counting
    // decorator (never a second SMTP client — RESEARCH Pitfall 1), manager
    // leases for the filing leg.
    let worker = SyncWorker::new(store.clone());
    // Run-wide shared call counter (D-07 never-resend proof spans legs).
    let send_calls = Arc::new(AtomicUsize::new(0));
    let transport: Arc<dyn SmtpTransport> = Arc::new(SharedCountingTransport::new(
        Arc::new(SmtpTransporter::new()),
        send_calls.clone(),
    ));
    let flush_env = SendFlushEnv {
        gate: SendGate::default(),
        transport,
        account: smtp_account(&cfg),
        manager: Some(manager.clone()),
        sent_wire: Some(sent_wire.clone()),
    };
    eprintln!("[live-gate] {} (arrival leg)", envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]));
    let summary = worker.flush_send_queue(&flush_env).await;
    eprintln!(
        "[live-gate] arrival flush done: sent={} deferred={} failed={} uncertain={} unfiled={} transport_calls={}",
        summary.sent,
        summary.deferred,
        summary.failed,
        summary.uncertain,
        summary.sent_unfiled,
        send_calls.load(Ordering::SeqCst)
    );
    if summary.sent != 1 {
        log_step(&tracked,
            3,
            "arrival",
            false,
            &format!(
                "flush summary sent={} (want 1) deferred={} failed={} uncertain={}",
                summary.sent, summary.deferred, summary.failed, summary.uncertain
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Bounded poll for server-side delivery latency (3x10s, then FAIL).
    let mut arrival_uid: Option<u32> = None;
    let mut arrival_detail = String::new();
    for round in 0..ARRIVAL_POLL_ROUNDS {
        if round > 0 {
            async_std::task::sleep(std::time::Duration::from_secs(ARRIVAL_POLL_SECS)).await;
        }
        let mut lease = match manager.lease_for("INBOX").await {
            Ok(l) => l,
            Err(e) => {
                arrival_detail = format!("INBOX lease failed: {}", e);
                break;
            }
        };
        match lease.selected_validity() {
            Some(v) if v == inbox_validity => {}
            other => {
                arrival_detail = format!("INBOX epoch change mid-run (want {:?}, got {:?}) — aborting loud", inbox_validity, other);
                break;
            }
        }
        match lease.session().uid_search_header("Message-ID", &arrival_mid).await {
            Ok(hits) => {
                if hits.len() == 1 {
                    arrival_uid = Some(hits[0]);
                    break;
                }
                arrival_detail = format!("round {}: {} INBOX hits (want 1)", round + 1, hits.len());
            }
            Err(e) => {
                arrival_detail = format!("INBOX SEARCH failed: {}", e);
                break;
            }
        }
    }
    let arrival_uid = match arrival_uid {
        Some(u) => u,
        None => {
            log_step(&tracked, 3, "arrival", false, &arrival_detail);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    // PEEK-only header confirm (never BODY[] — RESEARCH Pitfall 3).
    let header_ok = match manager.lease_for("INBOX").await {
        Ok(mut lease) => match lease.session().fetch_body(arrival_uid).await {
            Ok(bytes) => match mail_parser::MessageParser::new().parse(&bytes) {
                Some(m) => {
                    let subj_ok = m.subject().map(|s| s.starts_with(SUBJECT_PREFIX)).unwrap_or(false);
                    let bare = arrival_mid.trim_matches(&['<', '>'][..]);
                    let mid_ok = m.message_id().map(|id| id == bare).unwrap_or(false);
                    subj_ok && mid_ok
                }
                None => false,
            },
            Err(_) => false,
        },
        Err(_) => false,
    };
    if !header_ok {
        log_step(&tracked, 3, "arrival", false, "PEEK header confirm failed (subject prefix / Message-ID)");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // A-02/A-03: clean state after ack (badge hidden, no failed surface).
    let post_ok = {
        let guard = store.lock().unwrap();
        match send_queue::send_status_snapshot(guard.conn()) {
            Ok(snap) => snap.pending_count == 0 && snap.failed == 0 && snap.uncertain == 0,
            Err(_) => false,
        }
    };
    if !post_ok {
        log_step(&tracked, 3, "arrival", false, "A-02/A-03 post-condition failed (pending/failed/uncertain not all 0)");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    tracked.message_ids.push(arrival_mid.clone());
    tracked.inbox_uids.push((arrival_mid.clone(), arrival_uid));
    if !log_step(&tracked, 3, "arrival", true, &format!("1 INBOX UID + headers confirm (mid {})", arrival_mid)) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 3, "arrival-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 4. sent-copy — SEARCH Sent for the same Message-ID: exactly 1 UID +
    // \Seen + verbatim-bytes compare against the queued .eml (D-06).
    let sent_uid = {
        let mut lease = match manager.lease_for(&sent_wire).await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 4, "sent-copy", false, &format!("Sent lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        match lease.selected_validity() {
            Some(v) if v == sent_validity => {}
            other => {
                log_step(&tracked, 4, "sent-copy", false, &format!("Sent epoch change mid-run (want {:?}, got {:?})", sent_validity, other));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
        let hits = match lease.session().uid_search_header("Message-ID", &arrival_mid).await {
            Ok(h) => h,
            Err(e) => {
                log_step(&tracked, 4, "sent-copy", false, &format!("Sent SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        let uid = match assert_exactly_one(&hits, "sent-copy") {
            Ok(u) => u,
            Err(e) => {
                log_step(&tracked, 4, "sent-copy", false, &e);
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        // \Seen via the envelope/flags read path (never a non-PEEK fetch).
        let seen_ok = match lease.session().fetch_envelopes(&uid.to_string()).await {
            Ok(envs) => envs.iter().any(|h| h.uid == uid && flags_contain_seen(&h.flags)),
            Err(_) => false,
        };
        if !seen_ok {
            log_step(&tracked, 4, "sent-copy", false, "\\Seen not set on the Sent copy");
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        let server_bytes = match lease.session().fetch_body(uid).await {
            Ok(b) => b,
            Err(e) => {
                log_step(&tracked, 4, "sent-copy", false, &format!("Sent PEEK fetch failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        let queued_bytes = {
            let guard = store.lock().unwrap();
            match queries::get_send_row_by_message_id(guard.conn(), &arrival_mid) {
                Ok(Some(row)) => std::fs::read(&row.eml_path).unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        if queued_bytes.is_empty() || !verbatim_equal(&server_bytes, &queued_bytes) {
            log_step(&tracked, 4, "sent-copy", false, "Sent copy bytes differ from the queued .eml (not verbatim)");
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        uid
    };
    tracked.sent_uids.push((arrival_mid.clone(), sent_uid));
    if !log_step(&tracked, 4, "sent-copy", true, &format!("1 Sent UID + \\Seen + verbatim bytes (uid {})", sent_uid)) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 4, "sent-copy-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 5. no-duplicate (D-07) — double-enqueue the same Message-ID (via a
    // seeded draft anchor, the DRAFT-03 link), flush, assert single delivery:
    // exactly 1 INBOX UID for the Message-ID.
    let nondup_mid = {
        let guard = store.lock().unwrap();
        let mb = match queries::ensure_mailbox(guard.conn(), "INBOX") {
            Ok(id) => id,
            Err(e) => {
                log_step(&tracked, 5, "no-duplicate", false, &format!("temp mailbox seed failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        let draft_id = format!("live21-{}-nondup", tag);
        let mid = new_message_id(&format!("live21-{}-nondup", tag));
        if let Err(e) = queries::upsert_draft(
            guard.conn(),
            &draft_id,
            mb,
            &mid,
            &format!("{}{}-nondup", SUBJECT_PREFIX, tag),
            "live gate no-duplicate probe",
            &cfg.self_addr,
            "",
            "",
        ) {
            log_step(&tracked, 5, "no-duplicate", false, &format!("temp draft seed failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        let mk_input = |did: Option<String>| EnqueueInput {
            from: cfg.self_addr.clone(),
            to: vec![cfg.self_addr.clone()],
            cc: vec![],
            bcc: vec![],
            subject: format!("{}{}-nondup", SUBJECT_PREFIX, tag),
            body: "live gate no-duplicate probe".to_string(),
            draft_id: did,
        };
        let first = match send_queue::enqueue_send(guard.conn(), &app_data, mk_input(Some(draft_id.clone()))) {
            Ok(o) => o,
            Err(e) => {
                log_step(&tracked, 5, "no-duplicate", false, &format!("first enqueue failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        // Recipient guard re-checked on the resolved second envelope too.
        if let Err(e) = assert_self_only(&cfg.self_addr, &[cfg.self_addr.clone()], &[], &[], &cfg.self_addr) {
            log_step(&tracked, 5, "no-duplicate", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        let second = match send_queue::enqueue_send(guard.conn(), &app_data, mk_input(Some(draft_id))) {
            Ok(o) => o,
            Err(e) => {
                log_step(&tracked, 5, "no-duplicate", false, &format!("second enqueue failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        if !(second.deduped && second.queue_id == first.queue_id && second.message_id == first.message_id) {
            log_step(&tracked,
                5,
                "no-duplicate",
                false,
                &format!(
                    "dedupe failed: second enqueue deduped={} queue_id={} (want {} + deduped)",
                    second.deduped, second.queue_id, first.queue_id
                ),
            );
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        // One row, one .eml for the double-enqueued Message-ID.
        let row_count: i64 = guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM send_queue WHERE message_id = ?1",
                rusqlite::params![first.message_id],
                |r| r.get(0),
            )
            .unwrap_or(-1);
        if row_count != 1 {
            log_step(&tracked, 5, "no-duplicate", false, &format!("{} rows for one Message-ID (want 1)", row_count));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        mid
    };
    let calls_before_nondup = send_calls.load(Ordering::SeqCst);
    eprintln!("[live-gate] {} (no-duplicate leg)", envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]));
    let nondup_summary = worker.flush_send_queue(&flush_env).await;
    eprintln!(
        "[live-gate] no-duplicate flush done: sent={} deferred={} failed={} uncertain={} unfiled={} transport_calls={}",
        nondup_summary.sent,
        nondup_summary.deferred,
        nondup_summary.failed,
        nondup_summary.uncertain,
        nondup_summary.sent_unfiled,
        send_calls.load(Ordering::SeqCst)
    );
    if nondup_summary.sent != 1 {
        log_step(&tracked,
            5,
            "no-duplicate",
            false,
            &format!("flush sent={} (want 1 for the deduped row)", nondup_summary.sent),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Single delivery: exactly 1 INBOX UID for the double-enqueued ID.
    let nondup_uid = match manager.lease_for("INBOX").await {
        Ok(mut lease) => match lease.session().uid_search_header("Message-ID", &nondup_mid).await {
            Ok(hits) => match assert_exactly_one(&hits, "no-duplicate arrival") {
                Ok(u) => u,
                Err(e) => {
                    log_step(&tracked, 5, "no-duplicate", false, &e);
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            },
            Err(e) => {
                log_step(&tracked, 5, "no-duplicate", false, &format!("INBOX SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 5, "no-duplicate", false, &format!("INBOX lease failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    // ...and exactly 1 filed Sent copy.
    let nondup_sent_uid = match manager.lease_for(&sent_wire).await {
        Ok(mut lease) => match lease.session().uid_search_header("Message-ID", &nondup_mid).await {
            Ok(hits) => match assert_exactly_one(&hits, "no-duplicate sent-copy") {
                Ok(u) => u,
                Err(e) => {
                    log_step(&tracked, 5, "no-duplicate", false, &e);
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            },
            Err(e) => {
                log_step(&tracked, 5, "no-duplicate", false, &format!("Sent SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 5, "no-duplicate", false, &format!("Sent lease failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    tracked.message_ids.push(nondup_mid.clone());
    tracked.inbox_uids.push((nondup_mid.clone(), nondup_uid));
    tracked.sent_uids.push((nondup_mid.clone(), nondup_sent_uid));
    if !log_step(&tracked,
        5,
        "no-duplicate",
        true,
        &format!(
            "double-enqueue deduped, 1 INBOX UID + 1 Sent UID (transport_calls {} -> {})",
            calls_before_nondup,
            send_calls.load(Ordering::SeqCst)
        ),
    ) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 5, "no-duplicate-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 6. sent-unfiled (D-07) — SMTP succeeds but the Sent APPEND cannot land:
    // drive the production APPEND-failure path with an unresolvable Sent
    // wire (lease fails → SentUnfiled, APPEND-only retry, never re-SMTP-send
    // per worker.rs filing leg). Assert the outcome + exactly one transport
    // call + row parked sent (never failed); a second flush must add zero
    // transport calls (reconcile-not-resend encoded as call-count assert).
    let unfiled_mid = {
        let guard = store.lock().unwrap();
        match send_queue::enqueue_send(
            guard.conn(),
            &app_data,
            EnqueueInput {
                from: cfg.self_addr.clone(),
                to: vec![cfg.self_addr.clone()],
                cc: vec![],
                bcc: vec![],
                subject: format!("{}{}-unfiled", SUBJECT_PREFIX, tag),
                body: "live gate sent-unfiled probe".to_string(),
                draft_id: None,
            },
        ) {
            Ok(out) => out.message_id,
            Err(e) => {
                log_step(&tracked, 6, "sent-unfiled", false, &format!("enqueue_send failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    let bogus_sent_wire = format!("SGE-NoSuchFolder-{}", tag);
    // Shared run-wide call counter: the never-resend proof spans legs.
    let unfiled_transport: Arc<dyn SmtpTransport> = Arc::new(SharedCountingTransport::new(
        Arc::new(SmtpTransporter::new()),
        send_calls.clone(),
    ));
    let unfiled_env = SendFlushEnv {
        gate: SendGate::default(),
        transport: unfiled_transport,
        account: smtp_account(&cfg),
        manager: Some(manager.clone()),
        sent_wire: Some(bogus_sent_wire.clone()),
    };
    let calls_before_unfiled = send_calls.load(Ordering::SeqCst);
    eprintln!(
        "[live-gate] {} (sent-unfiled leg, sent_wire={})",
        envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]),
        bogus_sent_wire
    );
    let unfiled_summary = worker.flush_send_queue(&unfiled_env).await;
    let calls_after_unfiled = send_calls.load(Ordering::SeqCst);
    eprintln!(
        "[live-gate] sent-unfiled flush done: sent={} unfiled={} failed={} uncertain={} transport_calls={}",
        unfiled_summary.sent,
        unfiled_summary.sent_unfiled,
        unfiled_summary.failed,
        unfiled_summary.uncertain,
        calls_after_unfiled
    );
    if unfiled_summary.sent_unfiled != 1 {
        log_step(&tracked,
            6,
            "sent-unfiled",
            false,
            &format!("flush sent_unfiled={} (want 1 via bogus Sent wire)", unfiled_summary.sent_unfiled),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if calls_after_unfiled != calls_before_unfiled + 1 {
        log_step(&tracked,
            6,
            "sent-unfiled",
            false,
            &format!("transport calls {} -> {} (want exactly +1 SMTP send)", calls_before_unfiled, calls_after_unfiled),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Row parked sent (never failed).
    let parked_sent = {
        let guard = store.lock().unwrap();
        match queries::get_send_row_by_message_id(guard.conn(), &unfiled_mid) {
            Ok(Some(row)) => row.state == queries::SEND_STATE_SENT,
            _ => false,
        }
    };
    if !parked_sent {
        log_step(&tracked, 6, "sent-unfiled", false, "row not parked sent after SentUnfiled");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Never-resend: a second flush adds zero transport calls (parked sent
    // rows are not due — reconcile-not-resend, no second send_raw).
    let reflush_summary = worker.flush_send_queue(&unfiled_env).await;
    let calls_after_reflush = send_calls.load(Ordering::SeqCst);
    if reflush_summary != FlushSummary::default() {
        log_step(&tracked,
            6,
            "sent-unfiled",
            false,
            &format!(
                "re-flush summary not empty ({:?}) — parked rows must never be due",
                reflush_summary
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if calls_after_reflush != calls_after_unfiled {
        log_step(&tracked,
            6,
            "sent-unfiled",
            false,
            &format!("re-flush re-sent (calls {} -> {}) — SentUnfiled must never re-SMTP-send", calls_after_unfiled, calls_after_reflush),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // The mail WAS delivered (SMTP succeeded) — track the INBOX arrival for
    // cleanup. Sent side: 0 (no autosave) or 1 (server autosaved) — the
    // autosave probe (step 7) reports which.
    let unfiled_inbox_uid = match manager.lease_for("INBOX").await {
        Ok(mut lease) => match lease.session().uid_search_header("Message-ID", &unfiled_mid).await {
            Ok(hits) => match assert_exactly_one(&hits, "sent-unfiled arrival") {
                Ok(u) => u,
                Err(e) => {
                    log_step(&tracked, 6, "sent-unfiled", false, &e);
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            },
            Err(e) => {
                log_step(&tracked, 6, "sent-unfiled", false, &format!("INBOX SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 6, "sent-unfiled", false, &format!("INBOX lease failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    tracked.message_ids.push(unfiled_mid.clone());
    tracked.inbox_uids.push((unfiled_mid.clone(), unfiled_inbox_uid));
    tracked.unfiled.push(unfiled_mid.clone());
    // A-07 observability gap (RESEARCH Pitfall 5, plan-recorded):
    // send_status_snapshot().sent_unfiled hard-codes 0 — A-07 is asserted
    // via FlushOutcome::SentUnfiled counts + transport call counts, never via
    // the snapshot field.
    if !log_step(&tracked,
        6,
        "sent-unfiled",
        true,
        &format!(
            "SentUnfiled outcome, 1 SMTP send, row parked sent, re-flush +0 calls (A-07 via outcomes+call-counts; snapshot.sent_unfiled hard-codes 0 — recorded gap)"
        ),
    ) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 6, "sent-unfiled-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 7. offline-flush (D-09) — enqueue under the closed app-level gate,
    // prove the durable queue holds (A-01: OutboxBadgeWired
    // "{N} mensagem(s) pendentes" + SendStatus
    // "{N} alteração(ões) aguardando envio"), reopen, flush for real with the
    // backoff schedule visible, arrive exactly once (A-02/A-03).
    let offline_mid = {
        // Close the IMAP-side gate FIRST and prove it fails closed with a
        // zero-effect write attempt (the decorator errors BEFORE touching
        // the server — no UID on the box is at risk).
        let gate_flag = Arc::new(AtomicBool::new(true));
        {
            let mut lease = match manager.lease_for("INBOX").await {
                Ok(l) => l,
                Err(e) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("INBOX lease failed: {}", e));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            };
            let mut gated = GateSession::new(lease.session(), gate_flag.clone());
            match gated.store_deleted(u32::MAX, true).await {
                Err(e) if e.to_string().contains("offline gate closed") => {}
                Err(e) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("gate-closed write failed unexpectedly: {}", e));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
                Ok(()) => {
                    log_step(&tracked, 7, "offline-flush", false, "gate-closed write SUCCEEDED — gate is not failing closed");
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
        }
        // Enqueue locally while "offline" (the durable queue needs no
        // network — D-09). Recipient guard first, like every leg.
        if let Err(e) = assert_self_only(&cfg.self_addr, &[cfg.self_addr.clone()], &[], &[], &cfg.self_addr) {
            log_step(&tracked, 7, "offline-flush", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        let mid = {
            let guard = store.lock().unwrap();
            match send_queue::enqueue_send(
                guard.conn(),
                &app_data,
                EnqueueInput {
                    from: cfg.self_addr.clone(),
                    to: vec![cfg.self_addr.clone()],
                    cc: vec![],
                    bcc: vec![],
                    subject: format!("{}{}-offline", SUBJECT_PREFIX, tag),
                    body: "live gate offline-flush probe".to_string(),
                    draft_id: None,
                },
            ) {
                Ok(out) => out.message_id,
                Err(e) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("offline enqueue_send failed: {}", e));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
        };
        // A-01 observable-state precondition: pending visible before any
        // network (existing OutboxBadgeWired + SendStatus copy, no new UI).
        {
            let guard = store.lock().unwrap();
            match send_queue::send_status_snapshot(guard.conn()) {
                Ok(snap) => {
                    if !(snap.pending_count >= 1 && snap.queued >= 1) {
                        log_step(
                            &tracked,
                            7,
                            "offline-flush",
                            false,
                            &format!(
                                "A-01 precondition failed: pending={} queued={}",
                                snap.pending_count, snap.queued
                            ),
                        );
                        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                    }
                }
                Err(e) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("send_status snapshot failed: {}", e));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
        }
        // Reopen the gate; prove the pre-send baseline (no leftover MID).
        gate_flag.store(false, Ordering::SeqCst);
        {
            let mut lease = match manager.lease_for("INBOX").await {
                Ok(l) => l,
                Err(e) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("INBOX lease failed: {}", e));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            };
            match lease.session().uid_search_header("Message-ID", &mid).await {
                Ok(hits) if hits.is_empty() => {}
                Ok(hits) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("pre-send INBOX baseline not empty: {} hit(s) — prior-run leftover, aborting loud", hits.len()));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
                Err(e) => {
                    log_step(&tracked, 7, "offline-flush", false, &format!("pre-send INBOX SEARCH failed: {}", e));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
        }
        mid
    };
    let calls_before_offline = send_calls.load(Ordering::SeqCst);
    eprintln!("[live-gate] {} (offline-flush leg, gate reopened)", envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]));
    let offline_summary = worker.flush_send_queue(&flush_env).await;
    let calls_after_offline = send_calls.load(Ordering::SeqCst);
    record_calls(&tracked, "offline-flush", calls_before_offline, calls_after_offline);
    eprintln!(
        "[live-gate] offline flush done: sent={} deferred={} failed={} uncertain={} unfiled={} transport_calls={}",
        offline_summary.sent,
        offline_summary.deferred,
        offline_summary.failed,
        offline_summary.uncertain,
        offline_summary.sent_unfiled,
        calls_after_offline
    );
    if offline_summary.sent != 1 {
        log_step(
            &tracked,
            7,
            "offline-flush",
            false,
            &format!(
                "flush summary sent={} (want 1) deferred={} failed={} uncertain={}",
                offline_summary.sent, offline_summary.deferred, offline_summary.failed, offline_summary.uncertain
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if calls_after_offline != calls_before_offline + 1 {
        log_step(
            &tracked,
            7,
            "offline-flush",
            false,
            &format!("transport calls {} -> {} (want exactly +1 SMTP send)", calls_before_offline, calls_after_offline),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Backoff visible (D-09): any deferred row's next_retry_at must sit
    // inside backoff_bounds(attempts). A clean first-try send defers nothing
    // (vacuous pass recorded); the bounds VALUES are pinned offline.
    {
        let (ok, detail) = {
            let guard = store.lock().unwrap();
            let mut rows: Vec<(String, i64, Option<String>)> = Vec::new();
            if let Ok(mut stmt) = guard.conn().prepare(
                "SELECT id, attempts, next_retry_at FROM send_queue WHERE attempts > 0",
            ) {
                if let Ok(mapped) = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                }) {
                    rows = mapped.filter_map(|r| r.ok()).collect();
                }
            }
            deferred_backoff_ok(&rows, chrono::Utc::now().timestamp())
        };
        if !ok {
            log_step(&tracked, 7, "offline-flush", false, &format!("backoff schedule violated: {}", detail));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        eprintln!("[live-gate] offline backoff check: {}", detail);
    }
    // Arrival exactly once + PEEK confirm + clean state (A-02/A-03).
    let offline_uid = match poll_inbox_for_mid(&manager, &offline_mid, inbox_validity).await {
        Ok(u) => u,
        Err(d) => {
            log_step(&tracked, 7, "offline-flush", false, &d);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    if !peek_confirms_mid(&manager, "INBOX", offline_uid, &offline_mid).await {
        log_step(&tracked, 7, "offline-flush", false, "PEEK header confirm failed (subject prefix / Message-ID)");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    {
        let guard = store.lock().unwrap();
        match send_queue::send_status_snapshot(guard.conn()) {
            Ok(snap) => {
                if !(snap.pending_count == 0 && snap.failed == 0 && snap.uncertain == 0) {
                    log_step(&tracked, 7, "offline-flush", false, "A-02/A-03 post-condition failed (pending/failed/uncertain not all 0)");
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            Err(e) => {
                log_step(&tracked, 7, "offline-flush", false, &format!("send_status snapshot failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Sent-side filing for the offline leg (same verbatim-copy bar as D-06).
    let offline_sent_uid = match search_sent_for_mid(&manager, &sent_wire, &offline_mid).await {
        Ok(hits) => match assert_exactly_one(&hits, "offline-flush sent-copy") {
            Ok(u) => u,
            Err(e) => {
                log_step(&tracked, 7, "offline-flush", false, &e);
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 7, "offline-flush", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    tracked.message_ids.push(offline_mid.clone());
    tracked.inbox_uids.push((offline_mid.clone(), offline_uid));
    tracked.sent_uids.push((offline_mid.clone(), offline_sent_uid));
    if !log_step(
        &tracked,
        7,
        "offline-flush",
        true,
        &format!("gate-closed enqueue held (A-01), reopen flush sent once (+1 call), 1 INBOX UID + 1 Sent UID, backoff visible, clean state (A-02/A-03/A-08)"),
    ) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 7, "offline-flush-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 8. uncertain-reconcile (D-10) — harness-injected DATA-timeout fault →
    // `uncertain` hold (A-04: SendStatus uncertain hold line, no new send
    // issued) → simulated ambiguous server-accept → reconcile-not-resend
    // (SEARCH hit confirms with ZERO new transport calls).
    let uncertain_mid = {
        if let Err(e) = assert_self_only(&cfg.self_addr, &[cfg.self_addr.clone()], &[], &[], &cfg.self_addr) {
            log_step(&tracked, 8, "uncertain-reconcile", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        let guard = store.lock().unwrap();
        match send_queue::enqueue_send(
            guard.conn(),
            &app_data,
            EnqueueInput {
                from: cfg.self_addr.clone(),
                to: vec![cfg.self_addr.clone()],
                cc: vec![],
                bcc: vec![],
                subject: format!("{}{}-uncertain", SUBJECT_PREFIX, tag),
                body: "live gate uncertain-reconcile probe".to_string(),
                draft_id: None,
            },
        ) {
            Ok(out) => out.message_id,
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("enqueue_send failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    // Fault rig: TimeoutTransport over a shared-counting inner, so real
    // sends still count into the run-wide never-resend counter while the
    // faulted attempt counts nowhere.
    let (timeout_transport, fault_armed, fault_inner_calls) = TimeoutTransport::new(Arc::new(
        SharedCountingTransport::new(Arc::new(SmtpTransporter::new()), send_calls.clone()),
    ));
    fault_armed.store(true, Ordering::SeqCst);
    let fault_env = SendFlushEnv {
        gate: SendGate::default(),
        transport: Arc::new(timeout_transport),
        account: smtp_account(&cfg),
        manager: Some(manager.clone()),
        sent_wire: Some(sent_wire.clone()),
    };
    let calls_before_fault = send_calls.load(Ordering::SeqCst);
    let inner_before_fault = fault_inner_calls.load(Ordering::SeqCst);
    eprintln!("[live-gate] {} (uncertain leg, DATA-timeout fault ARMED)", envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]));
    let fault_summary = worker.flush_send_queue(&fault_env).await;
    let calls_after_fault = send_calls.load(Ordering::SeqCst);
    let inner_after_fault = fault_inner_calls.load(Ordering::SeqCst);
    record_calls(&tracked, "uncertain-armed-flush", calls_before_fault, calls_after_fault);
    eprintln!(
        "[live-gate] uncertain flush done: sent={} deferred={} failed={} uncertain={} unfiled={} transport_calls={}",
        fault_summary.sent,
        fault_summary.deferred,
        fault_summary.failed,
        fault_summary.uncertain,
        fault_summary.sent_unfiled,
        calls_after_fault
    );
    if fault_summary.uncertain != 1 || fault_summary.sent != 0 {
        log_step(
            &tracked,
            8,
            "uncertain-reconcile",
            false,
            &format!(
                "armed flush sent={} uncertain={} (want sent=0 uncertain=1)",
                fault_summary.sent, fault_summary.uncertain
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if calls_after_fault != calls_before_fault || inner_after_fault != inner_before_fault {
        log_step(
            &tracked,
            8,
            "uncertain-reconcile",
            false,
            &format!(
                "faulted attempt left the box (run-wide {} -> {}, inner {} -> {}) — zero SMTP bytes required",
                calls_before_fault, calls_after_fault, inner_before_fault, inner_after_fault
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // A-04 + A-06: uncertain hold visible, and uncertain never renders as
    // failed (SendStatus shows the "Reconciliando na pasta Enviadas" hold
    // line, never the failed surface + Retentar for this row).
    {
        let guard = store.lock().unwrap();
        match send_queue::send_status_snapshot(guard.conn()) {
            Ok(snap) => {
                if !(snap.uncertain >= 1 && snap.failed == 0) {
                    log_step(
                        &tracked,
                        8,
                        "uncertain-reconcile",
                        false,
                        &format!("A-04/A-06 violated: uncertain={} failed={} (want uncertain>=1 failed=0)", snap.uncertain, snap.failed),
                    );
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("send_status snapshot failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Negative control: zero SMTP bytes means zero INBOX hits for this MID.
    {
        let mut lease = match manager.lease_for("INBOX").await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("INBOX lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        match lease.session().uid_search_header("Message-ID", &uncertain_mid).await {
            Ok(hits) if hits.is_empty() => {}
            Ok(hits) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("fault leaked bytes: {} INBOX hit(s) for an unsent MID", hits.len()));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("INBOX SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Simulated ambiguous server-accept: a real DATA-timeout may land
    // server-side while the client sees the timeout. Mirror the filing leg's
    // APPEND (verbatim queued bytes + \Seen) through the preflight-resolved
    // Sent wire — harness-issued, MID-tracked, explicitly allowed (never
    // INBOX: arrival lands by delivery only).
    let uncertain_eml = {
        let guard = store.lock().unwrap();
        match queries::get_send_row_by_message_id(guard.conn(), &uncertain_mid) {
            Ok(Some(row)) => std::fs::read(&row.eml_path).unwrap_or_default(),
            _ => Vec::new(),
        }
    };
    if uncertain_eml.is_empty() {
        log_step(&tracked, 8, "uncertain-reconcile", false, "queued .eml unreadable for the simulated accept");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if let Err(e) = assert_harness_append_target(&sent_wire, &[sent_wire.clone()]) {
        log_step(&tracked, 8, "uncertain-reconcile", false, &format!("append-target guard refused the Sent wire: {}", e));
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    {
        let mut lease = match manager.lease_for(&sent_wire).await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("Sent lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        if let Err(e) = lease.session().append_message(&sent_wire, "\\Seen", &uncertain_eml).await {
            log_step(&tracked, 8, "uncertain-reconcile", false, &format!("simulated-accept APPEND failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }
    fault_armed.store(false, Ordering::SeqCst);
    // Reconcile: SEARCH-before-resend. The hit must confirm with the
    // transport call counts UNCHANGED (D-10: reconcile-not-resend).
    let rec_before = send_calls.load(Ordering::SeqCst);
    let rec_inner_before = fault_inner_calls.load(Ordering::SeqCst);
    let reconcile_summary = {
        let mut lease = match manager.lease_for("INBOX").await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("INBOX lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        match worker
            .reconcile_uncertain_sends(lease.session(), Some(sent_wire.as_str()), "INBOX")
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &format!("reconcile failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    let rec_after = send_calls.load(Ordering::SeqCst);
    let rec_inner_after = fault_inner_calls.load(Ordering::SeqCst);
    record_calls(&tracked, "uncertain-reconcile", rec_before, rec_after);
    eprintln!(
        "[live-gate] uncertain reconcile done: confirmed_sent={} requeued={} still_uncertain={} transport_calls={}",
        reconcile_summary.confirmed_sent,
        reconcile_summary.requeued,
        reconcile_summary.still_uncertain,
        rec_after
    );
    if reconcile_summary.confirmed_sent != 1 || reconcile_summary.requeued != 0 {
        log_step(
            &tracked,
            8,
            "uncertain-reconcile",
            false,
            &format!(
                "reconcile confirmed_sent={} requeued={} (want 1/0 on the SEARCH hit)",
                reconcile_summary.confirmed_sent, reconcile_summary.requeued
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if rec_after != rec_before || rec_inner_after != rec_inner_before {
        log_step(
            &tracked,
            8,
            "uncertain-reconcile",
            false,
            &format!("reconcile re-sent (run-wide {} -> {}, inner {} -> {}) — SEARCH hit must confirm without a transport call", rec_before, rec_after, rec_inner_before, rec_inner_after),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // End state: exactly 1 Sent UID (the simulated accept), row parked sent.
    let uncertain_sent_uid = match search_sent_for_mid(&manager, &sent_wire, &uncertain_mid).await {
        Ok(hits) => match assert_exactly_one(&hits, "uncertain-reconcile sent-copy") {
            Ok(u) => u,
            Err(e) => {
                log_step(&tracked, 8, "uncertain-reconcile", false, &e);
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 8, "uncertain-reconcile", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    tracked.message_ids.push(uncertain_mid.clone());
    tracked.sent_uids.push((uncertain_mid.clone(), uncertain_sent_uid));
    if !log_step(
        &tracked,
        8,
        "uncertain-reconcile",
        true,
        &format!("armed flush uncertain=1 (+0 calls, INBOX 0 hits); simulated server-accept APPENDed to Sent (uid {}); reconcile confirmed_sent=1 with transport calls unchanged (reconcile-not-resend); A-04 hold line observed, failed=0 (A-06)", uncertain_sent_uid),
    ) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 8, "uncertain-reconcile-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 9. crash-recover (D-11) — kill mid-`sending` → restart → flush-pass
    // uncertain triage (A-05, superseding the UI-SPEC `queued` wording per
    // RESEARCH Pitfall 4) → reconcile-miss requeue (no blind resend) →
    // backoff-waited single send: exactly 1 INBOX UID + 1 Sent UID,
    // failed == 0 (pending, never failed).
    let (crash_mid, crash_qid) = {
        if let Err(e) = assert_self_only(&cfg.self_addr, &[cfg.self_addr.clone()], &[], &[], &cfg.self_addr) {
            log_step(&tracked, 9, "crash-recover", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        let guard = store.lock().unwrap();
        match send_queue::enqueue_send(
            guard.conn(),
            &app_data,
            EnqueueInput {
                from: cfg.self_addr.clone(),
                to: vec![cfg.self_addr.clone()],
                cc: vec![],
                bcc: vec![],
                subject: format!("{}{}-crash", SUBJECT_PREFIX, tag),
                body: "live gate crash-recover probe".to_string(),
                draft_id: None,
            },
        ) {
            Ok(out) => (out.message_id, out.queue_id),
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("enqueue_send failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    // Kill #1: claim the row to `sending` (the exact first write
    // flush_one_row performs), then drop all state without a verdict —
    // the kill lands between claim and verdict.
    {
        let guard = store.lock().unwrap();
        if let Err(e) = queries::set_send_state(guard.conn(), &crash_qid, queries::SEND_STATE_SENDING) {
            log_step(&tracked, 9, "crash-recover", false, &format!("claim to sending failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }
    // Restart #1: launch-discipline crash_recover flips stranded rows back
    // to `queued` — assert the D-11 reset ran (returns the flipped count).
    {
        let guard = store.lock().unwrap();
        match send_queue::crash_recover(guard.conn()) {
            Ok(1) => {}
            Ok(n) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("crash_recover flipped {} rows (want 1)", n));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("crash_recover failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
        match queries::get_send_row(guard.conn(), &crash_qid) {
            Ok(Some(row)) if row.state == queries::SEND_STATE_QUEUED => {}
            Ok(Some(row)) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("post-crash_recover state={} (want queued)", row.state));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
            _ => {
                log_step(&tracked, 9, "crash-recover", false, "crash row vanished after crash_recover");
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Kill #2: strand the row in the claim window again, then restart
    // STRAIGHT into a flush pass — the pass header triage (crash_recover +
    // promote stranded `sending` to `uncertain`) IS the production recovery
    // path, so no explicit crash_recover runs here (it would empty the
    // stranded set the triage must observe).
    {
        let guard = store.lock().unwrap();
        if let Err(e) = queries::set_send_state(guard.conn(), &crash_qid, queries::SEND_STATE_SENDING) {
            log_step(&tracked, 9, "crash-recover", false, &format!("re-claim to sending failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }
    let calls_before_crash = send_calls.load(Ordering::SeqCst);
    eprintln!("[live-gate] {} (crash-recover leg, restart into flush pass)", envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]));
    let crash_summary = worker.flush_send_queue(&flush_env).await;
    let calls_after_crash = send_calls.load(Ordering::SeqCst);
    record_calls(&tracked, "crash-triage-flush", calls_before_crash, calls_after_crash);
    eprintln!(
        "[live-gate] crash triage flush done: sent={} deferred={} failed={} uncertain={} unfiled={} transport_calls={}",
        crash_summary.sent,
        crash_summary.deferred,
        crash_summary.failed,
        crash_summary.uncertain,
        crash_summary.sent_unfiled,
        calls_after_crash
    );
    if crash_summary.sent != 0 || calls_after_crash != calls_before_crash {
        log_step(
            &tracked,
            9,
            "crash-recover",
            false,
            &format!("recovery pass sent={} calls {} -> {} — triage must send nothing (no blind resend)", crash_summary.sent, calls_before_crash, calls_after_crash),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Post-restart state MUST be `uncertain` (flush-pass triage), never
    // `queued` (RESEARCH Pitfall 4) and never `failed` (A-05/A-06).
    {
        let guard = store.lock().unwrap();
        match send_queue::send_status_snapshot(guard.conn()) {
            Ok(snap) => {
                if !(snap.uncertain >= 1 && snap.failed == 0) {
                    log_step(
                        &tracked,
                        9,
                        "crash-recover",
                        false,
                        &format!("post-restart state uncertain={} failed={} (want uncertain>=1 failed=0 — triage, not queued/failed)", snap.uncertain, snap.failed),
                    );
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("send_status snapshot failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
        match queries::get_send_row(guard.conn(), &crash_qid) {
            Ok(Some(row)) if row.state == queries::SEND_STATE_UNCERTAIN => {}
            Ok(Some(row)) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("crash row state={} (want uncertain — Pitfall 4)", row.state));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
            _ => {
                log_step(&tracked, 9, "crash-recover", false, "crash row vanished after the triage pass");
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Reconcile: SEARCH Sent MISSES (zero bytes ever left the box) → exactly
    // one requeue with attempts counted and backoff scheduled (no blind
    // resend — the miss path still SEARCHes first).
    let crash_rec_before = send_calls.load(Ordering::SeqCst);
    let crash_reconcile = {
        let mut lease = match manager.lease_for("INBOX").await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("INBOX lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        match worker
            .reconcile_uncertain_sends(lease.session(), Some(sent_wire.as_str()), "INBOX")
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("reconcile failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    let crash_rec_after = send_calls.load(Ordering::SeqCst);
    record_calls(&tracked, "crash-reconcile", crash_rec_before, crash_rec_after);
    if crash_reconcile.requeued != 1 || crash_reconcile.confirmed_sent != 0 {
        log_step(
            &tracked,
            9,
            "crash-recover",
            false,
            &format!(
                "reconcile requeued={} confirmed_sent={} (want 1/0 — SEARCH miss on zero sent bytes)",
                crash_reconcile.requeued, crash_reconcile.confirmed_sent
            ),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if crash_rec_after != crash_rec_before {
        log_step(
            &tracked,
            9,
            "crash-recover",
            false,
            &format!("reconcile re-sent (calls {} -> {}) — miss path must requeue, never resend", crash_rec_before, crash_rec_after),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // The requeue carries attempts=1 with a scheduled backoff — prove the
    // schedule sits inside the bounds (D-09, second sighting).
    {
        let guard = store.lock().unwrap();
        match queries::get_send_row(guard.conn(), &crash_qid) {
            Ok(Some(row)) => {
                let now = chrono::Utc::now().timestamp();
                if row.attempts != 1 || !retry_within_bounds(&row.next_retry_at, 1, now, 15) {
                    log_step(
                        &tracked,
                        9,
                        "crash-recover",
                        false,
                        &format!("requeue schedule off: attempts={} next_retry_at={:?} (want attempts=1 inside backoff_bounds(1))", row.attempts, row.next_retry_at),
                    );
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            _ => {
                log_step(&tracked, 9, "crash-recover", false, "crash row vanished after reconcile");
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Backoff-waited single send: poll-flush until the row is due (≤10 ×
    // 15s — production due-gating decides, never a forced send). Non-winning
    // polls must add zero calls; the winning pass adds exactly one.
    let mut crash_sent = false;
    for round in 0..10usize {
        if round > 0 {
            async_std::task::sleep(std::time::Duration::from_secs(15)).await;
        }
        let b = send_calls.load(Ordering::SeqCst);
        let s = worker.flush_send_queue(&flush_env).await;
        let a = send_calls.load(Ordering::SeqCst);
        eprintln!(
            "[live-gate] crash backoff poll {}/10: sent={} deferred={} failed={} uncertain={} transport_calls={}",
            round + 1,
            s.sent,
            s.deferred,
            s.failed,
            s.uncertain,
            a
        );
        if s.sent == 1 {
            record_calls(&tracked, "crash-final-flush", b, a);
            crash_sent = true;
            break;
        }
        record_calls(&tracked, "crash-backoff-poll", b, a);
        if a != b || s.sent != 0 {
            log_step(
                &tracked,
                9,
                "crash-recover",
                false,
                &format!("backoff poll {} sent without due row (sent={} calls {} -> {})", round + 1, s.sent, b, a),
            );
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }
    if !crash_sent {
        log_step(&tracked, 9, "crash-recover", false, "requeued row never became due within 10 × 15s polls");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Exactly-once: 1 INBOX UID + 1 Sent UID, failed == 0, leg calls +1.
    let crash_uid = match poll_inbox_for_mid(&manager, &crash_mid, inbox_validity).await {
        Ok(u) => u,
        Err(d) => {
            log_step(&tracked, 9, "crash-recover", false, &d);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    if !peek_confirms_mid(&manager, "INBOX", crash_uid, &crash_mid).await {
        log_step(&tracked, 9, "crash-recover", false, "PEEK header confirm failed (subject prefix / Message-ID)");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    let crash_sent_uid = match search_sent_for_mid(&manager, &sent_wire, &crash_mid).await {
        Ok(hits) => match assert_exactly_one(&hits, "crash-recover sent-copy") {
            Ok(u) => u,
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &e);
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 9, "crash-recover", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    {
        let guard = store.lock().unwrap();
        match send_queue::send_status_snapshot(guard.conn()) {
            Ok(snap) => {
                if snap.failed != 0 {
                    log_step(&tracked, 9, "crash-recover", false, &format!("failed={} (want 0 — crash-recovered item shows pending, never failed)", snap.failed));
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            Err(e) => {
                log_step(&tracked, 9, "crash-recover", false, &format!("send_status snapshot failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    tracked.message_ids.push(crash_mid.clone());
    tracked.inbox_uids.push((crash_mid.clone(), crash_uid));
    tracked.sent_uids.push((crash_mid.clone(), crash_sent_uid));
    if !log_step(
        &tracked,
        9,
        "crash-recover",
        true,
        &format!("kill→crash_recover=1 (queued reset)→re-strand→flush triage uncertain (A-05 post-flush-triage, failed=0); reconcile miss→requeue (no blind resend, backoff in bounds); backoff-waited flush sent once: 1 INBOX UID + 1 Sent UID, leg calls +1 (exactly once)"),
    ) {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 9, "crash-recover-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 10. draft-send (D-16, BINARY verdict) — sandbox draft → production
    // send → no-orphan assert. PASS ONLY if the flush itself removed the
    // local drafts row AND the server copy with zero harness-side discard
    // (same-transaction delete per D-16). Any orphan → FAIL BY DESIGN — the
    // honest outcome (RESEARCH A6 re-verified at execution time: no
    // draft-delete call exists in flush_one_row/flush_send_queue, only the
    // draft_id Message-ID-reuse arm in enqueue_send) routing to gap closure.
    // This verification-only phase implements no product fix and performs no
    // assisted discard on this path.
    let draft_folder = format!("{}{}-drafts", SANDBOX_PREFIX, tag);
    if let Err(e) = assert_sandbox_prefix(&draft_folder) {
        log_step(&tracked, 10, "draft-send", false, &format!("sandbox guard refused the draft folder: {}", e));
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    // Sandbox APPEND preferred (D-16 researcher flag): attempt account-root
    // CREATE for the draft sandbox first.
    let sandbox_created = {
        let mut lease = match manager.lease_for("INBOX").await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("INBOX lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        lease.session().create_mailbox(&draft_folder).await.is_ok()
    };
    // Real-Drafts fallback with tracked-UID cleanup when the server refuses
    // root CREATE — recorded verbatim in the STEP detail line.
    let mut fallback_note = String::new();
    let draft_wire = if sandbox_created {
        draft_folder.clone()
    } else {
        let infos = match manager.list_mailboxes().await {
            Ok(i) => i,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("LIST for Drafts fallback failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        let wire = infos
            .iter()
            .find(|i| i.name == "Drafts")
            .or_else(|| infos.iter().find(|i| i.name.eq_ignore_ascii_case("drafts")))
            .or_else(|| infos.iter().find(|i| i.name.eq_ignore_ascii_case("rascunhos")))
            .map(|i| i.name.clone());
        match wire {
            Some(w) => {
                fallback_note = format!("fallback: real Drafts {:?} (server refused root CREATE for {:?}; tracked-UID cleanup only, never DELETE)", w, draft_folder);
                w
            }
            None => {
                log_step(&tracked, 10, "draft-send", false, "no Drafts folder resolved for the fallback — refusing blind APPEND elsewhere");
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    if sandbox_created {
        tracked.draft_sandbox = Some(draft_folder.clone());
    } else {
        tracked.draft_used_real_drafts = true;
        tracked.draft_real_drafts_wire = Some(draft_wire.clone());
    }
    // Local drafts row (the Message-ID anchor the draft_id arm reuses).
    let draft_id = format!("live21-{}-draftsend", tag);
    let draft_mid = new_message_id(&format!("live21-{}-draftsend", tag));
    let draft_subject = format!("{}{}-draftsend", SUBJECT_PREFIX, tag);
    {
        let guard = store.lock().unwrap();
        let mb = match queries::ensure_mailbox(guard.conn(), "INBOX") {
            Ok(id) => id,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("temp mailbox seed failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        if let Err(e) = queries::upsert_draft(
            guard.conn(),
            &draft_id,
            mb,
            &draft_mid,
            &draft_subject,
            "live gate draft-send probe",
            &cfg.self_addr,
            "",
            "",
        ) {
            log_step(&tracked, 10, "draft-send", false, &format!("temp draft seed failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }
    tracked.draft_id = Some(draft_id.clone());
    tracked.draft_mid = Some(draft_mid.clone());
    // Enqueue with draft_id set (production path reuses the drafts row
    // Message-ID per the send_queue.rs draft_id arm).
    if let Err(e) = assert_self_only(&cfg.self_addr, &[cfg.self_addr.clone()], &[], &[], &cfg.self_addr) {
        log_step(&tracked, 10, "draft-send", false, &e);
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    {
        let guard = store.lock().unwrap();
        match send_queue::enqueue_send(
            guard.conn(),
            &app_data,
            EnqueueInput {
                from: cfg.self_addr.clone(),
                to: vec![cfg.self_addr.clone()],
                cc: vec![],
                bcc: vec![],
                subject: draft_subject.clone(),
                body: "live gate draft-send probe".to_string(),
                draft_id: Some(draft_id.clone()),
            },
        ) {
            Ok(out) => {
                if out.deduped || out.message_id != draft_mid {
                    log_step(
                        &tracked,
                        10,
                        "draft-send",
                        false,
                        &format!("draft_id arm did not reuse the drafts-row Message-ID (deduped={} mid={})", out.deduped, out.message_id),
                    );
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            }
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("draft-anchored enqueue_send failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Server copy: APPEND the queued .eml bytes (same Message-ID) to the
    // sandbox / fallback Drafts. Harness-issued, MID-tracked, allow-listed.
    let draft_eml = {
        let guard = store.lock().unwrap();
        match queries::get_send_row_by_message_id(guard.conn(), &draft_mid) {
            Ok(Some(row)) => std::fs::read(&row.eml_path).unwrap_or_default(),
            _ => Vec::new(),
        }
    };
    if draft_eml.is_empty() {
        log_step(&tracked, 10, "draft-send", false, "queued .eml unreadable for the draft server copy");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if let Err(e) = assert_harness_append_target(&draft_wire, &[draft_wire.clone()]) {
        log_step(&tracked, 10, "draft-send", false, &format!("append-target guard refused the draft wire: {}", e));
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    {
        let mut lease = match manager.lease_for(&draft_wire).await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("draft-wire lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        if let Err(e) = lease.session().append_message(&draft_wire, "\\Seen", &draft_eml).await {
            log_step(&tracked, 10, "draft-send", false, &format!("draft server-copy APPEND failed: {}", e));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
        match lease.session().uid_search_header("Message-ID", &draft_mid).await {
            Ok(hits) => match assert_exactly_one(&hits, "draft-send server copy") {
                Ok(u) => {
                    tracked.draft_server_uid = Some(u);
                }
                Err(e) => {
                    log_step(&tracked, 10, "draft-send", false, &e);
                    return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
                }
            },
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("draft-wire SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    }
    // Flush the draft-anchored send for real (SMTP → INBOX arrival + Sent
    // filing — the send half must succeed for the orphan verdict to mean
    // anything).
    let calls_before_draft = send_calls.load(Ordering::SeqCst);
    eprintln!("[live-gate] {} (draft-send leg)", envelope_log_line(&cfg.self_addr, &[cfg.self_addr.clone()]));
    let draft_summary = worker.flush_send_queue(&flush_env).await;
    let calls_after_draft = send_calls.load(Ordering::SeqCst);
    record_calls(&tracked, "draft-send", calls_before_draft, calls_after_draft);
    eprintln!(
        "[live-gate] draft flush done: sent={} deferred={} failed={} uncertain={} unfiled={} transport_calls={}",
        draft_summary.sent,
        draft_summary.deferred,
        draft_summary.failed,
        draft_summary.uncertain,
        draft_summary.sent_unfiled,
        calls_after_draft
    );
    if draft_summary.sent != 1 || calls_after_draft != calls_before_draft + 1 {
        log_step(
            &tracked,
            10,
            "draft-send",
            false,
            &format!("draft send did not deliver once (sent={} calls {} -> {})", draft_summary.sent, calls_before_draft, calls_after_draft),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    let draft_uid = match poll_inbox_for_mid(&manager, &draft_mid, inbox_validity).await {
        Ok(u) => u,
        Err(d) => {
            log_step(&tracked, 10, "draft-send", false, &d);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    if !peek_confirms_mid(&manager, "INBOX", draft_uid, &draft_mid).await {
        log_step(&tracked, 10, "draft-send", false, "PEEK header confirm failed (subject prefix / Message-ID)");
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    let draft_sent_uid = match search_sent_for_mid(&manager, &sent_wire, &draft_mid).await {
        Ok(hits) => match assert_exactly_one(&hits, "draft-send sent-copy") {
            Ok(u) => u,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &e);
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        },
        Err(e) => {
            log_step(&tracked, 10, "draft-send", false, &e);
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    };
    tracked.message_ids.push(draft_mid.clone());
    tracked.inbox_uids.push((draft_mid.clone(), draft_uid));
    tracked.sent_uids.push((draft_mid.clone(), draft_sent_uid));
    // BINARY no-orphan verdict on the AUTO path: PASS only if the flush
    // ITSELF removed the local drafts row AND the server copy with zero
    // harness-side discard. No assisted-discard fallback, never a PASS with
    // a GAP footnote.
    let local_gone = {
        let guard = store.lock().unwrap();
        match queries::get_draft(guard.conn(), &draft_id) {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("drafts-row lookup failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    let server_hits: Vec<u32> = {
        let mut lease = match manager.lease_for(&draft_wire).await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("draft-wire lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        match lease.session().uid_search_header("Message-ID", &draft_mid).await {
            Ok(h) => h,
            Err(e) => {
                log_step(&tracked, 10, "draft-send", false, &format!("orphan re-SEARCH failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        }
    };
    // Zero harness-side discard ran on this path — recorded as a constant,
    // not an inference (this leg performs no discard call).
    let harness_discard_ran = false;
    let draft_evidenced = if fallback_note.is_empty() {
        format!("sandbox {:?}", draft_wire)
    } else {
        fallback_note.clone()
    };
    if local_gone && server_hits.is_empty() && !harness_discard_ran {
        if !log_step(
            &tracked,
            10,
            "draft-send",
            true,
            &format!("flush itself removed the local drafts row AND the server copy ({}); drafts-row lookup: gone; server SEARCH: 0 hits; harness discard ran: false (same-transaction delete D-16 observed)", draft_evidenced),
        ) {
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    } else {
        log_step(
            &tracked,
            10,
            "draft-send",
            false,
            &format!("FAIL BY DESIGN (binary verdict): local drafts row gone={} (want true), server copy SEARCH hits={} (want 0), harness discard ran={} (want false); {} — prod flush performs no delete_draft (RESEARCH A6 re-verified this run: draft_id arm reuses the Message-ID, nothing deletes the row); routes to gap closure (delete_draft-in-flush fix or explicit D-16 deferral)", local_gone, server_hits.len(), harness_discard_ran, draft_evidenced),
        );
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 10, "draft-send-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // Send audit (D-14, T-21-01): all five rules over the temp queue rows +
    // rendered bytes, printed as AUDIT-* lines for the wrapper re-check
    // (defense in depth — the bin aborts on any violation first).
    {
        let (rows, calls_snapshot): (Vec<(String, String, String, String, String)>, Vec<(String, usize, usize)>) = {
            let guard = store.lock().unwrap();
            let mut rows = Vec::new();
            if let Ok(mut stmt) = guard.conn().prepare(
                "SELECT message_id, to_addrs, cc_addrs, bcc_addrs, eml_path FROM send_queue ORDER BY created_at",
            ) {
                if let Ok(mapped) = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                }) {
                    rows = mapped.filter_map(|r| r.ok()).collect();
                }
            }
            let calls_snapshot = tracked
                .calls_log
                .lock()
                .map(|l| l.clone())
                .unwrap_or_default();
            (rows, calls_snapshot)
        };
        let mut violations: Vec<String> = Vec::new();
        let mut mids: Vec<String> = Vec::new();
        for (mid, to_json, cc_json, bcc_json, eml_path) in &rows {
            println!("AUDIT-ROW mid={} to={} cc={} bcc={}", mid, to_json, cc_json, bcc_json);
            if !audit_recipients_ok(to_json, cc_json, bcc_json, &cfg.self_addr) {
                violations.push(format!("rule1 recipients not self-only: mid {}", mid));
            }
            mids.push(mid.clone());
            match std::fs::read(eml_path) {
                Ok(bytes) => {
                    let subject = mail_parser::MessageParser::new()
                        .parse(&bytes)
                        .and_then(|m| m.subject().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let subject_ok = !subject.is_empty() && audit_subject_prefixed(&subject);
                    let bcc_absent = !eml_has_bcc_header(&bytes);
                    println!("AUDIT-EML mid={} subject_ok={} bcc_absent={}", mid, subject_ok, bcc_absent);
                    println!("AUDIT-SUBJECT {}", subject);
                    if !subject_ok {
                        violations.push(format!("rule2 subject without prefix: mid {}", mid));
                    }
                    if !bcc_absent {
                        violations.push(format!("rule3 Bcc: header in sent bytes: mid {}", mid));
                    }
                }
                Err(e) => {
                    violations.push(format!("rule2/3 unreadable .eml for mid {}: {}", mid, e));
                }
            }
        }
        for m in &mids {
            println!("AUDIT-MID {}", m);
        }
        let mid_refs: Vec<&str> = mids.iter().map(|s| s.as_str()).collect();
        if let Some(dup) = audit_first_duplicate_mid(&mid_refs) {
            violations.push(format!("rule4 reused Message-ID: {}", dup));
        }
        for (leg, before, after) in &calls_snapshot {
            let delta = after.saturating_sub(*before);
            let exp = expected_calls_delta(leg);
            let ok = exp == Some(delta);
            println!(
                "AUDIT-CALLS leg={} delta={} expected={} ok={}",
                leg,
                delta,
                exp.map(|e| e.to_string()).unwrap_or_else(|| "?".to_string()),
                ok
            );
            if !ok {
                violations.push(format!("rule5 call-count mismatch: leg {} delta {} (want {:?})", leg, delta, exp));
            }
        }
        if violations.is_empty() {
            println!(
                "AUDIT summary: PASS rules=5 rows={} mids-unique=true calls-ok=true",
                rows.len()
            );
            log_step(
                &tracked,
                10,
                "send-audit",
                true,
                &format!("rules 1-5 hold over {} queue rows (self-only recipients, prefixed subjects, no Bcc: header, unique MIDs, call counts reconcile)", rows.len()),
            );
        } else {
            println!("AUDIT summary: FAIL {}", violations.join("; "));
            log_step(&tracked, 10, "send-audit", false, &violations.join("; "));
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }

    // 11. autosave-probe (D-08): the end state must be exactly 1 Sent UID
    // either way — explicit APPEND or server auto-save deduped by the filing
    // leg's probe SEARCH (no twin copy). Which branch ran is named by the
    // flush stderr lines captured in the wrapper log ("deduped in Sent" vs
    // "APPENDed to Sent"). Unfiled legs tolerate 0|1 (the probe signal).
    {
        let mut probe_ok = true;
        let mut probe_detail = String::new();
        let mut probe_counts: Vec<(String, usize, bool)> = Vec::new();
        let mut lease = match manager.lease_for(&sent_wire).await {
            Ok(l) => l,
            Err(e) => {
                log_step(&tracked, 11, "autosave-probe", false, &format!("Sent lease failed: {}", e));
                return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
            }
        };
        for mid in tracked.message_ids.iter() {
            let count = lease.session().uid_search_header("Message-ID", mid).await.unwrap_or_default().len();
            let is_unfiled = tracked.unfiled.iter().any(|u| u == mid);
            let ok = count == 1 || (is_unfiled && count == 0);
            if !ok {
                probe_ok = false;
            }
            probe_detail.push_str(&format!("{}:sent_uids={}{}; ", mid, count, if is_unfiled { "(unfiled)" } else { "" }));
            probe_counts.push((mid.clone(), count, is_unfiled));
        }
        probe_detail.push_str("branch per [SGE send] flush lines in log (deduped-in-Sent vs APPENDed-to-Sent)");
        tracked.probe_counts = probe_counts;
        tracked.probe_detail = probe_detail.clone();
        if !log_step(&tracked, 11, "autosave-probe", probe_ok, &probe_detail) {
            return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
        }
    }
    if !check_list_stable(&manager, &before, &list_guard, &tracked, 11, "autosave-probe-list").await {
        return cleanup(&manager, &db_path, &app_data, &before, &tracked).await;
    }

    // 12. cleanup — ALWAYS runs: expunge tracked UIDs, re-LIST must equal the
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
    // Secret scan (D-13, T-21-04): full SQLite dump of the temp store,
    // scanned in-process BEFORE cleanup removes the DB file (post-run the
    // file is gone, so the wrapper cannot dump it — it re-checks the
    // SCAN-DB line instead). Fixed-substring match over the full password
    // values; any hit names WHICH artifact matched, never the secret.
    {
        let (tables, rows, dump) =
            match rusqlite::Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            ) {
                Ok(conn) => dump_store_for_scan(&conn),
                Err(e) => {
                    eprintln!(
                        "[live-gate] secret-scan: temp DB unreadable ({e}) — scanning empty dump"
                    );
                    (Vec::new(), 0, String::new())
                }
            };
        let secrets: Vec<&str> = tracked
            .scan_secrets
            .iter()
            .map(|s| s.as_str())
            .collect();
        let hit = scan_text_has_secret(&dump, &secrets);
        if hit {
            println!(
                "SECRET-LEAK: credential substring in temp-DB dump (artifact: sqlite-dump tables={:?}; see which artifact matched, never the secret)",
                tables
            );
        }
        println!(
            "SCAN-DB tables={} rows={} result={} (log + full temp-DB dump carry {} credential substring)",
            tables.len(),
            rows,
            if hit { "FAIL" } else { "PASS" },
            if hit { "a" } else { "no" }
        );
        if !log_step(
            tracked,
            TOTAL_STEPS,
            "secret-scan",
            !hit,
            &format!(
                "full temp-DB dump scanned (tables={} rows={}); {}",
                tables.len(),
                rows,
                if hit {
                    "CREDENTIAL HIT — run fails"
                } else {
                    "no credential substring in dump (log scanned by the wrapper)"
                }
            ),
        ) {
            ok = false;
        }
        if hit {
            ok = false;
        }
    }
    // INBOX arrivals expunged via INBOX, Sent copies via the Sent wire —
    // UIDs are per-folder (T-21-05).
    for (mid, uid) in tracked.inbox_uids.iter() {
        if !expunge_tracked(mgr, "INBOX", *uid, mid).await {
            ok = false;
        }
    }
    let sent_folder = tracked.sent_wire.clone().unwrap_or_else(|| "INBOX".to_string());
    for (mid, uid) in tracked.sent_uids.iter() {
        if !expunge_tracked(mgr, &sent_folder, *uid, mid).await {
            ok = false;
        }
    }
    let after = list_snapshot(mgr).await.unwrap_or_default();
    let list_guard = format!("{SANDBOX_PREFIX}never-created");
    let clean = unexpected_list_delta(before, &after, &list_guard).is_empty();
    if !clean {
        ok = false;
    }
    log_step(
        tracked,
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
    // Exit-code contract (USAGE: 0 all-pass · 1 assertion FAIL): a mid-run
    // STEP FAIL must propagate even when cleanup itself succeeds — the
    // canonical outcome log is the witness (Rule 1 fix: pre-21-02 code
    // returned `ok && clean` only, swallowing leg failures).
    let steps_ok = tracked
        .step_log
        .lock()
        .map(|log| log.iter().all(|s| s.ok))
        .unwrap_or(false);
    if ok && clean && steps_ok { 0 } else { 1 }
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

    #[test]
    fn live_gate_21_timeout_transport_armed_emits_uncertain_with_zero_inner_calls() {
        let (inner, inner_calls) = CountingTransport::offline();
        let (t, armed, forwarded) = TimeoutTransport::new(Arc::new(inner));
        armed.store(true, Ordering::SeqCst);
        let acct = SmtpAccount {
            host: "smtp.example".to_string(),
            port: 587,
            username: "u".to_string(),
            password: "p".to_string(),
        };
        let verdict = t.send_raw(&acct, "a@example.com", &["b@example.com".to_string()], b"bytes");
        assert!(
            matches!(verdict, SmtpOutcome::Uncertain { .. }),
            "armed fault must surface Uncertain"
        );
        assert_eq!(inner_calls.load(Ordering::SeqCst), 0, "zero inner calls while armed");
        assert_eq!(forwarded.load(Ordering::SeqCst), 0, "zero forwarded calls while armed");
        // Disarmed: forwards to the inner transport and counts once.
        armed.store(false, Ordering::SeqCst);
        assert_eq!(
            t.send_raw(&acct, "a@example.com", &["b@example.com".to_string()], b"bytes"),
            SmtpOutcome::Sent
        );
        assert_eq!(inner_calls.load(Ordering::SeqCst), 1);
        assert_eq!(forwarded.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn live_gate_21_harness_append_target_allows_only_the_allow_list() {
        let sent = "Sent".to_string();
        let sandbox = "SGE-Test-1-drafts".to_string();
        assert!(assert_harness_append_target(&sent, &[sent.clone()]).is_ok());
        assert!(assert_harness_append_target(&sandbox, &[sent.clone(), sandbox.clone()]).is_ok());
        // INBOX is never appendable by the harness — arrival lands by
        // server delivery only.
        assert!(assert_harness_append_target("INBOX", &[sent.clone(), sandbox.clone()]).is_err());
        assert!(assert_harness_append_target("Drafts", &[sent.clone()]).is_err());
        assert!(assert_harness_append_target(&sent, &[]).is_err());
    }

    #[test]
    fn live_gate_21_backoff_bounds_growth_pinned() {
        // Growth pinned offline per the plan: (24,36) at 0, cap (720,1080).
        assert_eq!(send_queue::backoff_bounds(0), (24, 36));
        assert_eq!(send_queue::backoff_bounds(u32::MAX), (720, 1080));
        let (lo, hi) = send_queue::backoff_bounds(1);
        assert_eq!((lo, hi), (48, 72));
    }

    #[test]
    fn live_gate_21_retry_within_bounds_shapes() {
        let now = chrono::Utc::now().timestamp();
        let in_window = chrono::DateTime::from_timestamp(now + 30, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(retry_within_bounds(&Some(in_window), 0, now, 15));
        assert!(!retry_within_bounds(&None, 0, now, 15));
        assert!(!retry_within_bounds(&Some("not-a-timestamp".to_string()), 0, now, 15));
        // Far-future schedule is outside the bounds (not a valid backoff).
        let far = chrono::DateTime::from_timestamp(now + 100_000, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(!retry_within_bounds(&Some(far), 0, now, 15));
    }

    #[test]
    fn live_gate_21_deferred_backoff_ok_vacuous_and_violation() {
        let now = chrono::Utc::now().timestamp();
        let (ok, _) = deferred_backoff_ok(&[], now);
        assert!(ok, "no deferred rows passes vacuously");
        let good_ts = chrono::DateTime::from_timestamp(now + 60, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let (ok, _) = deferred_backoff_ok(&[("q1".to_string(), 1, Some(good_ts))], now);
        assert!(ok);
        let (ok, detail) = deferred_backoff_ok(&[("q9".to_string(), 0, None)], now);
        assert!(!ok, "missing schedule must fail: {detail}");
    }

    #[test]
    fn live_gate_21_step_log_records_outcomes_for_report() {
        let tracked = TrackedMail::empty();
        assert!(log_step(&tracked, 7, "offline-flush", true, "detail-a"));
        assert!(!log_step(&tracked, 8, "uncertain-reconcile", false, "detail-b"));
        assert_eq!(logged_outcome(&tracked, 7, "offline-flush"), Some(true));
        assert_eq!(logged_outcome(&tracked, 8, "uncertain-reconcile"), Some(false));
        assert_eq!(logged_outcome(&tracked, 9, "crash-recover"), None);
        record_calls(&tracked, "offline-flush", 3, 4);
        let calls = tracked.calls_log.lock().unwrap();
        assert_eq!(calls.as_slice(), &[("offline-flush".to_string(), 3, 4)]);
    }

    #[test]
    fn live_gate_21_expected_calls_delta_pins_reconcile_at_zero() {
        for leg in ["arrival", "offline-flush", "draft-send", "crash-final-flush"] {
            assert_eq!(expected_calls_delta(leg), Some(1));
        }
        // Reconcile / re-flush / wait-poll legs must add ZERO transport
        // calls — the reconcile-not-resend proof.
        for leg in [
            "uncertain-armed-flush",
            "uncertain-reconcile",
            "crash-triage-flush",
            "crash-reconcile",
            "crash-backoff-poll",
            "sent-unfiled-reflush",
        ] {
            assert_eq!(expected_calls_delta(leg), Some(0), "leg {leg} must add zero calls");
        }
        assert_eq!(expected_calls_delta("no-such-leg"), None);
    }
}
