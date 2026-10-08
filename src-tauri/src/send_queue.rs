//! Durable send queue: enqueue/dedupe path + unit-testable state machine
//! (Phase 13, Plan 13-01).
//!
//! No network I/O lives here — SMTP transport lands in Plan 13-02, Tauri
//! commands in Plan 13-03. This module proves the exactly-once foundation:
//!
//! - Message-ID assigned once at enqueue (`message_id` UNIQUE dedupes
//!   double-invokes; retries resend the identical `.eml` bytes, never
//!   re-render — T-13-04).
//! - Rendered bytes are immutable after write (`<app_data>/outbox/<id>.eml`).
//! - Crash recovery flips `sending` → `queued` before any flush pass.
//! - BCC is envelope-only (never in headers); 25 MB cap enforced on the
//!   rendered bytes before any file write (T-13-03).
//! - Header-injection guard reuses the `drafts.rs` discipline: the renderer
//!   strips CR/LF from every header field (T-13-02).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::Utc;
use rusqlite::Connection;
use serde::Serialize;
use thiserror::Error;

use crate::store::queries::{
    self, SEND_STATE_FAILED, SEND_STATE_QUEUED, SEND_STATE_SENDING, SEND_STATE_SENT,
    SEND_STATE_UNCERTAIN, SendRow,
};
use crate::store::{StoreError, StoreResult};

/// Backoff base: first retry waits ~30 s, doubling per attempt.
pub const SEND_BACKOFF_BASE_SECS: i64 = 30;
/// Backoff cap: waits never exceed ~15 min (before jitter).
pub const SEND_BACKOFF_CAP_SECS: i64 = 900;
/// Jitter band: ±20 % around the nominal wait (prevents retry thundering).
pub const SEND_JITTER_PCT: f64 = 0.2;
/// Send cap: rendered bytes over 25 MiB are refused at enqueue with a
/// plain-language error (T-13-03).
pub const SEND_MAX_BYTES: usize = 25 * 1024 * 1024;

/// Queue lifecycle states (mirrors the `send_queue.state` CHECK).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SendState {
    Queued,
    Sending,
    Sent,
    Failed,
    Uncertain,
}

impl SendState {
    /// Stored spelling (matches the SQL CHECK values).
    pub fn as_str(self) -> &'static str {
        match self {
            SendState::Queued => SEND_STATE_QUEUED,
            SendState::Sending => SEND_STATE_SENDING,
            SendState::Sent => SEND_STATE_SENT,
            SendState::Failed => SEND_STATE_FAILED,
            SendState::Uncertain => SEND_STATE_UNCERTAIN,
        }
    }
}

/// Single-flight guard for flush passes (Phase 13).
///
/// Separate from [`crate::sync::SyncGate`] on purpose: a send flush must
/// never hold the IMAP session open while waiting, and a sync sweep must
/// never block a retry. At most one flush runs at a time — a second caller
/// gets `None` from [`try_begin`](SendGate::try_begin) and must skip, never
/// overlap (double-flush would risk double-SMTP-send).
pub struct SendGate {
    in_flight: AtomicBool,
}

impl Default for SendGate {
    fn default() -> Self {
        Self {
            in_flight: AtomicBool::new(false),
        }
    }
}

impl SendGate {
    /// Begin a flush pass. Returns `None` while another pass is in flight.
    pub fn try_begin(&self) -> Option<SendGuard<'_>> {
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(SendGuard { gate: self })
        } else {
            None
        }
    }
}

/// RAII release for [`SendGate`]: drop ends the flush pass.
pub struct SendGuard<'a> {
    gate: &'a SendGate,
}

impl Drop for SendGuard<'_> {
    fn drop(&mut self) {
        self.in_flight_store();
    }
}

impl SendGuard<'_> {
    fn in_flight_store(&self) {
        self.gate.in_flight.store(false, Ordering::SeqCst);
    }
}

/// Typed enqueue errors. Command-facing variants carry the frozen
/// `send-*` prefix verbatim (Plan 13-03 exposes these strings unchanged).
#[derive(Debug, Error)]
pub enum SendQueueError {
    /// Rendered bytes exceed the 25 MB cap — plain language, no secrets.
    #[error("{0}")]
    TooLarge(String),
    /// No recipient across To/Cc/Bcc.
    #[error("{0}")]
    NoRecipient(String),
    /// Unknown queue id (or unknown draft id at enqueue).
    #[error("{0}")]
    Missing(String),
    /// Row exists in a state the transition does not allow.
    #[error("{0}")]
    Refused(String),
    #[error("database error: {0}")]
    Store(#[from] StoreError),
    #[error("outbox file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("address list error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type SendQueueResult<T> = Result<T, SendQueueError>;

/// `CURRENT_TIMESTAMP`-shaped UTC now (`%Y-%m-%d %H:%M:%S`) — the same shape
/// as SQLite `datetime('now')`, so lexicographic `next_retry_at <= now`
/// comparisons in [`queries::list_due_sends`] are chronological.
pub fn utc_now_sql() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// RFC 2822 date for the rendered `Date:` header (renderer stays pure, so
/// the clock is read here at the enqueue boundary).
pub fn rfc2822_now() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S +0000").to_string()
}

static QUEUE_ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Fresh queue id (`sq-<nanos>-<seq>`). Uniqueness within a process comes
/// from the sequence; across restarts from nanos — collisions would hit the
/// `id` PRIMARY KEY and surface as a loud store error, never a silent merge.
pub fn new_queue_id() -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let seq = QUEUE_ID_SEQ.fetch_add(1, Ordering::SeqCst);
    format!("sq-{nanos:x}-{seq:x}")
}

/// Filesystem home of the immutable renders: `<app_data>/outbox/<id>.eml`.
pub fn outbox_eml_path(app_data: &Path, id: &str) -> PathBuf {
    app_data.join("outbox").join(format!("{id}.eml"))
}

/// Nominal backoff bounds (seconds) for `attempts` post-increment failures:
/// `min(cap, base * 2^attempts)` widened by ±20 % jitter. Returns `(lo, hi)`
/// with `lo >= 1` so a zero-wait retry can never hot-loop.
pub fn backoff_bounds(attempts: u32) -> (i64, i64) {
    let shift = attempts.min(10);
    let nominal = SEND_BACKOFF_BASE_SECS
        .saturating_mul(1i64 << shift)
        .min(SEND_BACKOFF_CAP_SECS);
    let lo = ((nominal as f64) * (1.0 - SEND_JITTER_PCT)) as i64;
    let hi = ((nominal as f64) * (1.0 + SEND_JITTER_PCT)) as i64;
    (lo.max(1), hi.max(1))
}

/// Pure delay picker: `jitter_roll` in `[0, 1)` selects inside the bounds.
/// Split out so tests can pin the roll; production rolls from the clock
/// (no new `rand` dependency per T-13-SC).
pub fn backoff_delay_for_attempt(attempts: u32, jitter_roll: f64) -> i64 {
    let (lo, hi) = backoff_bounds(attempts);
    let roll = jitter_roll.clamp(0.0, 1.0);
    lo + ((hi - lo) as f64 * roll) as i64
}

/// Clock-derived roll in `[0, 1)` (xorshift over current nanos — uniform
/// enough for retry spreading, never a security use).
fn clock_roll() -> f64 {
    let mut x = (Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    ((x % 1000) as f64) / 1000.0
}

/// `next_retry_at` (SQL-timestamp shape) for a failure that just raised the
/// row to `attempts` post-increment failures.
pub fn compute_next_retry_at(post_attempts: u32, now: &chrono::DateTime<Utc>) -> String {
    let delay = backoff_delay_for_attempt(post_attempts, clock_roll());
    (*now + chrono::Duration::seconds(delay))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Caller-supplied mail for one enqueue.
///
/// Frozen command contract (Plan 13-03 exposes verbatim): `queue_send`
/// takes `{from, to[], cc[], bcc[], subject, body, draft_id?}` — exactly
/// these fields — and returns `{queue_id, message_id, state, pending_count}`.
/// Error strings keep their prefixes verbatim: `send-too-large`,
/// `send-no-recipient`, `send-missing` (unknown `draft_id` at enqueue),
/// matching the `drafts-missing` precedent from Phase 12.
#[derive(Debug, Clone)]
pub struct EnqueueInput {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// When set, the Message-ID is reused from the drafts row (DRAFT-03
    /// link) so the send transaction and the draft reconcile key on the
    /// same ID; otherwise a fresh `<...@sge.local>` is minted.
    pub draft_id: Option<String>,
}

/// Enqueue result. `deduped` is true when the Message-ID was already queued
/// (double-invoke — no second row, no second `.eml` write).
///
/// Frozen command contract: `queue_send` returns `{queue_id, message_id,
/// state, pending_count}` — the command layer builds `pending_count` from
/// [`send_status_snapshot`] (`.pending_count`); `deduped` stays internal
/// (the double-click case is indistinguishable from success by design).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnqueueOutcome {
    pub queue_id: String,
    pub message_id: String,
    pub state: String,
    pub deduped: bool,
}

fn outcome_of(row: &SendRow, deduped: bool) -> EnqueueOutcome {
    EnqueueOutcome {
        queue_id: row.id.clone(),
        message_id: row.message_id.clone(),
        state: row.state.clone(),
        deduped,
    }
}

fn is_unique_violation(e: &StoreError) -> bool {
    match e {
        StoreError::Sql(rusqlite::Error::SqliteFailure(code, msg)) => {
            code.extended_code == rusqlite::ErrorCode::ConstraintViolation as i32
                || msg.as_deref().unwrap_or_default().contains("UNIQUE")
        }
        StoreError::Sql(rusqlite::Error::ToSqlConversionFailure(_)) => false,
        StoreError::Sql(other) => other.to_string().contains("UNIQUE"),
        StoreError::Migration(_) | StoreError::Path(_) => false,
    }
}

/// Queue one outgoing mail: validate → resolve Message-ID → dedupe-check →
/// render → cap-check → write immutable `.eml` → insert row.
///
/// Ordering is the safety argument (T-13-03/T-13-04): the 25 MB cap is
/// enforced on the rendered bytes *before* any file write, and the row
/// insert comes last, so a refusal leaves neither a partial row nor a
/// partial file behind. BCC rides the envelope only — it is never rendered
/// into headers.
pub fn enqueue_send(
    conn: &Connection,
    app_data: &Path,
    input: EnqueueInput,
) -> SendQueueResult<EnqueueOutcome> {
    let to: Vec<String> = input
        .to
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let cc: Vec<String> = input
        .cc
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let bcc: Vec<String> = input
        .bcc
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(SendQueueError::NoRecipient(
            "send-no-recipient: add at least one recipient before queuing — \
             the message was not queued and nothing was saved"
                .to_string(),
        ));
    }

    let queue_id = new_queue_id();
    let message_id = match &input.draft_id {
        Some(draft_id) => match queries::get_draft(conn, draft_id)? {
            Some(draft) => draft.message_id,
            None => {
                return Err(SendQueueError::Missing(format!(
                    "send-missing: unknown draft '{draft_id}' — \
                     the message was not queued and nothing was saved"
                )));
            }
        },
        None => crate::drafts::new_message_id(&queue_id),
    };

    // Double-invoke dedupe: same Message-ID already queued → return it.
    if let Some(existing) = queries::get_send_row_by_message_id(conn, &message_id)? {
        return Ok(outcome_of(&existing, true));
    }

    let fields = crate::drafts::DraftFields {
        from: input.from.clone(),
        to: to.clone(),
        cc: cc.clone(),
        // Envelope-only: BCC never reaches the headers.
        bcc: Vec::new(),
        subject: input.subject.clone(),
        body: input.body.clone(),
        message_id: message_id.clone(),
    };
    let rendered = crate::drafts::render_draft_rfc5322(&fields, &rfc2822_now());
    if rendered.len() > SEND_MAX_BYTES {
        let mb = rendered.len() as f64 / (1024.0 * 1024.0);
        return Err(SendQueueError::TooLarge(format!(
            "send-too-large: this message is {mb:.1} MB, over the 25 MB send \
             limit — shorten the body or remove attachments, then try again \
             (nothing was queued)"
        )));
    }

    let eml_path = outbox_eml_path(app_data, &queue_id);
    if let Some(parent) = eml_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&eml_path, &rendered)?;

    let insert_result = queries::enqueue_send_row(
        conn,
        &queue_id,
        &message_id,
        &input.from,
        &serde_json::to_string(&to)?,
        &serde_json::to_string(&cc)?,
        &serde_json::to_string(&bcc)?,
        &eml_path.to_string_lossy(),
        input.draft_id.as_deref(),
    );
    match insert_result {
        Ok(()) => {
            let row = queries::get_send_row(conn, &queue_id)?.ok_or_else(|| {
                SendQueueError::Refused(
                    "send-missing: queued row vanished right after insert — \
                     retry the enqueue"
                        .to_string(),
                )
            })?;
            Ok(outcome_of(&row, false))
        }
        Err(e) => {
            // Lost a concurrent double-enqueue race: drop our orphan file and
            // reconcile to the winner instead of failing the enqueue.
            let _ = std::fs::remove_file(&eml_path);
            if is_unique_violation(&e) {
                if let Some(existing) = queries::get_send_row_by_message_id(conn, &message_id)? {
                    return Ok(outcome_of(&existing, true));
                }
            }
            Err(SendQueueError::Store(e))
        }
    }
}

/// Record one failed send attempt: `attempts + 1`, schedule the next retry
/// with capped+jittered backoff, stash the plain-language error. Returns the
/// row's new attempt count. Never touches `state` — promotion to terminal
/// `failed` (or `uncertain`) is an explicit separate step.
pub fn record_send_failure(
    conn: &Connection,
    id: &str,
    last_error: &str,
) -> StoreResult<i64> {
    let row = queries::get_send_row(conn, id)?.ok_or_else(|| {
        StoreError::Path(format!("send row '{id}' not found for attempt recording"))
    })?;
    let post_attempts = (row.attempts + 1).max(1) as u32;
    let next = compute_next_retry_at(post_attempts, &Utc::now());
    queries::record_send_attempt(conn, id, Some(&next), Some(last_error))?;
    Ok(row.attempts + 1)
}

/// Park a row as terminal `failed`: records the attempt (backoff schedule
/// kept for display) and moves to `failed`, which `list_due_sends` never
/// returns — only a manual retry re-queues it. `next_retry_at` is NOT
/// cleared, so the failed-retry surface can show when the last attempt ran.
pub fn mark_send_failed(conn: &Connection, id: &str, last_error: &str) -> StoreResult<()> {
    record_send_failure(conn, id, last_error)?;
    queries::set_send_state(conn, id, SEND_STATE_FAILED)?;
    Ok(())
}

/// Manual retry: `failed` → `queued` with a cleared schedule (due
/// immediately). Any other state is a no-op `false` — retry never yanks a
/// row out of `sending`/`uncertain` (those have a verdict or an owner).
///
/// Frozen command contract: `retry_send({queue_id})` returns `{queue_id,
/// state}` and only transitions `failed` → `queued`; unknown `queue_id`
/// surfaces `send-missing: unknown send '<id>' — it may already be sent`
/// (never a raw SQL error).
pub fn requeue_failed(conn: &Connection, id: &str) -> StoreResult<bool> {
    let row = queries::get_send_row(conn, id)?;
    match row {
        Some(r) if r.state == SEND_STATE_FAILED => {
            queries::record_send_attempt(conn, id, None, r.last_error.as_deref())?;
            queries::set_send_state(conn, id, SEND_STATE_QUEUED)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Launch-time crash recovery: flip stranded `sending` rows back to `queued`.
///
/// MUST run before any flush pass (same pre-sweep replay discipline as the
/// Phase 10 outbox). Returns the flipped count for the launch log.
pub fn crash_recover(conn: &Connection) -> StoreResult<usize> {
    queries::reset_sending_to_queued(conn)
}

/// Outbox status snapshot for the badge + failed-retry surface.
///
/// Frozen command contract: `send_status()` returns `{queued, sending,
/// failed, uncertain, sent_unfiled, pending_count}` verbatim — this struct
/// serializes to exactly that shape. `sent_unfiled` (SMTP succeeded but the
/// Sent APPEND did not) is defined as [`crate::sync::worker::FlushOutcome`]
/// by Plan 13-02 but produced only by Plan 13-03's APPEND leg (with the
/// `sent_unfiled` CHECK-extension migration, M11); until then it reads 0.
/// `pending_count` (`queued + sending`) is the badge number and the
/// `queue_send` fourth field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SendStatusSnapshot {
    pub queued: i64,
    pub sending: i64,
    pub failed: i64,
    pub uncertain: i64,
    pub sent_unfiled: i64,
    pub pending_count: i64,
}

/// Read the current outbox depths in one scan (no network, no clock).
pub fn send_status_snapshot(conn: &Connection) -> StoreResult<SendStatusSnapshot> {
    let c = queries::send_state_counts(conn)?;
    Ok(SendStatusSnapshot {
        queued: c.queued,
        sending: c.sending,
        failed: c.failed,
        uncertain: c.uncertain,
        // Plan 13-03 scope (APPEND leg + M11 CHECK migration): no writer
        // exists yet, always 0. Plan 13-02 defines the SentUnfiled outcome
        // the APPEND leg will produce.
        sent_unfiled: 0,
        pending_count: c.pending(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sge-sendq-{}-{}-{}",
            std::process::id(),
            name,
            QUEUE_ID_SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn input(to: &str) -> EnqueueInput {
        EnqueueInput {
            from: "eu@utfpr.edu.br".to_string(),
            to: vec![to.to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "Oi".to_string(),
            body: "corpo".to_string(),
            draft_id: None,
        }
    }

    #[test]
    fn send_gate_second_begin_skips_while_in_flight() {
        let gate = SendGate::default();
        let _first = gate.try_begin().expect("first flush begins");
        assert!(
            gate.try_begin().is_none(),
            "concurrent flush must skip, never overlap"
        );
    }

    #[test]
    fn send_gate_reopens_after_release() {
        let gate = SendGate::default();
        {
            let _pass = gate.try_begin().expect("pass begins");
            assert!(gate.try_begin().is_none());
        }
        assert!(
            gate.try_begin().is_some(),
            "next flush after pass end must proceed"
        );
    }

    #[test]
    fn enqueue_one_mail_writes_row_plus_immutable_eml() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("enqueue-one");
        let out = enqueue_send(conn, &app_data, input("amigo@example.com")).unwrap();
        assert!(!out.deduped);
        assert_eq!(out.state, SEND_STATE_QUEUED);

        let row = queries::get_send_row(conn, &out.queue_id)
            .unwrap()
            .expect("row must exist");
        assert_eq!(row.message_id, out.message_id);
        assert_eq!(row.attempts, 0);

        let eml = Path::new(&row.eml_path);
        assert!(eml.is_file(), ".eml must exist at {}", row.eml_path);
        let bytes_first = std::fs::read(eml).unwrap();
        // Immutable: a second read returns identical bytes (no path rewrites).
        let bytes_second = std::fs::read(eml).unwrap();
        assert_eq!(bytes_first, bytes_second);
        assert!(
            String::from_utf8_lossy(&bytes_first).contains("corpo"),
            "rendered body must survive the round trip"
        );
        std::fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn enqueue_without_recipient_refuses_with_prefix() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("no-recipient");
        let mut bad = input("amigo@example.com");
        bad.to = vec![];
        let err = enqueue_send(conn, &app_data, bad).unwrap_err();
        assert!(
            err.to_string().starts_with("send-no-recipient"),
            "unexpected: {err}"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM send_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "refusal must leave no partial row");
        std::fs::remove_dir_all(&app_data).ok();
    }

    fn seed_draft(conn: &Connection, draft_id: &str, message_id: &str) {
        let mb = queries::ensure_mailbox(conn, "INBOX").unwrap();
        queries::upsert_draft(
            conn,
            draft_id,
            mb,
            message_id,
            "Assunto",
            "corpo do rascunho",
            "amigo@example.com",
            "",
            "",
        )
        .unwrap();
    }

    fn outbox_file_count(app_data: &Path) -> usize {
        std::fs::read_dir(app_data.join("outbox"))
            .map(|rd| rd.count())
            .unwrap_or(0)
    }

    #[test]
    fn double_enqueue_same_message_id_returns_existing_row() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("dedupe");
        seed_draft(conn, "draft-dedupe", "<dedupe-1@sge.local>");

        let with_draft = || EnqueueInput {
            from: "eu@utfpr.edu.br".to_string(),
            to: vec!["amigo@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "Assunto".to_string(),
            body: "corpo do rascunho".to_string(),
            draft_id: Some("draft-dedupe".to_string()),
        };
        let first = enqueue_send(conn, &app_data, with_draft()).unwrap();
        assert!(!first.deduped);
        assert_eq!(first.message_id, "<dedupe-1@sge.local>");
        let second = enqueue_send(conn, &app_data, with_draft()).unwrap();
        assert!(second.deduped, "second invoke must dedupe");
        assert_eq!(second.queue_id, first.queue_id);
        assert_eq!(second.message_id, first.message_id);

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM send_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "dedupe must leave exactly one row");
        assert_eq!(
            outbox_file_count(&app_data),
            1,
            "dedupe must not write a second .eml"
        );
        std::fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn crash_recovery_resets_only_sending() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let states = [
            SEND_STATE_QUEUED,
            SEND_STATE_SENDING,
            SEND_STATE_SENDING,
            SEND_STATE_SENT,
            SEND_STATE_FAILED,
            SEND_STATE_UNCERTAIN,
        ];
        for (i, state) in states.iter().enumerate() {
            let id = format!("sq-crash-{i}");
            queries::enqueue_send_row(
                conn,
                &id,
                &format!("<crash-{i}@sge.local>"),
                "eu@utfpr.edu.br",
                "[\"a@x.com\"]",
                "[]",
                "[]",
                &format!("/tmp/{id}.eml"),
                None,
            )
            .unwrap();
            if *state != SEND_STATE_QUEUED {
                queries::set_send_state(conn, &id, state).unwrap();
            }
        }

        let flipped = crash_recover(conn).unwrap();
        assert_eq!(flipped, 2, "exactly the two sending rows flip");

        let state_of = |i: usize| {
            queries::get_send_row(conn, &format!("sq-crash-{i}"))
                .unwrap()
                .unwrap()
                .state
        };
        assert_eq!(state_of(0), SEND_STATE_QUEUED);
        assert_eq!(state_of(1), SEND_STATE_QUEUED);
        assert_eq!(state_of(2), SEND_STATE_QUEUED);
        assert_eq!(state_of(3), SEND_STATE_SENT, "sent untouched");
        assert_eq!(state_of(4), SEND_STATE_FAILED, "failed untouched");
        assert_eq!(state_of(5), SEND_STATE_UNCERTAIN, "uncertain untouched");
    }

    #[test]
    fn backoff_stays_inside_jitter_bounds_and_caps() {
        // Edge rolls pin the band; production rolls fall strictly inside.
        for attempts in [0u32, 1, 2, 5, 8, 12] {
            let (lo, hi) = backoff_bounds(attempts);
            assert!(lo >= 1 && hi >= lo);
            assert_eq!(backoff_delay_for_attempt(attempts, 0.0), lo);
            assert_eq!(backoff_delay_for_attempt(attempts, 1.0), hi);
            let mid = backoff_delay_for_attempt(attempts, 0.5);
            assert!((lo..=hi).contains(&mid));
        }
        // Growth: 30s base doubling (attempts are post-increment failures).
        assert_eq!(backoff_bounds(0), (24, 36));
        assert_eq!(backoff_bounds(1), (48, 72));
        // Cap: 15 min nominal, ±20 % jitter → never above 1080 s.
        let (lo, hi) = backoff_bounds(100);
        assert_eq!((lo, hi), (720, 1080));
        for _ in 0..50 {
            let d = backoff_delay_for_attempt(100, clock_roll());
            assert!((720..=1080).contains(&d), "capped delay {d} out of band");
        }
    }

    #[test]
    fn oversize_render_refuses_with_plain_language_limit() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("oversize");
        let mut big = input("amigo@example.com");
        big.body = "x".repeat(SEND_MAX_BYTES + 1);
        let err = enqueue_send(conn, &app_data, big).unwrap_err();
        let text = err.to_string();
        assert!(
            text.starts_with("send-too-large"),
            "unexpected prefix: {text}"
        );
        assert!(
            text.contains("25 MB"),
            "refusal must name the limit in plain language: {text}"
        );
        for secret in ["amigo@example.com", "Oi", "xxx"] {
            assert!(
                !text.contains(secret),
                "refusal must not leak content: {text}"
            );
        }
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM send_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "oversize refusal must leave no row");
        assert_eq!(
            outbox_file_count(&app_data),
            0,
            "oversize refusal must leave no file"
        );
        std::fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn bcc_envelope_only_never_in_headers() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("bcc");
        let mut with_bcc = input("amigo@example.com");
        with_bcc.cc = vec!["copia@example.com".to_string()];
        with_bcc.bcc = vec!["oculta@example.com".to_string()];
        let out = enqueue_send(conn, &app_data, with_bcc).unwrap();
        assert!(!out.deduped);

        let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
        let envelope_bcc: Vec<String> = serde_json::from_str(&row.bcc_addrs).unwrap();
        assert_eq!(envelope_bcc, vec!["oculta@example.com".to_string()]);

        let raw = std::fs::read(Path::new(&row.eml_path)).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            !text.lines().any(|l| l.to_ascii_lowercase().starts_with("bcc:")),
            "rendered headers must never carry Bcc:\n{text}"
        );
        assert!(
            text.contains("oculta@example.com") == false,
            "BCC address must not appear anywhere in the bytes"
        );
        let msg = mail_parser::MessageParser::new()
            .parse(&raw)
            .expect("rendered bytes must parse");
        assert_eq!(
            msg.bcc().map(|a| a.iter().count()).unwrap_or(0),
            0,
            "parsed message must expose no BCC"
        );
        std::fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn header_injection_bcc_line_stripped() {
        // T-13-02: a hostile subject must not smuggle a second header.
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("injection");
        let mut hostile = input("vitima@example.com");
        hostile.subject = "Oi\r\nBcc: evil@example.com".to_string();
        let out = enqueue_send(conn, &app_data, hostile).unwrap();
        let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
        let raw = std::fs::read(Path::new(&row.eml_path)).unwrap();
        let msg = mail_parser::MessageParser::new()
            .parse(&raw)
            .expect("rendered bytes must parse");
        assert_eq!(
            msg.bcc().map(|a| a.iter().count()).unwrap_or(0),
            0,
            "injected Bcc header must not survive"
        );
        assert_eq!(msg.subject(), Some("OiBcc: evil@example.com"));
        std::fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn failure_attempt_schedules_inside_bounds_and_failed_is_terminal() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("attempt");
        let out = enqueue_send(conn, &app_data, input("amigo@example.com")).unwrap();

        let before = Utc::now();
        let attempts = record_send_failure(conn, &out.queue_id, "connection reset").unwrap();
        let after = Utc::now();
        assert_eq!(attempts, 1);
        let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
        assert_eq!(row.attempts, 1);
        assert_eq!(row.last_error.as_deref(), Some("connection reset"));
        // attempts=1 → nominal 60 s ±20 % → next_retry_at in [before+48s, after+72s].
        let scheduled = row.next_retry_at.clone().unwrap();
        let lo = (before + chrono::Duration::seconds(48))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let hi = (after + chrono::Duration::seconds(72))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(
            scheduled >= lo && scheduled <= hi,
            "next_retry_at {scheduled} outside [{lo}, {hi}]"
        );

        // Terminal failed: kept schedule for display, never due-listed.
        mark_send_failed(conn, &out.queue_id, "message rejected").unwrap();
        let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
        assert_eq!(row.state, SEND_STATE_FAILED);
        assert!(
            row.next_retry_at.is_some(),
            "terminal failed keeps its schedule (display only)"
        );
        assert!(
            list_due(&out.queue_id, conn).is_empty(),
            "failed must never appear in the due list"
        );

        // Manual retry re-queues due-immediately; non-failed retry is a no-op.
        assert!(requeue_failed(conn, &out.queue_id).unwrap());
        let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
        assert_eq!(row.state, SEND_STATE_QUEUED);
        assert_eq!(row.next_retry_at, None);
        assert_eq!(list_due(&out.queue_id, conn).len(), 1);
        assert!(
            !requeue_failed(conn, &out.queue_id).unwrap(),
            "retry on a queued row must not yank it"
        );
        std::fs::remove_dir_all(&app_data).ok();
    }

    fn list_due(id: &str, conn: &Connection) -> Vec<SendRow> {
        queries::list_due_sends(conn, "2999-01-01 00:00:00")
            .unwrap()
            .into_iter()
            .filter(|r| r.id == id)
            .collect()
    }

    #[test]
    fn status_snapshot_reports_frozen_shape() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let app_data = test_dir("status");
        let a = enqueue_send(conn, &app_data, input("a@example.com")).unwrap();
        let b = enqueue_send(conn, &app_data, input("b@example.com")).unwrap();
        mark_send_failed(conn, &b.queue_id, "refused").unwrap();

        let snap = send_status_snapshot(conn).unwrap();
        assert_eq!(snap.queued, 1);
        assert_eq!(snap.sending, 0);
        assert_eq!(snap.failed, 1);
        assert_eq!(snap.uncertain, 0);
        assert_eq!(snap.sent_unfiled, 0, "Plan 13-03 fills this");
        assert_eq!(snap.pending_count, 1);
        // Serializes to exactly the frozen send_status() keys.
        let json = serde_json::to_value(&snap).unwrap();
        for key in [
            "queued",
            "sending",
            "failed",
            "uncertain",
            "sent_unfiled",
            "pending_count",
        ] {
            assert!(json.get(key).is_some(), "missing key {key}");
        }
        let _ = a;
        std::fs::remove_dir_all(&app_data).ok();
    }
}
