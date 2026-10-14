//! `live_gate_26` — Phase 26 bundle: sandboxed live proof of the deferred
//! gates (v1.1 read-side 6/7/8, v1.3 confirm→MOVE→undo + RENAME/DELETE +
//! batch/chunk probe, v1.2 12/13-derived rows by citation of the locked
//! 21/22 runners) against the real server, closed by one combined bundle
//! REPORT with reasoned deferral rows.
//!
//! Structural clone of `live_gate_22` (per 26-CONTEXT D-31): the bin owns ONE
//! [`SessionManager`] plus one temp-file [`Store`] for the whole run — never
//! a second IMAP connection alongside the manager. Every step prints
//! `STEP n/N <name>: PASS|FAIL <detail>` plus a one-line `REPORT gate=…
//! verdict=… evidence=…` row; the process exits 0 all-pass / 1 assertion
//! FAIL / 2 pre-flight-skip-or-env-missing.
//!
//! Safety contract (26-CONTEXT D-35..D-44, carried verbatim from 21/22):
//! - Mutating ops target ONLY the per-run sandbox folder `SGE-Test-<ts>-<pid>`
//!   at account root. Every mutating call site asserts the sandbox prefix in
//!   code before touching the session.
//! - Credentials travel as env vars only (`--password` argv is refused, same
//!   as `sync_diag`). `--seed-only` writes the env password to the OS keyring
//!   once; the run itself prefers keyring-loaded values.
//! - All reads are BODY.PEEK-only; UIDs re-derived via UID SEARCH per step
//!   with UIDVALIDITY epoch gating — abort loud on mid-run epoch change.
//! - Test mail ONLY to SGE_LIVE_SELF with the `SGE-TEST-` subject prefix.
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
    SecurityMode, SyncError, SyncSession,
};
use sge_lib::send_queue::rfc2822_now;
use sge_lib::store::{queries, Store};
use sge_lib::sync::worker::SyncWorker;
use sge_lib::sync::SyncCallback;

/// Sandbox folder prefix (D-35). Every mutating call asserts it in code.
pub const SANDBOX_PREFIX: &str = "SGE-Test-";

/// Subject prefix marking every test message (only-self test mail).
pub const SUBJECT_PREFIX: &str = "SGE-TEST-";

/// Step count for the full live run (gates 6/7/8 + audit + v1.3 legs +
/// 12/13 citation rows + cleanup).
pub const TOTAL_STEPS: usize = 15;

/// Cleanup discipline (D-37): 3 retries with backoff, then exact manual
/// steps — never an unbounded destructive loop.
const CLEANUP_BACKOFF_SECS: [u64; 3] = [5, 15, 45];

/// Per-run sandbox folder at account root (D-35): unique timestamp + pid, so
/// concurrent runs never share a folder and a present name always means a
/// prior failed cleanup (never reused).
pub fn sandbox_name() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{SANDBOX_PREFIX}{ts}-{}", std::process::id())
}

/// Pre-flight prefix guard (D-36, T-26-02): refuse any wire name outside the
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

/// Stable Message-ID for one live compose session (unique per run).
pub fn run_message_id(run_tag: &str) -> String {
    new_message_id(&format!("live26-{run_tag}"))
}

/// Render one self-addressed test message via the prod codec (never
/// string-concatenated MIME). Subject always carries the `SGE-TEST-` prefix
/// (D-16 audit hook, carried from 21/22).
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

/// Sorted wire-name set of a LIST snapshot (D-36 mid-run diff basis).
pub fn sorted_names(infos: &[MailboxInfo]) -> Vec<String> {
    let mut names: Vec<String> = infos.iter().map(|i| i.name.clone()).collect();
    names.sort();
    names
}

/// LIST deltas other than the sandbox itself (D-36): any entry here aborts
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

/// Structural pending indicator: queued Seen toggles plus queued IMAP ops
/// for one mailbox. Mirrors the `sync_status` pending sum the existing
/// Phase 10/12 UI badge reads, so pending-until-acked is asserted on the
/// number with no new UI surface.
pub fn pending_depth(conn: &rusqlite::Connection, mailbox_id: u64) -> i64 {
    let seen = queries::outbox_count(conn, mailbox_id).unwrap_or(0);
    let imap = queries::imap_outbox_count(conn, mailbox_id).unwrap_or(0);
    seen.saturating_add(imap)
}

/// Transcript-order predicate (D-50): an `imap replay` line must precede the
/// first sweep line. The worker emits `[SGE sync] imap replay: ...` before
/// the sweep code runs, so file order in the captured log is the evidence;
/// this is the checkable shape of that rule.
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

/// Fixed-substring secret scan predicate (T-26-04/T-26-05): the index of the
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

/// Only-test-mail audit predicate (T-26-01): every subject carries the
/// `SGE-TEST-` prefix and every recipient equals the run's self address.
pub fn audit_test_mail(
    subjects: &[&str],
    recipients: &[&str],
    self_addr: &str,
) -> Result<(), String> {
    for subject in subjects {
        if !subject.contains(SUBJECT_PREFIX) {
            return Err(format!("unprefixed subject fails audit: {subject:?}"));
        }
    }
    for rcpt in recipients {
        if *rcpt != self_addr {
            return Err("non-self recipient fails audit (only mail to self)".to_string());
        }
    }
    Ok(())
}

/// Cleanup retry schedule (D-37): 3 attempts, 5s/15s/45s.
pub fn cleanup_backoff_secs() -> [u64; 3] {
    CLEANUP_BACKOFF_SECS
}

/// Probe chunk size for the v1.3 batch leg (T-26-09): deliberately small —
/// the PROVEN batch path chunks 25 (Phase 20), but this live probe only
/// needs ≥2 chunks to prove per-chunk UIDVALIDITY re-check + resume.
/// Small probe, never a full-account sweep (server-throttle etiquette).
pub const BATCH_PROBE_CHUNK: usize = 3;

/// Seed count for the batch probe: 7 messages at chunk 3 → ranges
/// (0,3),(3,6),(6,7): three chunks with a partial tail (exercises it).
pub const BATCH_PROBE_SEEDS: usize = 7;

/// Sandbox-scoped Auto/ subtree wire name (T-26-07): the v1.3 filing legs
/// file into `{sandbox}-Auto-{top}`, NEVER the real `Auto/` tree. The
/// derived name keeps the sandbox prefix (the prefix guard accepts it)
/// while [`assert_never_real_auto`] refuses anything under the real tree.
pub fn sandbox_auto_wire(sandbox: &str, top: &str) -> String {
    format!("{sandbox}-Auto-{top}")
}

/// Refuse the real Auto/ tree (T-26-07): `Auto`, `Auto/` children, and the
/// legacy `.`-delimited form. The filing legs call this on every dest wire
/// BEFORE the prefix assert — both must pass.
pub fn assert_never_real_auto(wire: &str) -> Result<(), String> {
    if wire == "Auto" || wire.starts_with("Auto/") || wire.starts_with("Auto.") {
        return Err(format!(
            "live-gate: refusing real Auto/ tree target {wire:?} (sandbox-scoped Auto only)"
        ));
    }
    Ok(())
}

/// Chunk plan for the batch probe: (start,end) index ranges over `total`
/// items at `size` per chunk. Pure so the offline doubles pin the exact
/// ranges (including the partial tail).
pub fn plan_chunks(total: usize, size: usize) -> Vec<(usize, usize)> {
    let size = size.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    while start < total {
        let end = (start + size).min(total);
        out.push((start, end));
        start = end;
    }
    out
}

/// Backend-side folder-delete guard order (Phase 11 command-layer rule,
/// mirrored in-harness because `guard_delete` is `pub(crate)` to the
/// command module): INBOX refusal → non-empty refusal until emptied. The
/// RENAME/DELETE leg proves both refusals live before any DELETE verb
/// fires — guards backend-side, never UI-only (T-26-08).
pub fn assert_folder_delete_allowed(wire: &str, messages: u32) -> Result<(), String> {
    if wire.eq_ignore_ascii_case("inbox") {
        return Err("live-gate: INBOX is server-fixed — never renamed or deleted".to_string());
    }
    if messages > 0 {
        return Err(format!(
            "live-gate: folder holds {messages} messages — empty (scoped expunge) before DELETE"
        ));
    }
    Ok(())
}

/// Prior-run log pointers for the 12/13 citation STEPs (D-30): when the
/// human has already run live-21/live-22, these env vars point at the
/// captured /tmp logs and the bundle cites their REPORT lines instead of
/// re-implementing (or duplicate-sending) the 21/22 legs. Unset — the
/// normal case until the human runs them — yields reasoned SKIP deferral
/// rows with resume pointers, never silent drops.
pub const CITE_ENV_21: &str = "SGE_LIVE21_LOG";
pub const CITE_ENV_22: &str = "SGE_LIVE22_LOG";

/// REPORT markers the citation STEPs grep for in the prior logs:
/// live_gate_22 STEP 5 (draft one-copy save), the live_gate_21 draft-send
/// binary verdict (SMTP 587 send), live_gate_22 STEP 11 (Seen both ways).
pub const CITE_DRAFT_MARKER: &str = "one-copy: PASS";
pub const CITE_SEND_MARKER: &str = "REPORT draft-send binary verdict: PASS";
pub const CITE_SEEN_MARKER: &str = "seen-both-ways: PASS";

/// First prior-log line containing `marker` (the cited REPORT evidence),
/// trimmed to 160 chars (STEP lines carry UIDs/counts, never secrets —
/// the secret scan gates both the 21/22 wrappers and this one). `None`
/// when the leg never passed there; empty markers never match.
pub fn citation_evidence(log_text: &str, marker: &str) -> Option<String> {
    if marker.trim().is_empty() {
        return None;
    }
    log_text.lines().find(|l| l.contains(marker)).map(|l| {
        let t = l.trim();
        if t.len() > 160 {
            t[..160].to_string()
        } else {
            t.to_string()
        }
    })
}

/// Reasoned deferral evidence (D-47): `reason=<concrete> resume=<pointer>`.
/// A bare "deferred" with no reason is a FAIL, not a deferral — enforced
/// by [`is_reasoned_deferral`], unit-tested below and doubled offline.
pub fn deferral_evidence(reason: &str, resume: &str) -> String {
    format!("reason={reason} resume={resume}")
}

/// True only when both deferral halves are present, in canonical order,
/// and non-empty. Rejects bare `deferred`, either half missing, either
/// half blank.
pub fn is_reasoned_deferral(evidence: &str) -> bool {
    let rpos = match evidence.find("reason=") {
        Some(p) => p,
        None => return false,
    };
    let spos = match evidence.find("resume=") {
        Some(p) => p,
        None => return false,
    };
    if spos < rpos {
        return false;
    }
    let reason = evidence[rpos + "reason=".len()..spos].trim();
    let resume = evidence[spos + "resume=".len()..].trim();
    !reason.is_empty() && !resume.is_empty()
}

/// Full deferred set the bundle REPORT must cover (D-28/D-46): v1.1 6/7/8
/// legs, v1.3 legs, 12/13 citation rows. `cleanup` fills any gate without
/// a collected row as NOT-RUN — no silent drops, structurally enforced.
pub const ALL_GATES: &[&str] = &[
    "seen-roundtrip",
    "tree-browse-badges",
    "arrival-tick",
    "reconnect-recovery",
    "confirm-move-undo",
    "rename-delete",
    "batch-probe",
    "cite-draft-onecopy",
    "cite-smtp-send",
    "cite-seen-22",
];

/// Gates in ALL_GATES with no collected row (incomplete run — never a
/// silent pass). Pure so the offline doubles pin the fill shape.
pub fn missing_gates<'a>(collected: &[(String, String, String)], all: &'a [&str]) -> Vec<&'a str> {
    all.iter()
        .copied()
        .filter(|g| !collected.iter().any(|(gate, _, _)| gate == g))
        .collect()
}

/// Documented STARTTLS deferral (deferred idea, carried from 26-CONTEXT):
/// the account's server is 993-only, so a live STARTTLS proof against a
/// 143 server is impossible here — stub plus unit coverage stand.
pub const STARTTLS_DEFER_REASON: &str =
    "server is 993-only (no 143 STARTTLS endpoint); stub plus unit coverage stand";
pub const STARTTLS_DEFER_RESUME: &str =
    "re-run against a 143 STARTTLS server, else accept stub coverage";

/// SQLite dump writer: renders every user table of the run's temp Store as
/// text so the wrapper can grep the dump for credential substrings.
/// Read-only open — the live Store keeps its own connection. FTS5 virtual
/// tables are skipped (`SELECT *` is unreliable on them and they mirror
/// `messages` content, which carries no credential surface — passwords only
/// ever live in keyring/env, never mail rows).
pub fn write_sqlite_dump_text(
    db_path: &std::path::Path,
    dump_path: &std::path::Path,
) -> Result<usize, String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
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

// ── App-level offline toggle ─────────────────────────────────────────
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
            self.inner
                .condstore_fetch_changedsince(&range, modseq)
                .await
        })
    }
}

// ── Config (env-only) ──────────────────────────────────────────────────

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
    // Keyring-first: the seed step wrote the password once; prefer it.
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

/// `--seed-only`: write the env password to the OS keyring once, plus the
/// server config — the run itself then loads via the keyring path.
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

/// UIDVALIDITY epoch gate: every leg re-derives the epoch and aborts loud
/// on a mid-run generation change (stale UIDs must never be STOREd).
async fn check_epoch(mgr: &SessionManager, wire: &str, expected: u32) -> Result<(), String> {
    let now = current_validity(mgr, wire)
        .await
        .map_err(|e| format!("validity read failed: {e}"))?;
    if now != expected {
        return Err(format!(
            "UIDVALIDITY changed mid-run ({expected} → {now}) — aborting, UIDs are stale"
        ));
    }
    Ok(())
}

fn step(n: usize, name: &str, ok: bool, detail: &str) -> bool {
    println!(
        "STEP {n}/{TOTAL_STEPS} {name}: {} {detail}",
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

/// SKIP verdict line for reasoned deferrals (D-47): the gate ran far
/// enough to know WHY it cannot pass yet. Always paired with a
/// `verdict=SKIP` REPORT row carrying `reason=` + `resume=`.
fn step_skip(n: usize, name: &str, detail: &str) {
    println!("STEP {n}/{TOTAL_STEPS} {name}: SKIP {detail}");
}

/// Mandatory per-leg REPORT row (D-46): gate, verdict, evidence pointer. A
/// gate without a row is an incomplete run, not a silent pass.
fn report(gate: &str, verdict: &str, evidence: &str) {
    println!("REPORT gate={gate} verdict={verdict} evidence={evidence}");
}

fn manual_cleanup_steps(sandbox: &str) -> String {
    format!(
        "MANUAL-CLEANUP folder={sandbox}: webmail (or any client) → folder list → \
         {sandbox} → delete; then re-run preflight (LIST must not contain it)"
    )
}

// ── Live steps (gates 6/7/8) ───────────────────────────────────────────
// Every step re-asserts the sandbox prefix before mutating and re-DIFFs LIST
// against the pre-run snapshot (D-36). UIDs are always re-derived via UID
// SEARCH Message-ID (async-imap APPEND returns unit, no APPENDUID) with a
// fresh UIDVALIDITY epoch check. Each returns the PASS detail or the FAIL
// reason; run() prints the STEP line plus the mandatory REPORT row.

type StoreRef = Arc<Mutex<Store>>;

/// Rendered subjects/bodies are module constants so the audit (step 8)
/// checks exactly what the legs rendered — never a divergent literal.
const SFX_SEEN: &str = "seen-roundtrip";
const BODY_SEEN: &str = "seen leg body";
const SFX_ARRIVAL: &str = "arrival-tick";
const BODY_ARRIVAL: &str = "arrival leg body";
const SFX_FILING: &str = "filing-confirm";
const BODY_FILING: &str = "filing leg body";
const SFX_RENAME: &str = "rename-folder";
const BODY_RENAME: &str = "rename leg body";
const SFX_BATCH: &str = "batch-probe";
const BODY_BATCH: &str = "batch probe body";

fn noop_cb() -> SyncCallback {
    Arc::new(|_| {})
}

/// Borrow `extras` as `&[&str]` for the LIST-delta allow-list: the v1.3
/// legs' sandbox-scoped folders are expected mid-run names, not drift.
fn extras_str(extras: &[String]) -> Vec<&str> {
    extras.iter().map(|s| s.as_str()).collect()
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
    lease
        .session()
        .uid_search_header("Message-ID", msg_id)
        .await
}

/// Local row resolver: (messages.id, uid) for one RFC Message-ID in one
/// mailbox. Sync passes may delete+re-insert rows when a MOVE re-homes a
/// message (new UID, possibly new row id), so legs re-resolve by RFC
/// Message-ID after every sync — never thread a stale local id.
fn resolve_local(
    conn: &rusqlite::Connection,
    mailbox_id: u64,
    rfc_msg_id: &str,
) -> Result<(u64, u32), String> {
    conn.query_row(
        "SELECT id, uid FROM messages WHERE mailbox_id = ?1 AND message_id = ?2",
        rusqlite::params![mailbox_id, rfc_msg_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(|e| format!("local row missing for Message-ID {rfc_msg_id}: {e}"))
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

/// STEP 4, gate 6 (Seen round-trip): APPEND a self-addressed SGE-TEST-
/// message into the shared sandbox, STORE +Seen via the existing `set_seen`
/// manager entry, assert FLAGS via fetch_envelopes only, drop the session
/// and reconnect through the single SessionManager, assert Seen survives,
/// then queue an offline Seen change and assert offline-replay convergence
/// through the real replay_outbox entry with pending-until-acked end-state.
async fn live_seen_roundtrip(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    mailbox_id: u64,
    epoch: u32,
    self_addr: &str,
    msg_id: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    check_epoch(mgr, wire, epoch).await?;
    // APPEND via the existing prod verb (never a raw session call here —
    // the arrival leg covers the raw-APPEND path).
    let bytes = draft_bytes(self_addr, SFX_SEEN, BODY_SEEN, msg_id);
    let uid = mgr
        .save_draft_copy_in(wire, msg_id, &bytes, None, Some(epoch))
        .await
        .map_err(|e| format!("APPEND-new failed: {e}"))?;
    // STORE +Seen via the existing manager entry (reconnect-retry inside).
    mgr.set_seen_in(wire, uid, true)
        .await
        .map_err(|e| format!("online read STORE failed: {e}"))?;
    // FLAGS assert reads through fetch_envelopes only (PEEK-safe).
    if !flag_has_seen(mgr, wire, uid)
        .await
        .map_err(|e| e.to_string())?
    {
        return Err("online read: \\Seen absent after STORE".to_string());
    }
    // Drop the session (lease scope ends) and reconnect through the single
    // SessionManager: a fresh lease re-establishes the session.
    drop(mgr.lease_for(wire).await.map_err(|e| e.to_string())?);
    check_epoch(mgr, wire, epoch).await?;
    if !flag_has_seen(mgr, wire, uid)
        .await
        .map_err(|e| e.to_string())?
    {
        return Err("Seen did not survive reconnect".to_string());
    }
    // Queue an offline Seen change, then converge through the real
    // replay_outbox entry with the epoch-mismatch and absent-UID drop rules
    // armed (current epoch, live UID set).
    {
        let guard = store.lock().unwrap();
        queries::enqueue_outbox(guard.conn(), mailbox_id, uid, false, epoch)
            .map_err(|e| format!("enqueue seen=false: {e}"))?;
        if pending_depth(guard.conn(), mailbox_id) <= 0 {
            return Err("pending indicator never visible for Seen op".to_string());
        }
    }
    let live: HashSet<u32> = search_uids_in(mgr, wire)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    if !live.contains(&uid) {
        return Err("seen target UID vanished before replay".to_string());
    }
    {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        let summary = worker
            .replay_outbox(lease.session(), mailbox_id, epoch, Some(&live))
            .await
            .map_err(|e| format!("replay seen=false failed: {e}"))?;
        if summary.acked != 1 {
            return Err(format!(
                "replay: acked={} dropped={} failed={}",
                summary.acked, summary.dropped, summary.failed
            ));
        }
    }
    {
        let guard = store.lock().unwrap();
        if pending_depth(guard.conn(), mailbox_id) != 0 {
            return Err("queue not drained after replay (want pending-until-acked 0)".to_string());
        }
    }
    let has = flag_has_seen(mgr, wire, uid)
        .await
        .map_err(|e| e.to_string())?;
    if has {
        return Err("replayed seen=false: FLAGS still reads Seen".to_string());
    }
    Ok(format!(
        "uid {uid}: online Seen + reconnect survival + offline-replay convergence (acked=1, depth 0)"
    ))
}

/// STEP 5, gate 7 (folder tree/browse/badges): LIST from the server versus
/// the pre-run snapshot, assert the sandbox appears and no real folder was
/// created/renamed/deleted (LIST-delta limited to the sandbox), browse the
/// sandbox per-folder, assert STATUS UNSEEN badge counts match the local
/// badge derivation after a guarded sync pass.
async fn live_tree_browse_badges(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    before: &[String],
    epoch: u32,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    check_epoch(mgr, wire, epoch).await?;
    // Tree: server LIST versus the pre-run snapshot.
    let after = list_snapshot(mgr)
        .await
        .map_err(|e| format!("LIST failed: {e}"))?;
    if !after.iter().any(|n| n == wire) {
        return Err("sandbox missing from server LIST".to_string());
    }
    let delta = unexpected_list_delta(before, &after, wire);
    if !delta.is_empty() {
        return Err(format!(
            "real-folder LIST drift (want sandbox-only): {delta:?}"
        ));
    }
    // Browse: per-folder UID SEARCH + envelope fetch (PEEK-safe).
    let uids = search_uids_in(mgr, wire)
        .await
        .map_err(|e| format!("browse SEARCH failed: {e}"))?;
    if uids.is_empty() {
        return Err("browse: sandbox holds zero UIDs (gate-6 seed missing?)".to_string());
    }
    {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        let ranges = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let headers = lease
            .session()
            .fetch_envelopes(&ranges)
            .await
            .map_err(|e| format!("browse FETCH failed: {e}"))?;
        if headers.len() != uids.len() {
            return Err(format!(
                "browse: FETCH {} of {} envelopes",
                headers.len(),
                uids.len()
            ));
        }
    }
    // Badges: server STATUS UNSEEN versus the local badge derivation. The
    // guarded sync pass below writes the STATUS datum into the local cache
    // (same shape the Phase 8 tick uses); the cached datum must equal the
    // server datum, and the local unread derivation is reported as evidence.
    let status = mgr
        .mailbox_status(wire)
        .await
        .map_err(|e| format!("STATUS failed: {e}"))?;
    {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("badge sync pass failed: {e}"))?;
    }
    let (cached, unread) = {
        let guard = store.lock().unwrap();
        let rows = queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?;
        let row = rows
            .iter()
            .find(|r| r.name == wire)
            .ok_or_else(|| "sandbox row missing from local cache".to_string())?;
        (row.unseen_count as u32, row.unread_count)
    };
    if cached != status.unseen {
        return Err(format!(
            "badge mismatch: STATUS UNSEEN={} but cached={}",
            status.unseen, cached
        ));
    }
    Ok(format!(
        "LIST delta=sandbox-only, browsed {} uids, STATUS UNSEEN={} == cached (local unread={})",
        uids.len(),
        status.unseen,
        unread
    ))
}

/// STEP 6, gate 8a (arrival tick): APPEND a self-addressed arrival into the
/// sandbox from a second logical sender path — a raw session APPEND rather
/// than the save_draft verb (still self-only, audited before transport) —
/// then assert the guarded tick-shaped pass (`sync_with_borrowed`, the same
/// entry the Phase 8 poll tick drives) picks it up without manual refresh.
async fn live_arrival_tick(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    mailbox_id: u64,
    epoch: u32,
    self_addr: &str,
    arrival_msg_id: &str,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    check_epoch(mgr, wire, epoch).await?;
    // Second logical sender path: raw APPEND with no flags (unseen
    // arrival), self-only audit BEFORE any transport call.
    let bytes = draft_bytes(self_addr, SFX_ARRIVAL, BODY_ARRIVAL, arrival_msg_id);
    audit_test_mail(
        &[&format!("{SUBJECT_PREFIX}{SFX_ARRIVAL}")],
        &[self_addr],
        self_addr,
    )?;
    {
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        lease
            .session()
            .append_message(wire, "", &bytes)
            .await
            .map_err(|e| format!("arrival APPEND failed: {e}"))?;
    }
    let arrival_uid = {
        let hits = search_msg_uids(mgr, wire, arrival_msg_id)
            .await
            .map_err(|e| e.to_string())?;
        if hits.len() != 1 {
            return Err(format!("arrival seed: want 1 UID, got {hits:?}"));
        }
        hits[0]
    };
    // Guarded tick: the same full-pass entry the poll timer drives. No
    // manual refresh — the pass itself must surface the arrival locally.
    {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("tick pass failed: {e}"))?;
    }
    {
        let guard = store.lock().unwrap();
        let local = queries::all_local_uids(guard.conn(), mailbox_id).map_err(|e| e.to_string())?;
        if !local.contains(&arrival_uid) {
            return Err(format!(
                "tick did not pick up arrival uid {arrival_uid} (local {local:?})"
            ));
        }
    }
    Ok(format!(
        "arrival uid {arrival_uid} picked up by guarded tick pass (no manual refresh)"
    ))
}

/// STEP 7, gate 8b (stale-session reconnect recovery): prove the offline
/// window through the GateSession decorator (writes fail while closed), drop
/// the session mid-run, then assert reconnect recovery resumes sync with no
/// silent gaps — backfill sweep convergence plus replay-before-sweep order
/// evidence in the log.
async fn live_reconnect_recovery(
    mgr: &SessionManager,
    store: &StoreRef,
    wire: &str,
    mailbox_id: u64,
    epoch: u32,
    arrival_uid: u32,
) -> Result<String, String> {
    assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
    check_epoch(mgr, wire, epoch).await?;
    let uids_before = search_uids_in(mgr, wire).await.map_err(|e| e.to_string())?;
    if !uids_before.contains(&arrival_uid) {
        return Err("precondition: arrival UID missing before reconnect leg".to_string());
    }
    // Offline window: GateSession proves writes fail while closed, reads
    // forward untouched.
    {
        let closed = Arc::new(AtomicBool::new(true));
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        let before_count = {
            let mut gate = GateSession::new(lease.session(), closed.clone());
            let probe = gate.set_seen(arrival_uid, true).await;
            if probe.is_ok() {
                return Err("offline window: write succeeded while gate closed".to_string());
            }
            gate.search_uids().await.map_err(|e| e.to_string())?.len()
        };
        if before_count != uids_before.len() {
            return Err("offline window: reads did not forward while closed".to_string());
        }
        // Reopen: queue the Seen change the offline window refused, so the
        // reconnect pass has a replay to run before its sweep.
        closed.store(false, Ordering::SeqCst);
    }
    {
        let guard = store.lock().unwrap();
        queries::enqueue_outbox(guard.conn(), mailbox_id, arrival_uid, true, epoch)
            .map_err(|e| format!("enqueue seen=true: {e}"))?;
    }
    // Drop the session mid-run (lease scope ends); the next lease resumes
    // through the single SessionManager (reconnect-retry inside).
    drop(mgr.lease_for(wire).await.map_err(|e| e.to_string())?);
    check_epoch(mgr, wire, epoch).await?;
    {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("reconnect pass failed: {e}"))?;
    }
    // No silent gaps: every pre-drop UID still present, arrival converged,
    // queue drained (replay ran before the sweep — the worker logs
    // `imap replay:` at Step 4b, checked by the wrapper).
    let uids_after = search_uids_in(mgr, wire).await.map_err(|e| e.to_string())?;
    for uid in &uids_before {
        if !uids_after.contains(uid) {
            return Err(format!("silent gap: uid {uid} lost across reconnect"));
        }
    }
    {
        let guard = store.lock().unwrap();
        if pending_depth(guard.conn(), mailbox_id) != 0 {
            return Err("queue not drained after reconnect pass".to_string());
        }
        let local = queries::all_local_uids(guard.conn(), mailbox_id).map_err(|e| e.to_string())?;
        if !local.contains(&arrival_uid) {
            return Err("backfill sweep did not converge the arrival UID".to_string());
        }
    }
    if !flag_has_seen(mgr, wire, arrival_uid)
        .await
        .map_err(|e| e.to_string())?
    {
        return Err("replayed seen=true: FLAGS reads unseen after reconnect".to_string());
    }
    Ok(format!(
        "reconnect resumed: {}→{} uids, no gaps, replay drained, arrival converged",
        uids_before.len(),
        uids_after.len()
    ))
}

/// Outcome of a 12/13 citation STEP: a PASS cite (prior REPORT line
/// found) or a reasoned SKIP deferral (reason + resume pointer).
pub enum CiteOutcome {
    Pass(String),
    Skip(String, String),
}

/// One 12/13-derived citation STEP (D-30, T-26-10): cite the locked 21/22
/// runner's REPORT line from a prior-run log when the human points at one
/// via `SGE_LIVE21_LOG`/`SGE_LIVE22_LOG`, else a reasoned SKIP deferral —
/// never a re-implementation (duplicating the leg risks divergence from
/// the locked STEP semantics) and never a duplicate send (SMTP re-runs
/// would mail the user twice for one validation).
async fn live_cite_prior(
    _gate: &str,
    marker: &str,
    log_env: &str,
    resume_cmd: &str,
) -> CiteOutcome {
    let path = std::env::var(log_env)
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if path.is_empty() {
        return CiteOutcome::Skip(
            format!("no prior-run log at ${log_env} (locked 21/22 live run still pending creds)"),
            resume_cmd.to_string(),
        );
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => match citation_evidence(&text, marker) {
            Some(line) => CiteOutcome::Pass(format!("cite={path} line={line}")),
            None => CiteOutcome::Skip(
                format!("prior log {path} lacks {marker:?} (that leg never passed there)"),
                resume_cmd.to_string(),
            ),
        },
        Err(e) => CiteOutcome::Skip(
            format!("prior-run log {path} unreadable: {e}"),
            resume_cmd.to_string(),
        ),
    }
}

/// STEP 9, v1.3 confirm→MOVE→undo (T-26-07): seed a classified suggestion
/// against a sandbox message via the existing filing entries, confirm
/// through the production confirm verb (`execute_filing_move` — the same
/// entry the `confirm_suggestion` IPC command drives; `plan_filing`
/// refuses missing/stale suggestions backend-side, so a direct-IPC bypass
/// cannot smuggle an unplanned move), assert the MOVE lands in the
/// sandbox-scoped Auto/ subtree (never the real Auto/ tree), journal the
/// move on the proven batch path, run `undo_run`, and assert the message is
/// restored to its origin with the undo logged as an override row (same
/// `log_override`/`latest_override` entries the `override_label` IPC uses).
///
/// Returns the PASS detail plus the rendered subject (for the step-8 audit,
/// which checks exactly what the legs rendered).
async fn live_confirm_move_undo(
    mgr: &Arc<SessionManager>,
    store: &StoreRef,
    origin_wire: &str,
    epoch: u32,
    self_addr: &str,
    msg_id: &str,
) -> Result<(String, String), String> {
    use sge_lib::classify::taxonomy::load_default;
    assert_sandbox_prefix(origin_wire).map_err(|e| e.to_string())?;
    check_epoch(mgr, origin_wire, epoch).await?;
    // Sandbox-scoped Auto/ subtree: derived from the sandbox name, both
    // guards armed before any verb fires.
    let tax = load_default().map_err(|e| format!("taxonomy: {e}"))?;
    let (top_id, top_name) = tax
        .categories
        .iter()
        .find(|c| c.parent.is_none())
        .map(|c| (c.id.clone(), c.name.clone()))
        .ok_or_else(|| "taxonomy has no top-level category".to_string())?;
    let auto_wire = sandbox_auto_wire(origin_wire, &top_name);
    assert_never_real_auto(&auto_wire)?;
    assert_sandbox_prefix(&auto_wire).map_err(|e| e.to_string())?;
    // Seed the origin message via the prod save verb (same as gate 6).
    let subject = format!("{SUBJECT_PREFIX}{SFX_FILING}");
    audit_test_mail(&[subject.as_str()], &[self_addr], self_addr)?;
    let bytes = draft_bytes(self_addr, SFX_FILING, BODY_FILING, msg_id);
    let seed_uid = mgr
        .save_draft_copy_in(origin_wire, msg_id, &bytes, None, Some(epoch))
        .await
        .map_err(|e| format!("filing seed APPEND failed: {e}"))?;
    // CREATE the sandbox Auto/ subtree folder via the Phase 11 verb.
    // `ensure_auto_tree` is deliberately NOT called — it builds the REAL
    // Auto/ tree (a scope-creep signal per D-32); the sandbox folder is the
    // in-scope substitute and the never-real-Auto assert above proves it.
    mgr.create_mailbox_in(&auto_wire)
        .await
        .map_err(|e| format!("sandbox Auto CREATE failed: {e}"))?;
    // Mailbox rows + epochs for both wires (what sync Step 1 does).
    let (origin_id, auto_id) = {
        let guard = store.lock().unwrap();
        let origin_id =
            queries::ensure_mailbox(guard.conn(), origin_wire).map_err(|e| e.to_string())?;
        let origin_v = current_validity(mgr, origin_wire)
            .await
            .map_err(|e| format!("validity {origin_wire}: {e}"))?;
        queries::set_sync_state(guard.conn(), origin_wire, origin_v, 0)
            .map_err(|e| e.to_string())?;
        let auto_id =
            queries::ensure_mailbox(guard.conn(), &auto_wire).map_err(|e| e.to_string())?;
        let auto_v = current_validity(mgr, &auto_wire)
            .await
            .map_err(|e| format!("validity {auto_wire}: {e}"))?;
        queries::set_sync_state(guard.conn(), &auto_wire, auto_v, 0).map_err(|e| e.to_string())?;
        (origin_id, auto_id)
    };
    // Sync pass over the origin so the local rows (location, classify
    // fields) exist for the suggest→plan entry.
    {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr
            .lease_for(origin_wire)
            .await
            .map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), origin_wire, noop_cb())
            .await
            .map_err(|e| format!("filing sync pass failed: {e}"))?;
    }
    // Seed the suggestion row (same row shape the suggest engine writes),
    // then suggest→confirm through the production entries.
    let local_id: u64 = {
        let guard = store.lock().unwrap();
        let (id, uid) = resolve_local(guard.conn(), origin_id, msg_id)?;
        if uid != seed_uid {
            return Err(format!(
                "filing seed UID drift: APPEND→{seed_uid} but sync row→{uid}"
            ));
        }
        queries::upsert_label(guard.conn(), id, &top_id, None, 0.9, 0.5, false)
            .map_err(|e| e.to_string())?;
        id
    };
    let plan = {
        let guard = store.lock().unwrap();
        sge_lib::classify::filing::plan_filing(guard.conn(), local_id, &tax)
            .map_err(|e| format!("suggest plan refused: {e}"))?
    };
    let confirmed = sge_lib::classify::filing::execute_filing_move(mgr, store, &plan, &auto_wire)
        .await
        .map_err(|e| format!("confirm MOVE failed: {e}"))?;
    if !confirmed.acked {
        return Err(format!(
            "confirm MOVE stayed pending (offline?): {}",
            confirmed.detail
        ));
    }
    // Assert the MOVE landed in the sandbox Auto/ subtree: exactly one
    // Message-ID hit there, zero in the origin (wire SEARCH is ground
    // truth; local rows refresh below for the journal entries).
    for wire in [origin_wire, auto_wire.as_str()] {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("post-move sync {wire} failed: {e}"))?;
    }
    let origin_hits = search_msg_uids(mgr, origin_wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    let auto_hits = search_msg_uids(mgr, &auto_wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if !origin_hits.is_empty() || auto_hits.len() != 1 {
        return Err(format!(
            "confirm MOVE mis-homed: origin {origin_hits:?} (want []), auto {auto_hits:?} (want 1)"
        ));
    }
    // Journal the confirmed move on the proven batch path, then undo.
    let run_id = {
        let guard = store.lock().unwrap();
        let run_id = queries::create_batch_run(guard.conn()).map_err(|e| e.to_string())?;
        let (fresh_id, _) = resolve_local(guard.conn(), auto_id, msg_id)?;
        sge_lib::classify::batch::journal_intent(
            guard.conn(),
            run_id,
            fresh_id,
            origin_wire,
            &auto_wire,
            &top_id,
            0,
        )
        .map_err(|e| format!("journal intent: {e}"))?;
        sge_lib::classify::batch::journal_moved(guard.conn(), run_id, fresh_id)
            .map_err(|e| format!("journal moved: {e}"))?;
        run_id
    };
    let restored = sge_lib::classify::batch::undo_run(mgr, store, run_id)
        .await
        .map_err(|e| format!("undo failed: {e}"))?;
    if restored != 1 {
        return Err(format!("undo restored {restored} (want 1)"));
    }
    // Restored to origin (wire truth + refreshed local row), with the undo
    // decision logged as an override row — the correction targets the
    // origin folder rather than a taxonomy category, so `to_id` carries
    // the origin wire (table is free-form TEXT; temp store is deleted).
    for wire in [origin_wire, auto_wire.as_str()] {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("post-undo sync {wire} failed: {e}"))?;
    }
    let origin_hits = search_msg_uids(mgr, origin_wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    let auto_hits = search_msg_uids(mgr, &auto_wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if origin_hits.len() != 1 || !auto_hits.is_empty() {
        return Err(format!(
            "undo mis-restored: origin {origin_hits:?} (want 1), auto {auto_hits:?} (want [])"
        ));
    }
    {
        let guard = store.lock().unwrap();
        let (final_id, _) = resolve_local(guard.conn(), origin_id, msg_id)?;
        queries::log_override(guard.conn(), final_id, &top_id, origin_wire)
            .map_err(|e| e.to_string())?;
        let pinned = queries::latest_override(guard.conn(), final_id).map_err(|e| e.to_string())?;
        if pinned.as_deref() != Some(origin_wire) {
            return Err(format!(
                "override not logged (want {origin_wire:?}, got {pinned:?})"
            ));
        }
    }
    Ok((
        format!(
            "confirm→MOVE uid {seed_uid}→{} in {auto_wire} + undo restored 1 to origin (override logged)",
            auto_hits.len()
        ),
        subject,
    ))
}

/// STEP 10, v1.3 RENAME/DELETE (T-26-08): CREATE a sandbox-scoped folder via
/// the Phase 11 verb, RENAME it within the `SGE-Test-` prefix via
/// `rename_mailbox_in`, assert caches invalidate (server LIST + migrated
/// local row) and UIDs survive, then prove the backend-side guards live —
/// INBOX refusal plus non-empty refusal — empty via scoped expunge, DELETE,
/// and assert LIST clean.
///
/// Returns the PASS detail plus the rendered seed subject (step-8 audit).
async fn live_rename_delete(
    mgr: &Arc<SessionManager>,
    store: &StoreRef,
    sandbox: &str,
    epoch: u32,
    self_addr: &str,
    msg_id: &str,
) -> Result<(String, String), String> {
    assert_sandbox_prefix(sandbox).map_err(|e| e.to_string())?;
    check_epoch(mgr, sandbox, epoch).await?;
    let old_wire = format!("{sandbox}-Rename");
    let new_wire = format!("{sandbox}-Renamed");
    for wire in [&old_wire, &new_wire] {
        assert_sandbox_prefix(wire).map_err(|e| e.to_string())?;
        assert_never_real_auto(wire)?;
    }
    // Backend-side guard evidence, before any verb fires: INBOX refusal
    // (prefix guard + delete guard both refuse it) and non-empty refusal.
    if assert_sandbox_prefix("INBOX").is_ok() {
        return Err("prefix guard accepted INBOX".to_string());
    }
    if assert_folder_delete_allowed("INBOX", 0).is_ok() {
        return Err("delete guard accepted INBOX".to_string());
    }
    // CREATE the probe folder (Phase 11 verb), seed one message, sync so
    // local rows exist for the cache-migrate asserts below.
    mgr.create_mailbox_in(&old_wire)
        .await
        .map_err(|e| format!("probe CREATE failed: {e}"))?;
    let subject = format!("{SUBJECT_PREFIX}{SFX_RENAME}");
    audit_test_mail(&[subject.as_str()], &[self_addr], self_addr)?;
    let bytes = draft_bytes(self_addr, SFX_RENAME, BODY_RENAME, msg_id);
    mgr.save_draft_copy_in(&old_wire, msg_id, &bytes, None, None)
        .await
        .map_err(|e| format!("probe seed APPEND failed: {e}"))?;
    let old_id = {
        let guard = store.lock().unwrap();
        let id = queries::ensure_mailbox(guard.conn(), &old_wire).map_err(|e| e.to_string())?;
        let v = current_validity(mgr, &old_wire)
            .await
            .map_err(|e| format!("validity {old_wire}: {e}"))?;
        queries::set_sync_state(guard.conn(), &old_wire, v, 0).map_err(|e| e.to_string())?;
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(&old_wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), &old_wire, noop_cb())
            .await
            .map_err(|e| format!("probe sync pass failed: {e}"))?;
        id
    };
    // RENAME within the prefix (Phase 11 verb; UIDVALIDITY bump surfaces as
    // a loud State error, never a silent accept).
    mgr.rename_mailbox_in(&old_wire, &new_wire)
        .await
        .map_err(|e| format!("RENAME failed: {e}"))?;
    // Caches invalidate: server LIST shows new/old-gone AND the local row
    // migrates through the existing prod entry (same one the rename IPC
    // command drives).
    let after_rename = list_snapshot(mgr)
        .await
        .map_err(|e| format!("re-LIST failed: {e}"))?;
    if after_rename.iter().any(|n| n == &old_wire) || !after_rename.iter().any(|n| n == &new_wire) {
        return Err("RENAME LIST mismatch: old still present or new absent".to_string());
    }
    {
        let guard = store.lock().unwrap();
        let delimiter = mgr
            .list_mailboxes()
            .await
            .map_err(|e| e.to_string())?
            .iter()
            .find(|i| i.name == new_wire)
            .map(|i| i.delimiter.clone())
            .unwrap_or_default();
        let touched = queries::rename_mailbox_cache(guard.conn(), &old_wire, &new_wire, &delimiter)
            .map_err(|e| e.to_string())?;
        if touched == 0 {
            return Err("cache migrate touched 0 rows (old row missing)".to_string());
        }
        let rows = queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?;
        if rows.iter().any(|r| r.name == old_wire) || !rows.iter().any(|r| r.name == new_wire) {
            return Err("cache migrate mismatch: old row present or new row absent".to_string());
        }
        let _ = old_id;
    }
    // UIDs survive: the seeded Message-ID resolves under the new name.
    // Re-derive the epoch on the new wire first (rename may bump it — the
    // verb already errors loud on that; here we just track current).
    let seed_hits = search_msg_uids(mgr, &new_wire, msg_id)
        .await
        .map_err(|e| e.to_string())?;
    if seed_hits.len() != 1 {
        return Err(format!(
            "UIDs did not survive RENAME: {seed_hits:?} (want 1)"
        ));
    }
    let seed_uid = seed_hits[0];
    // Non-empty guard armed: the folder holds 1 message, so the guard must
    // refuse DELETE until a scoped expunge empties it.
    let status = mgr
        .mailbox_status(&new_wire)
        .await
        .map_err(|e| format!("STATUS failed: {e}"))?;
    if assert_folder_delete_allowed(&new_wire, status.messages).is_ok() {
        return Err("delete guard allowed a non-empty folder".to_string());
    }
    mgr.discard_server_copy_in(&new_wire, seed_uid, None)
        .await
        .map_err(|e| format!("scoped expunge failed: {e}"))?;
    let status = mgr
        .mailbox_status(&new_wire)
        .await
        .map_err(|e| format!("post-expunge STATUS failed: {e}"))?;
    assert_folder_delete_allowed(&new_wire, status.messages)
        .map_err(|e| format!("guard still refuses emptied folder: {e}"))?;
    // DELETE + cache invalidate + LIST clean (old and new both gone).
    mgr.delete_mailbox_in(&new_wire)
        .await
        .map_err(|e| format!("DELETE failed: {e}"))?;
    {
        let guard = store.lock().unwrap();
        queries::delete_mailbox_cache(guard.conn(), &new_wire).map_err(|e| e.to_string())?;
        let rows = queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?;
        if rows
            .iter()
            .any(|r| r.name == old_wire || r.name == new_wire)
        {
            return Err("cache delete mismatch: probe rows linger".to_string());
        }
    }
    let after_delete = list_snapshot(mgr)
        .await
        .map_err(|e| format!("post-DELETE LIST failed: {e}"))?;
    if after_delete
        .iter()
        .any(|n| n == &old_wire || n == &new_wire)
    {
        return Err("DELETE LIST mismatch: probe folder still present".to_string());
    }
    Ok((
        format!(
            "RENAME {old_wire}→{new_wire} (uid {seed_uid} survived, caches migrated) + guarded DELETE (INBOX/non-empty refusals proven)"
        ),
        subject,
    ))
}

/// STEP 11, v1.3 batch/chunk probe (T-26-09): bounded batch over sandbox
/// messages through the proven batch journal path — per-chunk UIDVALIDITY
/// re-check, journal-intent-before-verb, progress lines, persisted run/item
/// rows, resume-safe with zero dupes or skips — then full `undo_run`
/// restore so cleanup stays trivial.
///
/// The probe runs in its own `{sandbox}-Batch` origin (candidate scoping
/// stays exact: gate-6/8 messages live in the shared sandbox, never in
/// scope here). Filing into the shared sandbox Auto/ subtree created by
/// STEP 9 (created if a prior leg never ran).
///
/// Returns the PASS detail plus rendered subjects (step-8 audit).
async fn live_batch_probe(
    mgr: &Arc<SessionManager>,
    store: &StoreRef,
    sandbox: &str,
    epoch: u32,
    self_addr: &str,
    run_tag: &str,
) -> Result<(String, Vec<String>), String> {
    assert_sandbox_prefix(sandbox).map_err(|e| e.to_string())?;
    check_epoch(mgr, sandbox, epoch).await?;
    let origin_wire = format!("{sandbox}-Batch");
    assert_sandbox_prefix(&origin_wire).map_err(|e| e.to_string())?;
    assert_never_real_auto(&origin_wire)?;
    // Sandbox Auto/ dest: shared with STEP 9; CREATE only when absent (a
    // blind re-CREATE would NO the run when STEP 9 already built it).
    let tax = sge_lib::classify::taxonomy::load_default().map_err(|e| format!("taxonomy: {e}"))?;
    let top_name = tax
        .categories
        .iter()
        .find(|c| c.parent.is_none())
        .map(|c| c.name.clone())
        .ok_or_else(|| "taxonomy has no top-level category".to_string())?;
    let auto_wire = sandbox_auto_wire(sandbox, &top_name);
    assert_never_real_auto(&auto_wire)?;
    assert_sandbox_prefix(&auto_wire).map_err(|e| e.to_string())?;
    mgr.create_mailbox_in(&origin_wire)
        .await
        .map_err(|e| format!("batch origin CREATE failed: {e}"))?;
    let listed = list_snapshot(mgr)
        .await
        .map_err(|e| format!("LIST failed: {e}"))?;
    if !listed.iter().any(|n| n == &auto_wire) {
        mgr.create_mailbox_in(&auto_wire)
            .await
            .map_err(|e| format!("sandbox Auto CREATE failed: {e}"))?;
    }
    // Seed the bounded probe set (distinct Message-IDs, self-only audit
    // before every transport call).
    let mut msg_ids: Vec<String> = Vec::new();
    let mut subjects: Vec<String> = Vec::new();
    for i in 0..BATCH_PROBE_SEEDS {
        let mid = run_message_id(&format!("{run_tag}-batch-{i}"));
        let subject = format!("{SUBJECT_PREFIX}{SFX_BATCH}-{i}");
        audit_test_mail(&[subject.as_str()], &[self_addr], self_addr)?;
        let bytes = draft_bytes(self_addr, &format!("{SFX_BATCH}-{i}"), BODY_BATCH, &mid);
        mgr.save_draft_copy_in(&origin_wire, &mid, &bytes, None, None)
            .await
            .map_err(|e| format!("batch seed {i} APPEND failed: {e}"))?;
        msg_ids.push(mid);
        subjects.push(subject);
    }
    // Mailbox rows + sync pass so candidate collection sees local rows.
    let origin_id = {
        let guard = store.lock().unwrap();
        let id = queries::ensure_mailbox(guard.conn(), &origin_wire).map_err(|e| e.to_string())?;
        let v = current_validity(mgr, &origin_wire)
            .await
            .map_err(|e| format!("validity {origin_wire}: {e}"))?;
        queries::set_sync_state(guard.conn(), &origin_wire, v, 0).map_err(|e| e.to_string())?;
        let auto_v = current_validity(mgr, &auto_wire)
            .await
            .map_err(|e| format!("validity {auto_wire}: {e}"))?;
        let _ = queries::ensure_mailbox(guard.conn(), &auto_wire).map_err(|e| e.to_string())?;
        queries::set_sync_state(guard.conn(), &auto_wire, auto_v, 0).map_err(|e| e.to_string())?;
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr
            .lease_for(&origin_wire)
            .await
            .map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), &origin_wire, noop_cb())
            .await
            .map_err(|e| format!("batch sync pass failed: {e}"))?;
        id
    };
    // Resolve (local id, uid) per seed by RFC Message-ID — UID-only
    // addressing, freshly derived, never threaded as stale state.
    let mut items: Vec<(u64, u32)> = Vec::new();
    {
        let guard = store.lock().unwrap();
        for mid in &msg_ids {
            items.push(resolve_local(guard.conn(), origin_id, mid)?);
        }
    }
    // Chunked moves with per-chunk epoch re-check + journal-intent-before-
    // verb (crash safety) + progress lines (persisted report rows land in
    // batch_runs/batch_items via the proven entries below).
    let run_id = {
        let guard = store.lock().unwrap();
        queries::create_batch_run(guard.conn()).map_err(|e| e.to_string())?
    };
    let chunks = plan_chunks(items.len(), BATCH_PROBE_CHUNK);
    if chunks.len() < 2 {
        return Err(format!(
            "probe needs ≥2 chunks (got {} for {} items)",
            chunks.len(),
            items.len()
        ));
    }
    let mut moved = 0usize;
    for (ci, (s, e)) in chunks.iter().enumerate() {
        check_epoch(mgr, &origin_wire, epoch)
            .await
            .map_err(|err| format!("chunk {ci}: {err}"))?;
        // NOTE: the origin wire keeps its epoch across APPENDs (no folder
        // recreate); the per-chunk re-check above is the T-26-09 proof.
        for (local, uid) in &items[*s..*e] {
            {
                let guard = store.lock().unwrap();
                sge_lib::classify::batch::journal_intent(
                    guard.conn(),
                    run_id,
                    *local,
                    &origin_wire,
                    &auto_wire,
                    "batch-probe",
                    ci,
                )
                .map_err(|e| format!("chunk {ci} journal intent: {e}"))?;
            }
            let res = sge_lib::classify::filing::move_one_uid(
                mgr,
                store,
                &origin_wire,
                *uid,
                &auto_wire,
                "batch probe",
            )
            .await
            .map_err(|e| format!("chunk {ci} move uid {uid} failed: {e}"))?;
            if !res.acked {
                return Err(format!(
                    "chunk {ci} move uid {uid} stayed pending: {}",
                    res.detail
                ));
            }
            // move_one_uid lives in filing (shared by confirm/override/
            // merge/delete) — the exact prod verb, journaled above.
            {
                let guard = store.lock().unwrap();
                sge_lib::classify::batch::journal_moved(guard.conn(), run_id, *local)
                    .map_err(|e| format!("chunk {ci} journal moved: {e}"))?;
            }
            moved += 1;
        }
        println!(
            "BATCH-PROGRESS run={run_id} chunk={}/{} moved={moved}",
            ci + 1,
            chunks.len()
        );
    }
    // Resume-safe: every scoped candidate is journaled (zero dupes), and no
    // unjournaled candidate remains (zero skips). Local rows are still
    // pre-move here, so the journal — not a vacuous post-sync scope — does
    // the skipping.
    {
        let guard = store.lock().unwrap();
        let cands = sge_lib::classify::batch::collect_candidates(guard.conn(), Some(&origin_wire))
            .map_err(|e| format!("candidates: {e}"))?;
        let done = sge_lib::classify::batch::journaled_ids(guard.conn(), run_id)
            .map_err(|e| e.to_string())?;
        let remaining: Vec<u64> = cands
            .iter()
            .filter(|(mid, _, _)| !done.contains(mid))
            .map(|(mid, _, _)| *mid)
            .collect();
        if !remaining.is_empty() {
            return Err(format!("resume would skip {remaining:?} (zero-skip rule)"));
        }
        if done.len() != moved {
            return Err(format!(
                "journal covers {} but moved={moved} (zero-dupe rule)",
                done.len()
            ));
        }
    }
    // Restore everything via the proven undo (leaves cleanup trivial) and
    // assert wire truth: all seeds home, dest empty.
    let restored = sge_lib::classify::batch::undo_run(mgr, store, run_id)
        .await
        .map_err(|e| format!("probe undo failed: {e}"))?;
    if restored != moved {
        return Err(format!("probe undo restored {restored} (want {moved})"));
    }
    for wire in [&origin_wire, &auto_wire] {
        let worker = SyncWorker::new(store.clone());
        let mut lease = mgr.lease_for(wire).await.map_err(|e| e.to_string())?;
        worker
            .sync_with_borrowed(lease.session(), wire, noop_cb())
            .await
            .map_err(|e| format!("post-undo sync {wire} failed: {e}"))?;
    }
    for mid in &msg_ids {
        let home = search_msg_uids(mgr, &origin_wire, mid)
            .await
            .map_err(|e| e.to_string())?;
        let away = search_msg_uids(mgr, &auto_wire, mid)
            .await
            .map_err(|e| e.to_string())?;
        if home.len() != 1 || !away.is_empty() {
            return Err(format!(
                "probe undo mis-restored {mid}: home {home:?}, away {away:?}"
            ));
        }
    }
    Ok((
        format!(
            "batch probe: {moved} moves in {} chunks (epoch re-checked per chunk), resume 0-dupe/0-skip, undo restored {restored}",
            chunks.len()
        ),
        subjects,
    ))
}
/// STEP 8: no-unprefixed-send audit over the temp drafts table plus every
/// subject rendered in-run. This phase sends no SMTP; any queued row must
/// still be self-only, and every subject must carry the `SGE-TEST-` prefix.
async fn live_audit_no_unprefixed(
    store: &StoreRef,
    subjects: &[String],
    self_addr: &str,
) -> Result<String, String> {
    let subject_refs: Vec<&str> = subjects.iter().map(|s| s.as_str()).collect();
    audit_test_mail(&subject_refs, &[self_addr], self_addr)?;
    let guard = store.lock().unwrap();
    let conn = guard.conn();
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
                    return Err("non-self recipient fails audit (only mail to self)".to_string());
                }
            }
        }
    }
    Ok(format!(
        "{} in-run subjects prefixed, {} send rows all self-only",
        subjects.len(),
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
        return Err(format!("unparseable addrs cell fails audit: {cell:?}"));
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
    // Sandbox-scoped extra folders the v1.3 legs own (STEP 9 Auto/ subtree
    // + STEP 11 batch origin): derived here — not from leg results — so
    // cleanup deletes them even when a leg FAILs midway after CREATEing.
    let extras: Vec<String> = match sge_lib::classify::taxonomy::load_default() {
        Ok(tax) => match tax.categories.iter().find(|c| c.parent.is_none()) {
            Some(top) => vec![
                sandbox_auto_wire(&sandbox, &top.name),
                format!("{sandbox}-Batch"),
            ],
            None => vec![format!("{sandbox}-Batch")],
        },
        Err(_) => vec![format!("{sandbox}-Batch")],
    };
    // Collected per-gate rows for the single combined bundle REPORT
    // (D-38): (gate, verdict, evidence). `cleanup` prints them plus
    // NOT-RUN fill for anything missing — no silent drops.
    let mut rows: Vec<(String, String, String)> = Vec::new();

    let manager = std::sync::Arc::new(SessionManager::new(account_config(&cfg)));
    let db_path = std::env::temp_dir().join(format!(
        "sge-live26-{}-{}.sqlite",
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
    if cfg.self_addr.trim().is_empty() {
        step(1, "preflight", false, "SGE_LIVE_SELF must not be empty");
        return 1;
    }

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
            &format!(
                "sandbox present (prior failed cleanup) — {}",
                manual_cleanup_steps(&sandbox)
            ),
        );
        return 1;
    }
    if !step(
        1,
        "preflight",
        true,
        &format!("LIST {} folders, sandbox absent", before.len()),
    ) {
        return 1;
    }

    // 2. create — CREATE via create_mailbox_in; re-LIST must show exactly
    // one delta: the sandbox.
    if let Err(e) = assert_sandbox_prefix(&sandbox) {
        step(2, "create", false, &e.to_string());
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    if let Err(e) = manager.create_mailbox_in(&sandbox).await {
        step(2, "create", false, &format!("CREATE failed: {e}"));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    let after_create = match list_snapshot(&manager).await {
        Ok(names) => names,
        Err(e) => {
            step(2, "create", false, &format!("re-LIST failed: {e}"));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    };
    let delta = unexpected_list_delta(&before, &after_create, &sandbox);
    let created_ok = after_create.iter().any(|n| n == &sandbox) && delta.is_empty();
    if !step(
        2,
        "create",
        created_ok,
        &format!("sandbox created, unexpected deltas: {delta:?}"),
    ) {
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    // Seed the temp store's mailbox row + epoch (what sync Step 1 does) so
    // later replay entries find their rows.
    let epoch = match current_validity(&manager, &sandbox).await {
        Ok(v) => v,
        Err(e) => {
            step(2, "create", false, &format!("validity read failed: {e}"));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    };
    let mb_id: u64 = {
        let guard = store.lock().unwrap();
        match (|| -> Result<u64, String> {
            let id = queries::ensure_mailbox(guard.conn(), &sandbox).map_err(|e| e.to_string())?;
            queries::set_sync_state(guard.conn(), &sandbox, epoch, 0).map_err(|e| e.to_string())?;
            Ok(id)
        })() {
            Ok(id) => id,
            Err(e) => {
                step(2, "create", false, &format!("store seed failed: {e}"));
                return cleanup(
                    &manager,
                    &db_path,
                    &sandbox,
                    &extras,
                    &before,
                    &cfg.password,
                    &rows,
                )
                .await;
            }
        }
    };

    // 3. guard-empty — fresh sandbox SEARCHes empty before first APPEND.
    let empty = match search_uids_in(&manager, &sandbox).await {
        Ok(uids) => uids,
        Err(e) => {
            step(3, "guard-empty", false, &format!("SEARCH failed: {e}"));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    };
    if !step(
        3,
        "guard-empty",
        empty.is_empty(),
        &format!("sandbox holds {} uids (want 0)", empty.len()),
    ) {
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }

    // In-run subjects for the audit — pushed from the same constants the
    // legs render, so the audit checks exactly what was sent.
    let msg_seen = run_message_id("seen");
    let msg_arrival = run_message_id("arrival");
    let mut subjects: Vec<String> = Vec::new();

    // Small sequencer: FAIL prints the STEP line plus a FAIL REPORT row and
    // always runs cleanup.
    macro_rules! live_step {
        ($n:expr, $gate:expr, $expr:expr) => {
            match $expr {
                Ok(detail) => {
                    step($n, $gate, true, &detail);
                    report($gate, "PASS", &detail);
                    rows.push(($gate.to_string(), "PASS".to_string(), detail));
                }
                Err(reason) => {
                    step($n, $gate, false, &reason);
                    report($gate, "FAIL", &reason);
                    rows.push(($gate.to_string(), "FAIL".to_string(), reason));
                    return cleanup(
                        &manager,
                        &db_path,
                        &sandbox,
                        &extras,
                        &before,
                        &cfg.password,
                        &rows,
                    )
                    .await;
                }
            }
        };
    }

    // 4. seen-roundtrip — gate 6 (D-29).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(4, "seen-roundtrip", false, &e);
        report("seen-roundtrip", "FAIL", &e);
        rows.push(("seen-roundtrip".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    live_step!(
        4,
        "seen-roundtrip",
        live_seen_roundtrip(
            &manager,
            &store,
            &sandbox,
            mb_id,
            epoch,
            &cfg.self_addr,
            &msg_seen
        )
        .await
    );
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_SEEN}"));

    // 5. tree-browse-badges — gate 7 (D-29).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(5, "tree-browse-badges", false, &e);
        report("tree-browse-badges", "FAIL", &e);
        rows.push(("tree-browse-badges".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    live_step!(
        5,
        "tree-browse-badges",
        live_tree_browse_badges(&manager, &store, &sandbox, &before, epoch).await
    );

    // 6. arrival-tick — gate 8a (D-29).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(6, "arrival-tick", false, &e);
        report("arrival-tick", "FAIL", &e);
        rows.push(("arrival-tick".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    live_step!(
        6,
        "arrival-tick",
        live_arrival_tick(
            &manager,
            &store,
            &sandbox,
            mb_id,
            epoch,
            &cfg.self_addr,
            &msg_arrival
        )
        .await
    );
    subjects.push(format!("{SUBJECT_PREFIX}{SFX_ARRIVAL}"));

    // 7. reconnect-recovery — gate 8b (D-29). The arrival UID is the
    // convergence anchor: re-derived here, never threaded as stale state.
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &[]).await {
        step(7, "reconnect-recovery", false, &e);
        report("reconnect-recovery", "FAIL", &e);
        rows.push(("reconnect-recovery".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    let arrival_uid = match search_msg_uids(&manager, &sandbox, &msg_arrival).await {
        Ok(hits) if hits.len() == 1 => hits[0],
        Ok(hits) => {
            let reason = format!("arrival anchor: want 1 UID, got {hits:?}");
            step(7, "reconnect-recovery", false, &reason);
            report("reconnect-recovery", "FAIL", &reason);
            rows.push(("reconnect-recovery".to_string(), "FAIL".to_string(), reason));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
        Err(e) => {
            let reason = format!("arrival anchor SEARCH failed: {e}");
            step(7, "reconnect-recovery", false, &reason);
            report("reconnect-recovery", "FAIL", &reason);
            rows.push(("reconnect-recovery".to_string(), "FAIL".to_string(), reason));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    };
    live_step!(
        7,
        "reconnect-recovery",
        live_reconnect_recovery(&manager, &store, &sandbox, mb_id, epoch, arrival_uid).await
    );

    // 8. no-unprefixed-send — self-only + prefix audit.
    match live_audit_no_unprefixed(&store, &subjects, &cfg.self_addr).await {
        Ok(detail) => {
            step(8, "no-unprefixed-send", true, &detail);
        }
        Err(reason) => {
            step(8, "no-unprefixed-send", false, &reason);
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    }

    // 9. confirm-move-undo — v1.3 filing leg (D-29). The sandbox Auto/
    // subtree + batch origin in `extras` are cleaned by step 12 even when
    // this leg FAILs midway.
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &extras_str(&extras)).await {
        step(9, "confirm-move-undo", false, &e);
        report("confirm-move-undo", "FAIL", &e);
        rows.push(("confirm-move-undo".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    let msg_filing = run_message_id("filing");
    match live_confirm_move_undo(
        &manager,
        &store,
        &sandbox,
        epoch,
        &cfg.self_addr,
        &msg_filing,
    )
    .await
    {
        Ok((detail, subject)) => {
            step(9, "confirm-move-undo", true, &detail);
            report("confirm-move-undo", "PASS", &detail);
            rows.push(("confirm-move-undo".to_string(), "PASS".to_string(), detail));
            subjects.push(subject);
        }
        Err(reason) => {
            step(9, "confirm-move-undo", false, &reason);
            report("confirm-move-undo", "FAIL", &reason);
            rows.push(("confirm-move-undo".to_string(), "FAIL".to_string(), reason));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    }

    // 10. rename-delete — v1.3 folder leg (D-29). The probe folder is
    // deleted in-leg; the run-level LIST guard re-verifies after.
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &extras_str(&extras)).await {
        step(10, "rename-delete", false, &e);
        report("rename-delete", "FAIL", &e);
        rows.push(("rename-delete".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    let msg_rename = run_message_id("rename");
    match live_rename_delete(
        &manager,
        &store,
        &sandbox,
        epoch,
        &cfg.self_addr,
        &msg_rename,
    )
    .await
    {
        Ok((detail, subject)) => {
            step(10, "rename-delete", true, &detail);
            report("rename-delete", "PASS", &detail);
            rows.push(("rename-delete".to_string(), "PASS".to_string(), detail));
            subjects.push(subject);
        }
        Err(reason) => {
            step(10, "rename-delete", false, &reason);
            report("rename-delete", "FAIL", &reason);
            rows.push(("rename-delete".to_string(), "FAIL".to_string(), reason));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    }

    // 11. batch-probe — v1.3 chunked batch leg (D-29, T-26-09).
    if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &extras_str(&extras)).await {
        step(11, "batch-probe", false, &e);
        report("batch-probe", "FAIL", &e);
        rows.push(("batch-probe".to_string(), "FAIL".to_string(), e));
        return cleanup(
            &manager,
            &db_path,
            &sandbox,
            &extras,
            &before,
            &cfg.password,
            &rows,
        )
        .await;
    }
    match live_batch_probe(&manager, &store, &sandbox, epoch, &cfg.self_addr, "probe").await {
        Ok((detail, mut leg_subjects)) => {
            step(11, "batch-probe", true, &detail);
            report("batch-probe", "PASS", &detail);
            rows.push(("batch-probe".to_string(), "PASS".to_string(), detail));
            subjects.append(&mut leg_subjects);
        }
        Err(reason) => {
            step(11, "batch-probe", false, &reason);
            report("batch-probe", "FAIL", &reason);
            rows.push(("batch-probe".to_string(), "FAIL".to_string(), reason));
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
    }

    // 12-14. 12/13-derived citation rows (D-30, T-26-10): cite the locked
    // 21/22 runners, never re-implement, never duplicate-send. SKIP (not
    // FAIL) while the 21/22 live runs are still pending — always with a
    // reason + resume pointer, never a silent drop.
    for (n, gate, marker, log_env, resume) in [
        (
            12,
            "cite-draft-onecopy",
            CITE_DRAFT_MARKER,
            CITE_ENV_22,
            "scripts/live-22.sh",
        ),
        (
            13,
            "cite-smtp-send",
            CITE_SEND_MARKER,
            CITE_ENV_21,
            "scripts/live-21.sh",
        ),
        (
            14,
            "cite-seen-22",
            CITE_SEEN_MARKER,
            CITE_ENV_22,
            "scripts/live-22.sh",
        ),
    ] {
        if let Err(e) = guard_list_clean(&manager, &before, &sandbox, &extras_str(&extras)).await {
            step(n, gate, false, &e);
            report(gate, "FAIL", &e);
            return cleanup(
                &manager,
                &db_path,
                &sandbox,
                &extras,
                &before,
                &cfg.password,
                &rows,
            )
            .await;
        }
        match live_cite_prior(gate, marker, log_env, resume).await {
            CiteOutcome::Pass(detail) => {
                step(n, gate, true, &detail);
                report(gate, "PASS", &detail);
                rows.push((gate.to_string(), "PASS".to_string(), detail));
            }
            CiteOutcome::Skip(reason, resume_ptr) => {
                let evidence = deferral_evidence(&reason, &resume_ptr);
                step_skip(n, gate, &reason);
                report(gate, "SKIP", &evidence);
                rows.push((gate.to_string(), "SKIP".to_string(), evidence));
            }
        }
    }

    // 15. cleanup — ALWAYS runs: SQLite dump for the secret scan, in-bin
    // dump scan (SCAN-DB line), expunge leftovers (sandbox + extras),
    // DELETE the sandbox + extras with backoff (D-37), re-LIST must equal
    // the pre-run snapshot.
    cleanup(
        &manager,
        &db_path,
        &sandbox,
        &extras,
        &before,
        &cfg.password,
        &rows,
    )
    .await
}

/// DELETE-with-backoff shared by `cleanup` and `--cleanup-only` (D-37):
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
    let manager = std::sync::Arc::new(SessionManager::new(account_config(&cfg)));
    if delete_with_backoff(&manager, sandbox).await {
        let after = list_snapshot(&manager).await.unwrap_or_default();
        if !after.iter().any(|n| n == sandbox) {
            println!("CLEANUP-ONLY {sandbox}: DELETED, LIST clean");
            return 0;
        }
    }
    println!(
        "CLEANUP-ONLY {sandbox}: FAIL — {}",
        manual_cleanup_steps(sandbox)
    );
    1
}

/// Cleanup (D-37): SQLite dump for the secret scan, in-bin dump scan
/// (SCAN-DB line), expunge-all in the sandbox AND the v1.3 extras reusing
/// `discard_server_copy_in`, then DELETE sandbox + extras with retry;
/// post-LIST must equal the pre-run snapshot.
/// Prints the run REPORT (per-step verdicts live on stdout above; REPORT
/// condenses gates/list/scan-surface/cleanup).
async fn cleanup(
    mgr: &SessionManager,
    db_path: &std::path::Path,
    sandbox: &str,
    extras: &[String],
    before: &[String],
    password: &str,
    rows: &[(String, String, String)],
) -> i32 {
    let mut ok = true;
    // Dump BEFORE expunge/DELETE so the scan sees the full run state
    // (queues, drafts, sync rows). The wrapper greps this file.
    let dump_path = std::env::temp_dir().join(format!(
        "sge-live26-{}-{}.dump",
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
    // In-bin secret scan over the dump text (T-26-05, D-50): passwords only.
    // Usernames are deliberately NOT scanned here — the temp Store holds
    // self addresses, so a username substring would false-positive; the
    // wrapper re-checks log + dump for the full credential set. A password
    // substring in the dump is never legitimate (passwords live in
    // keyring/env only) and FAILs the run.
    match std::fs::read_to_string(&dump_path) {
        Ok(text) => {
            if scan_text_for_secrets(&text, &[password]).is_some() {
                println!("SCAN-DB FAIL artifact=dump (credential substring in DB dump)");
                ok = false;
            } else {
                println!("SCAN-DB PASS artifact=dump (no credential substring)");
            }
        }
        Err(e) => {
            println!("SCAN-DB FAIL artifact=dump-unreadable ({e})");
            ok = false;
        }
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
    // v1.3 extras (STEP 9 Auto/ subtree + STEP 11 batch origin): expunge
    // leftovers a leg FAIL may have stranded, then DELETE with the same
    // backoff. Absent extras are skipped WITHOUT backoff (a leg that never
    // ran must not cost 65s of retries).
    let listed_now = list_snapshot(mgr).await.unwrap_or_default();
    for extra in extras {
        if assert_sandbox_prefix(extra).is_err() {
            ok = false;
            continue;
        }
        if !listed_now.iter().any(|n| n == extra) {
            println!("CLEANUP-SKIP extra={extra} (absent — owning leg never created it)");
            continue;
        }
        let extra_left = search_uids_in(mgr, extra).await.unwrap_or_default();
        for uid in extra_left {
            if let Err(e) = mgr.discard_server_copy_in(extra, uid, None).await {
                eprintln!(
                    "[live-gate] cleanup expunge extra {extra} uid {uid} failed ({e}) — continuing"
                );
            }
        }
        if !delete_with_backoff(mgr, extra).await {
            eprintln!(
                "[live-gate] cleanup DELETE extra {extra} failed — manual steps may be needed"
            );
            ok = false;
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
        if ok && clean { "deleted" } else { "MANUAL" },
        dump_path.display()
    );
    // Single combined bundle REPORT (D-38, D-48): one row per deferred
    // gate — collected PASS/FAIL/SKIP verdicts, NOT-RUN fill for legs that
    // never ran (incomplete run, never a silent pass), plus the STARTTLS
    // documented deferral. Verdict table only: reasons plus pointers,
    // never secrets, never transcripts (T-26-11 — the wrapper greps the
    // log + dump for credential substrings and FAILs on any hit).
    println!("BUNDLE-REPORT begin gates={}", ALL_GATES.len());
    for (gate, verdict, evidence) in rows {
        println!("BUNDLE-ROW gate={gate} verdict={verdict} evidence={evidence}");
    }
    for gate in missing_gates(rows, ALL_GATES) {
        println!("BUNDLE-ROW gate={gate} verdict=NOT-RUN evidence=leg-never-ran");
    }
    println!(
        "BUNDLE-ROW gate=starttls-live verdict=SKIP evidence={}",
        deferral_evidence(STARTTLS_DEFER_REASON, STARTTLS_DEFER_RESUME)
    );
    println!(
        "BUNDLE-REPORT end list_restored={} dump={}",
        if clean { "yes" } else { "no" },
        dump_path.display()
    );
    code
}

const USAGE: &str = r#"live_gate_26 — Phase 26 sandboxed live gate (STEP 1-15, gates 6/7/8 + v1.3 legs + 12/13 citations + cleanup)

USAGE:
    live_gate_26 [--seed-only]
    live_gate_26 --cleanup-only <SGE-Test-folder>

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
                    eprintln!("usage error: --cleanup-only needs a sandbox folder name\n\n{USAGE}");
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
mod live_gate_26_preflight_tests {
    use super::*;

    #[test]
    fn live_gate_26_prefix_accepts_sandbox_names() {
        assert!(assert_sandbox_prefix("SGE-Test-123-456").is_ok());
        assert!(sandbox_name().starts_with(SANDBOX_PREFIX));
    }

    #[test]
    fn live_gate_26_prefix_refuses_real_folders() {
        for wire in ["INBOX", "Drafts", "Sent", "SGE-Test", "xSGE-Test-1", ""] {
            assert!(assert_sandbox_prefix(wire).is_err(), "must refuse {wire:?}");
        }
    }

    #[test]
    fn live_gate_26_list_delta_ignores_only_the_sandbox() {
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
    fn live_gate_26_secret_scan_finds_password_ignores_empty() {
        assert_eq!(
            scan_text_for_secrets("abc hunter2 xyz", &["hunter2"]),
            Some(0)
        );
        assert_eq!(scan_text_for_secrets("clean text", &["hunter2"]), None);
        assert_eq!(scan_text_for_secrets("anything", &[""]), None);
    }

    #[test]
    fn live_gate_26_audit_enforces_prefix_and_self_only() {
        assert!(audit_test_mail(&["SGE-TEST-x"], &["me@example.com"], "me@example.com").is_ok());
        assert!(audit_test_mail(&["no-prefix"], &["me@example.com"], "me@example.com").is_err());
        assert!(
            audit_test_mail(&["SGE-TEST-x"], &["other@example.com"], "me@example.com").is_err()
        );
    }

    #[test]
    fn live_gate_26_replay_order_predicate() {
        let lines = ["[SGE sync] imap replay: acked=1", "sweep pass done"];
        let refs: Vec<&str> = lines.to_vec();
        assert!(replay_logged_before_sweep(&refs));
        let flipped: Vec<&str> = vec!["sweep pass done", "[SGE sync] imap replay: acked=1"];
        assert!(!replay_logged_before_sweep(&flipped));
    }

    #[test]
    fn live_gate_26_rendered_draft_carries_prefix_and_parses() {
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
        assert_eq!(
            parsed.subject(),
            Some(format!("{SUBJECT_PREFIX}round-trip").as_str())
        );
        assert_eq!(parsed.body_text(0).unwrap().to_string(), "hello body");
        assert_eq!(
            parsed.message_id(),
            Some(msg_id.trim_matches(&['<', '>'][..]))
        );
    }

    #[test]
    fn live_gate_26_auto_wire_stays_in_sandbox_never_real_auto() {
        let wire = sandbox_auto_wire("SGE-Test-1-2", "Acadêmico");
        assert_eq!(wire, "SGE-Test-1-2-Auto-Acadêmico");
        // Both guards pass on the derived wire …
        assert!(assert_never_real_auto(&wire).is_ok());
        assert!(assert_sandbox_prefix(&wire).is_ok());
        // … while the real Auto/ tree is refused by the first guard.
        for real in ["Auto", "Auto/Financeiro", "Auto.Financeiro"] {
            assert!(
                assert_never_real_auto(real).is_err(),
                "must refuse real tree {real:?}"
            );
        }
    }

    #[test]
    fn live_gate_26_plan_chunks_covers_tail() {
        assert_eq!(plan_chunks(7, 3), vec![(0, 3), (3, 6), (6, 7)]);
        assert_eq!(plan_chunks(6, 3), vec![(0, 3), (3, 6)]);
        assert_eq!(plan_chunks(0, 3), vec![]);
        // Degenerate size clamps to 1 (never an infinite loop, never empty).
        assert_eq!(plan_chunks(2, 0), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn live_gate_26_folder_delete_guard_order() {
        // INBOX refused regardless of count (first rule).
        assert!(assert_folder_delete_allowed("INBOX", 0).is_err());
        assert!(assert_folder_delete_allowed("inbox", 5).is_err());
        // Non-empty refused until emptied (second rule).
        assert!(assert_folder_delete_allowed("SGE-Test-1-Renamed", 1).is_err());
        // Emptied sandbox folder proceeds.
        assert!(assert_folder_delete_allowed("SGE-Test-1-Renamed", 0).is_ok());
    }

    #[test]
    fn live_gate_26_citation_finds_marker_line() {
        let log = "noise\nSTEP 5/13 one-copy: PASS exactly-one-copy uid=42\ntail\n";
        assert_eq!(
            citation_evidence(log, "one-copy: PASS"),
            Some("STEP 5/13 one-copy: PASS exactly-one-copy uid=42".to_string())
        );
        assert_eq!(citation_evidence(log, "smtp: PASS"), None);
        assert_eq!(citation_evidence(log, "   "), None);
        assert_eq!(citation_evidence("", "one-copy: PASS"), None);
    }

    #[test]
    fn live_gate_26_deferral_rows_carry_reason_and_pointer() {
        let ev = deferral_evidence(
            "no prior-run log (21 live pending creds)",
            "scripts/live-21.sh",
        );
        assert!(is_reasoned_deferral(&ev));
        // Bare or half-filled deferrals are FAILs, not deferrals (D-47).
        assert!(!is_reasoned_deferral("reason= resume=scripts/live-21.sh"));
        assert!(!is_reasoned_deferral("reason=no creds resume="));
        assert!(!is_reasoned_deferral("reason=no creds"));
        assert!(!is_reasoned_deferral("deferred"));
        assert!(!is_reasoned_deferral(""));
    }

    #[test]
    fn live_gate_26_bundle_covers_full_deferred_set() {
        // ALL_GATES is exactly the full deferred set (v1.1 6/7/8, v1.2
        // 12/13, v1.3 confirm→MOVE + RENAME/DELETE + batch) — no dupes,
        // no silent drops.
        let mut sorted = ALL_GATES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ALL_GATES.len(), "bundle gate list has dupes");
        for gate in [
            "seen-roundtrip",
            "tree-browse-badges",
            "arrival-tick",
            "reconnect-recovery",
            "confirm-move-undo",
            "rename-delete",
            "batch-probe",
            "cite-draft-onecopy",
            "cite-smtp-send",
            "cite-seen-22",
        ] {
            assert!(ALL_GATES.contains(&gate), "bundle drops {gate}");
        }
    }

    #[test]
    fn live_gate_26_missing_gates_flags_unrun() {
        let collected = vec![(
            "seen-roundtrip".to_string(),
            "PASS".to_string(),
            "d".to_string(),
        )];
        let missing = missing_gates(&collected, ALL_GATES);
        assert!(missing.contains(&"batch-probe"));
        assert!(!missing.contains(&"seen-roundtrip"));
        assert_eq!(missing_gates(&[], ALL_GATES).len(), ALL_GATES.len());
        // Fully collected → nothing missing (complete run).
        let full: Vec<(String, String, String)> = ALL_GATES
            .iter()
            .map(|g| (g.to_string(), "PASS".to_string(), "d".to_string()))
            .collect();
        assert!(missing_gates(&full, ALL_GATES).is_empty());
    }

    #[test]
    fn live_gate_26_starttls_deferral_is_reasoned() {
        assert!(is_reasoned_deferral(&deferral_evidence(
            STARTTLS_DEFER_REASON,
            STARTTLS_DEFER_RESUME
        )));
    }
}
