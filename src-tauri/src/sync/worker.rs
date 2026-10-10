//! Sync worker: the 7-step header sweep + body fetch engine.
//!
//! Algorithm (Plan 02-02, Wave 2; extended Phase 7–9):
//! 1. Ensure mailbox row + read sync state
//! 2. SELECT mailbox → MailboxSummary (UIDVALIDITY, UIDNEXT, exists)
//! 2b. STATUS triage signals per folder (Phase 7, graceful)
//! 3. UIDVALIDITY guard — wipe if changed (per folder, Phase 7)
//! 4. Compute fetch set: SEARCH ALL minus tombstoned; convergence
//!    shortcut skips the sweep when the UID set is unchanged (Phase 9)
//! 5. Header sweep — 200-UID batches via fetch_envelopes, with in-pass
//!    range-diff re-fetch of partial drops + strike counting (Phase 9)
//! 6. Expunge diff — delete UIDs not on server (tombstoned join live set)
//! 7. Write sync state + logout
//!
//! The Store is behind an `Arc<Mutex<>>` so it can be shared
//! safely between the Tauri command thread and the sync worker.
//! The mutex is held only during synchronous DB operations,
//! never across `.await` points (IMAP fetches).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::imap::headers::MessageHeader;
use crate::imap::manager::{run_move_on_session, save_draft_on_session};
use crate::imap::roles::{resolve_roles, Role};
use crate::imap::{MailboxInfo, MailboxSummary, SyncError, SyncSession};
use crate::send_queue::{crash_recover, utc_now_sql, SendGate};
use crate::smtp::{SmtpAccount, SmtpOutcome, SmtpTransport};
use crate::store::queries;
use crate::store::queries::{SEND_STATE_QUEUED, SEND_STATE_SENDING, SEND_STATE_SENT};
use crate::store::Store;

use super::{SyncCallback, SyncEvent, SyncFlag, SyncSummary};

/// Sweep batch size — 200 UIDs per FETCH (matches Threat T-02-02).
const BATCH_SIZE: u32 = 200;

/// The sync engine. Holds an [`Arc<Mutex<Store>>`] for DB access;
/// the IMAP session is injected per-call so the caller controls
/// connect/logout and test fixtures are trivial.
pub struct SyncWorker {
    store: Arc<std::sync::Mutex<Store>>,
    /// Cooperative cancellation: `cancel_sync` sets it; the sweep checks
    /// it between batches and aborts cleanly (writes no partial state).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// App-data root for best-effort attachment-dir cleanup on expunge
    /// (`<root>/attachments/<uid_validity>/<uid>/`). `None` (tests, unset
    /// callers) skips fs cleanup — DB rows still clean explicitly.
    attachment_root: Option<PathBuf>,
    /// Send-flush environment (Plan 13-02: SMTP transport + gate +
    /// account). `None` (default, all pre-13-02 callers) disables the 4d
    /// pass entirely — Plan 13-03 plumbs it from the command layer.
    send_flush: Option<SendFlushEnv>,
}

/// Aggregate result of one outbox replay pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplaySummary {
    /// Ops acknowledged by the server (deleted from the queue).
    pub acked: usize,
    /// Ops dropped without a write (epoch mismatch or UID absent).
    pub dropped: usize,
    /// Ops that failed and stay queued for the next attempt.
    pub failed: usize,
    /// Delete/move ops acknowledged (every acked `imap_outbox` op moved
    /// server-side) — feeds `SyncSummary.moved`.
    pub moved: usize,
}

/// Cap on consecutive transient SMTP failures before a row parks as
/// terminal `failed` (manual retry only, never auto-dropped). Permanent
/// verdicts park immediately; uncertain verdicts reconcile instead of
/// counting here (the reconcile miss counts the attempt).
pub const MAX_SEND_ATTEMPTS: u32 = 10;

/// Delivery verdict of one flushed row.
///
/// `SentUnfiled` (SMTP accepted, Sent APPEND failed — APPEND-only retry,
/// never re-SMTP-send) is DEFINED here and PRODUCED by Plan 13-03's APPEND
/// leg with the `sent_unfiled` CHECK-extension migration (M11): no writer
/// exists yet, so 13-02 code never returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    Sent,
    SentUnfiled,
    /// Transient verdict: still queued with attempts+1 and a backoff schedule.
    Deferred,
    Failed,
    Uncertain,
}

/// Aggregate result of one [`SyncWorker::flush_send_queue`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlushSummary {
    pub sent: usize,
    pub deferred: usize,
    pub failed: usize,
    pub uncertain: usize,
    /// Always 0 in 13-02 (see [`FlushOutcome::SentUnfiled`]); mirrors the
    /// frozen `send_status()` shape for the 13-03 handoff.
    pub sent_unfiled: usize,
    /// True when the [`SendGate`] was busy — the caller skips, never overlaps.
    pub skipped: bool,
}

/// Aggregate result of one [`SyncWorker::reconcile_uncertain_sends`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReconcileSummary {
    /// Uncertain rows found in Sent: marked sent, no second transport call.
    pub confirmed_sent: usize,
    /// Uncertain rows missing in Sent: requeued for exactly one re-send
    /// with attempts counted and backoff scheduled.
    pub requeued: usize,
    /// Rows left `uncertain` (IMAP or store error — next pass retries).
    pub still_uncertain: usize,
    /// True when no Sent folder resolved — 13-03 owns the
    /// create-behind-confirmation flow, so rows wait untouched.
    pub skipped_no_sent: bool,
}

/// Everything the send flush needs: the single-flight gate, the (pooled)
/// blocking transport, the keyring-sourced account, and (Plan 13-03) the
/// manager lease + Sent folder wire for the filing leg. When `manager`
/// is `None` the leg is skipped — designed handoff pattern so pre-13-02
/// callers remain unaffected.
pub struct SendFlushEnv {
    pub gate: SendGate,
    pub transport: std::sync::Arc<dyn SmtpTransport>,
    pub account: SmtpAccount,
    /// Manager lease for the Sent filing leg (APPEND verbatim .eml bytes
    /// with `\Seen`). When `None` the leg is skipped (pre-13-03 behaviour).
    pub manager: Option<Arc<crate::imap::manager::SessionManager>>,
    /// Sent folder wire name resolved from the SessionManager lease.
    /// When `None` no Sent folder exists — the reconcile pass waits for
    /// Plan 13-03's create-behind-confirmation flow.
    pub sent_wire: Option<String>,
}

/// Envelope recipients for one row: To + Cc + BCC combined (BCC rides the
/// envelope only — it never reaches the rendered headers). A corrupt
/// recipient list is local damage: terminal, never retried.
fn envelope_recipients(row: &queries::SendRow) -> Result<Vec<String>, String> {
    const BAD: &str = "the queued recipients are unreadable — the message \
                       will not be retried automatically";
    let mut out: Vec<String> = Vec::new();
    for col in [&row.to_addrs, &row.cc_addrs, &row.bcc_addrs] {
        let addrs: Vec<String> = serde_json::from_str(col).map_err(|_| BAD.to_string())?;
        out.extend(addrs);
    }
    Ok(out)
}

/// Resolve the Sent folder's wire name from a LIST result (Phase 11 roles).
/// `None` means no Sent-like folder — the reconcile pass waits (13-03 owns
/// the create-behind-confirmation flow).
pub fn resolve_sent_wire(mailboxes: &[MailboxInfo]) -> Option<String> {
    resolve_roles(mailboxes)
        .into_iter()
        .find(|(_, role)| *role == Role::Sent)
        .map(|(wire, _)| wire)
}

impl SyncWorker {
    pub fn new(store: Arc<std::sync::Mutex<Store>>) -> Self {
        Self {
            store,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            attachment_root: None,
            send_flush: None,
        }
    }

    /// Share an externally owned cancellation flag (the `AppState` slot
    /// the `cancel_sync` command sets).
    pub fn with_cancel(
        store: Arc<std::sync::Mutex<Store>>,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            store,
            cancel,
            attachment_root: None,
            send_flush: None,
        }
    }

    /// Set the app-data root for attachment-dir cleanup on expunge.
    pub fn with_attachment_root(mut self, root: PathBuf) -> Self {
        self.attachment_root = Some(root);
        self
    }

    /// Attach the send-flush environment (SMTP transport + gate + account +
    /// manager lease + sent_wire for Sent filing leg). Without a manager the
    /// leg is skipped — designed handoff pattern so pre-13-02 callers remain unaffected.
    pub fn with_send_flush(mut self, env: SendFlushEnv) -> Self {
        self.send_flush = Some(env);
        self
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Replay queued Seen toggles for `mailbox_id` through `session`.
    ///
    /// RFC 4549 playback rules (T-6-02):
    /// - Every op stores its epoch (`uid_validity`) at enqueue; when the
    ///   current epoch differs the whole mailbox queue drops — replaying
    ///   stale UIDs against a renumbered mailbox would flag the wrong
    ///   messages.
    /// - Single ops whose UID is absent from `live_uids` drop (the message
    ///   is gone server-side). `None` skips the absent check — used by the
    ///   command path, which has no fresh SEARCH.
    ///
    /// Per-op failures are recorded (`attempts` + `last_error`) and stay
    /// queued for the next sync; only store-level errors abort the pass.
    /// The store lock is held only for brief synchronous sections, never
    /// across the `set_seen` await.
    pub async fn replay_outbox(
        &self,
        session: &mut dyn SyncSession,
        mailbox_id: u64,
        current_uid_validity: u32,
        live_uids: Option<&HashSet<u32>>,
    ) -> Result<ReplaySummary, SyncError> {
        let ops = {
            let guard = self.store.lock().unwrap();
            queries::list_outbox(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("list outbox: {e}")))?
        };
        let mut summary = ReplaySummary::default();
        if ops.is_empty() {
            return Ok(summary);
        }

        // Epoch check first: a UIDVALIDITY generation change invalidates
        // every queued UID at once.
        if ops.iter().any(|op| op.uid_validity != current_uid_validity) {
            let guard = self.store.lock().unwrap();
            let n = queries::drop_outbox_for_mailbox(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("drop stale outbox: {e}")))?;
            eprintln!(
                "[SGE sync] outbox epoch mismatch (current uid_validity={current_uid_validity}) — dropped {n} stale op(s)"
            );
            summary.dropped = n;
            return Ok(summary);
        }

        for op in &ops {
            if let Some(live) = live_uids {
                if !live.contains(&op.uid) {
                    let guard = self.store.lock().unwrap();
                    queries::delete_outbox_op(guard.conn(), mailbox_id, op.uid).map_err(|e| {
                        SyncError::Protocol(format!("delete absent outbox op: {e}"))
                    })?;
                    eprintln!(
                        "[SGE sync] outbox uid {} absent on server — dropped",
                        op.uid
                    );
                    summary.dropped += 1;
                    continue;
                }
            }
            match session.set_seen(op.uid, op.seen).await {
                Ok(()) => {
                    let guard = self.store.lock().unwrap();
                    queries::delete_outbox_op(guard.conn(), mailbox_id, op.uid).map_err(|e| {
                        SyncError::Protocol(format!("ack outbox op: {e}"))
                    })?;
                    summary.acked += 1;
                }
                Err(e) => {
                    let guard = self.store.lock().unwrap();
                    queries::record_outbox_error(
                        guard.conn(),
                        mailbox_id,
                        op.uid,
                        &e.to_string(),
                    )
                    .map_err(|store_err| {
                        SyncError::Protocol(format!("record outbox error: {store_err}"))
                    })?;
                    eprintln!(
                        "[SGE sync] outbox uid {} replay failed ({e}) — stays queued",
                        op.uid
                    );
                    summary.failed += 1;
                }
            }
        }
        Ok(summary)
    }

    /// Replay queued delete/move ops for `mailbox_id` through `session`.
    ///
    /// PRE-SWEEP position (non-negotiable): the pass calls this BEFORE the
    /// header sweep so locally-moved mail is gone server-side before the
    /// sweep can re-insert it (same-pass resurrection). `flag_outbox`
    /// deliberately stays post-sync: moves consume same-key flag ops at
    /// enqueue, so the two queues are disjoint by construction.
    ///
    /// Rules mirror [`replay_outbox`](Self::replay_outbox) (RFC 4549):
    /// epoch check first (whole-mailbox drop, hidden flags cleared so the
    /// sweep reconciles those rows as ordinary mail), absent-UID single
    /// drop, per-op failure stays queued with attempts. Both `delete`
    /// (stored Trash dest) and `move` replay as a server-side move —
    /// delete IS move-to-Trash, so no separate STORE-\Deleted path exists.
    ///
    /// A loud [`SyncError::Refused`](crate::imap::SyncError::Refused)
    /// (unverifiable unmark dance after the COPY leg already ran — the
    /// Wave-B residual) drops the op with a log instead of retrying:
    /// retrying would re-COPY duplicates only to refuse again. The src is
    /// untouched in that corner; dest-side partial copies converge on the
    /// next sweep.
    ///
    /// `mailbox_name` is the source wire name — re-SELECTed after any
    /// dest-side Seen resolution so the sweep below still lands on the
    /// right folder. The store lock is held only for brief synchronous
    /// sections, never across awaits.
    pub async fn replay_imap_outbox(
        &self,
        session: &mut dyn SyncSession,
        mailbox_id: u64,
        mailbox_name: &str,
        current_uid_validity: u32,
        live_uids: Option<&HashSet<u32>>,
    ) -> Result<ReplaySummary, SyncError> {
        let ops = {
            let guard = self.store.lock().unwrap();
            queries::list_imap_outbox(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("list imap outbox: {e}")))?
        };
        let mut summary = ReplaySummary::default();
        if ops.is_empty() {
            return Ok(summary);
        }

        // Epoch check first: a UIDVALIDITY generation change invalidates
        // every queued UID at once.
        if ops.iter().any(|op| op.uid_validity != current_uid_validity) {
            let guard = self.store.lock().unwrap();
            for op in &ops {
                let _ = queries::set_pending_delete(guard.conn(), mailbox_id, op.uid, false);
            }
            let n = queries::drop_imap_outbox_for_mailbox(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("drop stale imap outbox: {e}")))?;
            eprintln!(
                "[SGE sync] imap outbox epoch mismatch (current uid_validity={current_uid_validity}) — dropped {n} stale op(s)"
            );
            summary.dropped = n;
            return Ok(summary);
        }

        for op in &ops {
            if let Some(live) = live_uids {
                if !live.contains(&op.uid) {
                    let guard = self.store.lock().unwrap();
                    queries::delete_imap_outbox_op(guard.conn(), mailbox_id, op.uid).map_err(
                        |e| SyncError::Protocol(format!("delete absent imap op: {e}")),
                    )?;
                    let _ = queries::set_pending_delete(guard.conn(), mailbox_id, op.uid, false);
                    eprintln!(
                        "[SGE sync] imap outbox uid {} absent on server — dropped",
                        op.uid
                    );
                    summary.dropped += 1;
                    continue;
                }
            }
            let Some(dest) = op.dest_mailbox.as_deref() else {
                // Defensive only: enqueue always stores a dest.
                let guard = self.store.lock().unwrap();
                queries::delete_imap_outbox_op(guard.conn(), mailbox_id, op.uid).map_err(
                    |e| SyncError::Protocol(format!("delete destless imap op: {e}")),
                )?;
                eprintln!(
                    "[SGE sync] imap outbox uid {} has no dest — dropped",
                    op.uid
                );
                summary.dropped += 1;
                continue;
            };
            let caps = session.capabilities().await.map_err(|e| {
                SyncError::Protocol(format!("imap replay CAPABILITY: {e}"))
            })?;
            match run_move_on_session(session, &[op.uid], dest, &caps).await {
                Ok(_) => {
                    // Move-carries-toggle: apply captured Seen state at the
                    // destination UID (best-effort, never fails the replay).
                    if let Some(seen) = op.seen_intent {
                        self.apply_seen_intent(
                            session,
                            mailbox_id,
                            mailbox_name,
                            op.uid,
                            dest,
                            seen,
                        )
                        .await;
                    }
                    let guard = self.store.lock().unwrap();
                    queries::delete_imap_outbox_op(guard.conn(), mailbox_id, op.uid).map_err(
                        |e| SyncError::Protocol(format!("ack imap op: {e}")),
                    )?;
                    // Hidden flag stays set: the sweep's expunge-diff removes
                    // the row this same pass (the UID is gone server-side).
                    summary.acked += 1;
                    summary.moved += 1;
                }
                Err(SyncError::Refused(detail)) => {
                    let guard = self.store.lock().unwrap();
                    queries::delete_imap_outbox_op(guard.conn(), mailbox_id, op.uid).map_err(
                        |e| SyncError::Protocol(format!("drop refused imap op: {e}")),
                    )?;
                    let _ = queries::set_pending_delete(guard.conn(), mailbox_id, op.uid, false);
                    eprintln!(
                        "[SGE sync] imap outbox uid {} refused ({detail}) — dropped, src untouched",
                        op.uid
                    );
                    summary.dropped += 1;
                }
                Err(e) => {
                    let guard = self.store.lock().unwrap();
                    queries::record_imap_outbox_error(
                        guard.conn(),
                        mailbox_id,
                        op.uid,
                        &e.to_string(),
                    )
                    .map_err(|store_err| {
                        SyncError::Protocol(format!("record imap error: {store_err}"))
                    })?;
                    eprintln!(
                        "[SGE sync] imap outbox uid {} replay failed ({e}) — stays queued",
                        op.uid
                    );
                    summary.failed += 1;
                }
            }
        }
        Ok(summary)
    }

    /// Replay dirty drafts for one mailbox (Plan 12-02 reconnect flush).
    ///
    /// Iterates `list_dirty_drafts()` filtered to `drafts_mailbox_id`,
    /// renders each row via `render_draft_rfc5322` (Date = now), and runs
    /// the save-sequence against the already-held `session` — the same
    /// legs as [`save_draft_on_session`] (APPEND-new + SEARCH-reconcile +
    /// scoped expunge-old), minus the lease, since the pass already holds
    /// the session through its SELECT of this folder. `mark_draft_clean`
    /// per ack; per-row failure keeps `dirty=1` and never fails the pass
    /// (worker replay discipline). Returns the acked-row count.
    ///
    /// `epoch` is the pass's current UIDVALIDITY: when the stored sync
    /// state moved on, the tracked `server_uid` belongs to a dead
    /// generation and flushes as `old_uid=None` (the orphan is reaped by
    /// the sweep's expunge-diff, Phase 9 semantics).
    ///
    /// SELECT discipline (T-12-04): callers must run this while the
    /// session is SELECTed to `drafts_wire` — the expunge-old leg
    /// addresses the selected folder. Rows are filtered to
    /// `drafts_mailbox_id`, so other folders' drafts wait for their own
    /// pass. `From` renders empty: the row carries no From column (the
    /// `save_draft` command fills it from the account at save time) and
    /// only the Message-ID matters for reconcile.
    pub async fn replay_dirty_drafts(
        &self,
        session: &mut dyn SyncSession,
        drafts_mailbox_id: u64,
        drafts_wire: &str,
        epoch: u32,
    ) -> Result<usize, SyncError> {
        let rows = {
            let guard = self.store.lock().unwrap();
            queries::list_dirty_drafts(guard.conn())
                .map_err(|e| SyncError::Protocol(format!("list dirty drafts: {e}")))?
                .into_iter()
                .filter(|r| r.mailbox_id == drafts_mailbox_id)
                .collect::<Vec<_>>()
        };
        if rows.is_empty() {
            return Ok(0);
        }
        let stale_epoch = {
            let guard = self.store.lock().unwrap();
            match queries::get_sync_state(guard.conn(), drafts_wire)
                .map_err(|e| SyncError::Protocol(format!("drafts sync state: {e}")))?
            {
                Some((v, _)) => v != epoch,
                None => false,
            }
        };
        if stale_epoch {
            eprintln!(
                "[SGE sync] drafts epoch mismatch (current uid_validity={epoch}) — tracked server_uids are stale, appending fresh"
            );
        }
        let caps = session.capabilities().await.map_err(|e| {
            SyncError::Protocol(format!("draft replay CAPABILITY: {e}"))
        })?;
        let date = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S +0000")
            .to_string();
        let mut acked = 0;
        for row in &rows {
            let split = |s: &str| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            };
            let fields = crate::drafts::DraftFields {
                from: String::new(),
                to: split(&row.to),
                cc: split(&row.cc),
                bcc: split(&row.bcc),
                subject: row.subject.clone(),
                body: row.body.clone(),
                message_id: row.message_id.clone(),
            };
            let bytes = crate::drafts::render_draft_rfc5322(&fields, &date);
            let old_uid = if stale_epoch { None } else { row.server_uid };
            match save_draft_on_session(
                session,
                drafts_wire,
                &row.message_id,
                &bytes,
                old_uid,
                &caps,
            )
            .await
            {
                Ok(new_uid) => {
                    let guard = self.store.lock().unwrap();
                    if let Err(e) = queries::mark_draft_clean(guard.conn(), &row.id, new_uid) {
                        eprintln!(
                            "[SGE sync] draft {} acked as uid {new_uid} but clean-mark failed ({e}) — stays dirty",
                            row.id
                        );
                    } else {
                        eprintln!(
                            "[SGE sync] draft {} acknowledged as uid {new_uid}",
                            row.id
                        );
                        acked += 1;
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[SGE sync] draft {} replay failed ({e}) — stays dirty",
                        row.id
                    );
                }
            }
        }
        Ok(acked)
    }

    // ── send-queue flush + uncertain reconcile (Plan 13-02) ──────────
    //
    // The SMTP leg ([`SyncWorker::flush_send_queue`]) takes NO IMAP session
    // for the send itself; the Plan 13-03 Sent filing leg (SEARCH + APPEND
    // verbatim `.eml` with `\Seen`) takes a manager lease only when the env
    // carries one (`manager: None` skips it). Uncertain
    // verdicts reconcile through [`SyncWorker::reconcile_uncertain_sends`]
    // (Sent SEARCH before any re-send). Both run as step 4d of the
    // reconnect pass, after the dirty-draft replay.

    /// Flush due send-queue rows over SMTP.
    ///
    /// Blocking discipline: the transport is sync and MUST run on a blocking
    /// thread — the production caller (`start_sync`) already runs the whole
    /// pass inside `spawn_blocking` + `block_on`, and the async Sent filing
    /// leg (IMAP SEARCH + APPEND) awaits on that same blocking thread.
    /// Tests drive this through `flush_blocking` (a `block_on` shim) or
    /// inside `block_on` like the other async pass steps.
    ///
    /// Order per pass: crash recovery (stranded `sending` rows become
    /// `uncertain`, never blindly re-queued) → due list → per-row claim
    /// (`queued` → `sending`) → verdict transitions. Per-row failure
    /// (store, fs, envelope) never fails the pass.
    pub async fn flush_send_queue(&self, env: &SendFlushEnv) -> FlushSummary {
        let Some(_guard) = env.gate.try_begin() else {
            eprintln!("[SGE send] flush skipped: another send pass is in flight");
            return FlushSummary {
                skipped: true,
                ..Default::default()
            };
        };
        // Crash recovery FIRST (launch discipline): rows stranded in
        // `sending` may have been SMTP-accepted before the crash, so they
        // become `uncertain` (reconcile-not-resend) instead of re-queueing
        // blindly. `reset_sending_to_queued` keeps its 13-01 semantics; the
        // triage to `uncertain` is this pass's safety layer on top.
        {
            let guard = self.store.lock().unwrap();
            match queries::list_sending_send_ids(guard.conn()) {
                Ok(ids) if !ids.is_empty() => {
                    let n = ids.len();
                    if crash_recover(guard.conn()).is_ok() {
                        for id in &ids {
                            if let Err(e) = queries::mark_send_uncertain(
                                guard.conn(),
                                id,
                                "the app may have sent this before a restart — \
                                 it will be checked in Sent before any re-send",
                            ) {
                                eprintln!("[SGE send] {id} recovery triage failed ({e})");
                            }
                        }
                        eprintln!(
                            "[SGE send] crash recovery: {n} stranded row(s) → uncertain"
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("[SGE send] crash triage read failed ({e}) — continuing"),
            }
        }
        let now = utc_now_sql();
        let due: Vec<queries::SendRow> = {
            let guard = self.store.lock().unwrap();
            match queries::list_due_sends(guard.conn(), &now) {
                Ok(rows) => rows,
                Err(e) => {
                    eprintln!("[SGE send] due-list read failed ({e}) — pass ends");
                    return FlushSummary::default();
                }
            }
        };
        let mut summary = FlushSummary::default();
        for row in &due {
            match self.flush_one_row(env, row).await {
                Some(FlushOutcome::Sent) => summary.sent += 1,
                Some(FlushOutcome::Deferred) => summary.deferred += 1,
                Some(FlushOutcome::Failed) => summary.failed += 1,
                Some(FlushOutcome::Uncertain) => summary.uncertain += 1,
                Some(FlushOutcome::SentUnfiled) => summary.sent_unfiled += 1,
                None => {}
            }
        }
        eprintln!(
            "[SGE send] flush done: sent={} deferred={} failed={} uncertain={}",
            summary.sent, summary.deferred, summary.failed, summary.uncertain
        );
        summary
    }

    /// Send one due row to its verdict. Returns `None` when a store write
    /// failed before any verdict landed (row skipped, uncounted — never
    /// fails the pass). Reads the immutable `.eml` bytes off the store lock;
    /// the transport call itself runs on the caller's blocking thread, and
    /// the async Sent filing leg (SEARCH + APPEND) awaits in place — the
    /// whole pass already runs inside `spawn_blocking` + `block_on`.
    async fn flush_one_row(&self, env: &SendFlushEnv, row: &queries::SendRow) -> Option<FlushOutcome> {
        {
            let guard = self.store.lock().unwrap();
            if let Err(e) = queries::set_send_state(
                guard.conn(),
                &row.id,
                SEND_STATE_SENDING,
            ) {
                eprintln!("[SGE send] {} claim failed ({e}) — skipping", row.id);
                return None;
            }
        }
        let recipients = match envelope_recipients(row) {
            Ok(r) => r,
            Err(message) => return Some(self.park_failed(&row.id, &message)),
        };
        let bytes = match std::fs::read(&row.eml_path) {
            Ok(b) => b,
            Err(_) => {
                return Some(self.park_failed(
                    &row.id,
                    "the queued message file is missing — the message was \
                     not sent and will not be retried automatically",
                ));
            }
        };
        match env
            .transport
            .send_raw(&env.account, &row.from_addr, &recipients, &bytes)
        {
            SmtpOutcome::Sent => {
                let guard = self.store.lock().unwrap();
                if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT)
                {
                    eprintln!("[SGE send] {} sent but state write failed ({e})", row.id);
                    return None;
                }
                eprintln!("[SGE send] {} sent (message {})", row.id, row.message_id);
                // ── Plan 13-03 Sent filing leg ──
                // Probe-before-APPEND dedupe: SEARCH Sent for this Message-ID.
                // Hit means server auto-saved → skip APPEND; miss means APPEND
                // verbatim .eml bytes with \Seen. APPEND failure yields
                // sent_unfiled with APPEND-only retry, never re-SMTP-send.
                if let Some(ref manager) = env.manager {
                    let mid = &row.message_id;
                    let sent_folder =
                        env.sent_wire.clone().unwrap_or_else(|| "Sent".to_string());
                    // Select Sent folder
                    let sent_selected = manager
                        .lease_for(&sent_folder)
                        .await;
                    match sent_selected {
                        Ok(mut lease) => {
                            let search_result = lease
                                .session()
                                .uid_search_header("Message-ID", mid)
                                .await;
                            match search_result {
                                Ok(hits) if !hits.is_empty() => {
                                    // Probe hit: server already auto-saved this message.
                                    // Dedupe: skip APPEND, just mark sent.
                                    eprintln!(
                                        "[SGE send] {} deduped in Sent (Message-ID hit), skipping APPEND",
                                        row.id
                                    );
                                    // Still mark the state as sent
                                    if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                        eprintln!("[SGE send] {} state write failed after dedupe ({e})", row.id);
                                        return None;
                                    }
                                    Some(FlushOutcome::Sent)
                                }
                                Ok(_) => {
                                    // Probe miss: no existing copy in Sent → APPEND verbatim.
                                    let bytes = std::fs::read(&row.eml_path).unwrap_or_default();
                                    let append_result = lease
                                        .session()
                                        .append_message(
                                            &sent_folder,
                                            "\\Seen",
                                            &bytes,
                                        )
                                        .await;
                                    match append_result {
                                        Ok(()) => {
                                            eprintln!(
                                                "[SGE send] {} APPENDed to Sent with \\Seen",
                                                row.id
                                            );
                                            // Mark the queue row as sent after successful APPEND
                                            if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                                eprintln!("[SGE send] {} state write failed after APPEND ({e})", row.id);
                                                return None;
                                            }
                                            Some(FlushOutcome::Sent)
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[SGE send] {} APPEND failed ({e}) — sent_unfiled, APPEND-only retry later",
                                                row.id
                                            );
                                            // Park as sent_unfiled: SMTP succeeded but APPEND failed
                                            // Never triggers re-SMTP-send; APPEND-only retry on later passes
                                            if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                                eprintln!("[SGE send] {} state write marked sent despite APPEND failure ({e})", row.id);
                                            }
                                            Some(FlushOutcome::SentUnfiled)
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!(
                                        "[SGE send] {} Sent SEARCH failed ({e}) — treating as miss, attempting APPEND",
                                        row.id
                                    );
                                    // Treat SEARCH error as miss: attempt APPEND verbatim
                                    let bytes = std::fs::read(&row.eml_path).unwrap_or_default();
                                    let append_result = lease
                                        .session()
                                        .append_message(
                                            &sent_folder,
                                            "\\Seen",
                                            &bytes,
                                        )
                                        .await;
                                    match append_result {
                                        Ok(()) => {
                                            eprintln!(
                                                "[SGE send] {} APPENDed to Sent after SEARCH error",
                                                row.id
                                            );
                                            if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                                eprintln!("[SGE send] {} state write failed after APPEND ({e})", row.id);
                                                return None;
                                            }
                                            Some(FlushOutcome::Sent)
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[SGE send] {} APPEND failed after SEARCH error ({e}) — sent_unfiled",
                                                row.id
                                            );
                                            if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                                eprintln!("[SGE send] {} state write marked sent despite APPEND failure ({e})", row.id);
                                            }
                                            Some(FlushOutcome::SentUnfiled)
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "[SGE send] {} Sent lease failed ({e}) — treating as miss, attempting APPEND",
                                row.id
                            );
                            // Cannot lease Sent folder; treat as miss and attempt APPEND
                            // using a best-effort approach with the sent_wire if available
                            if let Some(sent_wire) = &env.sent_wire {
                                let lease_result = manager
                                    .lease_for(sent_wire)
                                    .await;
                                if let Ok(mut lease) = lease_result {
                                    let bytes = std::fs::read(&row.eml_path).unwrap_or_default();
                                    let append_result = lease
                                        .session()
                                        .append_message(sent_wire, "\\Seen", &bytes)
                                        .await;
                                    match append_result {
                                        Ok(()) => {
                                            eprintln!(
                                                "[SGE send] {} APPENDed to {} after lease recovery",
                                                row.id, sent_wire
                                            );
                                            if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                                eprintln!("[SGE send] {} state write failed after APPEND ({e})", row.id);
                                                return None;
                                            }
                                            Some(FlushOutcome::Sent)
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[SGE send] {} APPEND failed after lease recovery ({e}) — sent_unfiled",
                                                row.id
                                            );
                                            if let Err(e) = queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                                                eprintln!("[SGE send] {} state write marked sent despite APPEND failure ({e})", row.id);
                                            }
                                            Some(FlushOutcome::SentUnfiled)
                                        }
                                    }
                                } else {
                                    // Could not lease Sent folder at all — cannot file, mark as sent_unfiled
                                    Some(FlushOutcome::SentUnfiled)
                                }
                            } else {
                                // No sent_wire available — cannot file, mark as sent_unfiled
                                Some(FlushOutcome::SentUnfiled)
                            }
                        }
                    }
                } else {
                    // No manager provided — skip filing leg (pre-13-03 behaviour),
                    // just mark the row as sent
                    Some(FlushOutcome::Sent)
                }
                // ── End Sent filing leg ──
            }
            SmtpOutcome::Transient { message } => {
                let guard = self.store.lock().unwrap();
                let post = row.attempts + 1;
                let res = if post >= MAX_SEND_ATTEMPTS as i64 {
                    eprintln!(
                        "[SGE send] {} transient x{post} — parking as failed",
                        row.id
                    );
                    crate::send_queue::mark_send_failed(
                        guard.conn(),
                        &row.id,
                        &format!(
                            "gave up after {post} tries ({message}) — check \
                             the connection, then tap retry"
                        ),
                    )
                } else {
                    crate::send_queue::record_send_failure(guard.conn(), &row.id, &message)
                        .map(|_| ())
                };
                match res {
                    Ok(()) => Some(if post >= MAX_SEND_ATTEMPTS as i64 {
                        FlushOutcome::Failed
                    } else {
                        FlushOutcome::Deferred
                    }),
                    Err(e) => {
                        eprintln!("[SGE send] {} attempt write failed ({e})", row.id);
                        None
                    }
                }
            }
            SmtpOutcome::Permanent { message } => Some(self.park_failed(&row.id, &message)),
            SmtpOutcome::Uncertain { reason } => {
                let guard = self.store.lock().unwrap();
                if let Err(e) = queries::mark_send_uncertain(guard.conn(), &row.id, &reason) {
                    eprintln!("[SGE send] {} uncertain write failed ({e})", row.id);
                    return None;
                }
                eprintln!(
                    "[SGE send] {} uncertain — reconciling in Sent before any re-send",
                    row.id
                );
                Some(FlushOutcome::Uncertain)
            }
        }
    }

    /// Park a row terminally `failed` (manual retry only, never auto-dropped
    /// or auto-retried). A store failure here leaves the row `sending` for
    /// next-pass crash recovery — still loud, still safe.
    fn park_failed(&self, id: &str, message: &str) -> FlushOutcome {
        let guard = self.store.lock().unwrap();
        match crate::send_queue::mark_send_failed(guard.conn(), id, message) {
            Ok(()) => {
                eprintln!("[SGE send] {id} parked as failed");
                FlushOutcome::Failed
            }
            Err(e) => {
                eprintln!("[SGE send] {id} failed-park write failed ({e}) — recovery triages it");
                FlushOutcome::Failed
            }
        }
    }

    /// Reconcile `uncertain` rows against Sent (reconcile-not-resend,
    /// T-13-07): SELECT `sent_wire`, SEARCH each row's Message-ID — a hit
    /// means the bytes landed (mark `sent` and hand off to Plan 13-03's
    /// Sent filing; no second transport call ever), a miss means exactly one
    /// requeue with attempts counted and backoff scheduled.
    ///
    /// Zero IMAP when nothing is uncertain, and zero IMAP when `sent_wire`
    /// is `None` (no Sent folder resolved — 13-03 owns the
    /// create-behind-confirmation flow, so rows wait untouched). Restores
    /// `resume_wire` selection before returning (same SELECT discipline as
    /// `apply_seen_intent`).
    pub async fn reconcile_uncertain_sends(
        &self,
        session: &mut dyn SyncSession,
        sent_wire: Option<&str>,
        resume_wire: &str,
    ) -> Result<ReconcileSummary, SyncError> {
        let rows: Vec<queries::SendRow> = {
            let guard = self.store.lock().unwrap();
            queries::list_uncertain_sends(guard.conn())
                .map_err(|e| SyncError::Protocol(format!("list uncertain sends: {e}")))?
        };
        if rows.is_empty() {
            return Ok(ReconcileSummary::default());
        }
        let Some(sent) = sent_wire else {
            eprintln!(
                "[SGE send] {} uncertain row(s) but no Sent folder — waiting",
                rows.len()
            );
            return Ok(ReconcileSummary {
                skipped_no_sent: true,
                ..Default::default()
            });
        };
        session
            .select_mailbox(sent)
            .await
            .map_err(|e| SyncError::Protocol(format!("reconcile SELECT {sent}: {e}")))?;
        let mut summary = ReconcileSummary::default();
        for row in &rows {
            match session.uid_search_header("Message-ID", &row.message_id).await {
                Ok(hits) if !hits.is_empty() => {
                    let guard = self.store.lock().unwrap();
                    match queries::set_send_state(guard.conn(), &row.id, SEND_STATE_SENT) {
                        Ok(()) => {
                            eprintln!(
                                "[SGE send] {} found in Sent — confirmed sent, no re-send",
                                row.id
                            );
                            summary.confirmed_sent += 1;
                        }
                        Err(e) => {
                            eprintln!("[SGE send] {} Sent-hit state write failed ({e})", row.id);
                            summary.still_uncertain += 1;
                        }
                    }
                }
                Ok(_) => {
                    // Miss: a single re-send — attempts counted, backoff
                    // scheduled, identical bytes on the next try.
                    let guard = self.store.lock().unwrap();
                    let miss_note = "not found in Sent on reconcile — requeued for one more send";
                    match crate::send_queue::record_send_failure(guard.conn(), &row.id, miss_note)
                        .and_then(|_| {
                            queries::set_send_state(guard.conn(), &row.id, SEND_STATE_QUEUED)
                        }) {
                        Ok(()) => {
                            eprintln!("[SGE send] {} absent from Sent — requeued once", row.id);
                            summary.requeued += 1;
                        }
                        Err(e) => {
                            eprintln!("[SGE send] {} requeue write failed ({e})", row.id);
                            summary.still_uncertain += 1;
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[SGE send] {} Sent SEARCH failed ({e}) — stays uncertain",
                        row.id
                    );
                    summary.still_uncertain += 1;
                }
            }
        }
        if let Err(e) = session.select_mailbox(resume_wire).await {
            eprintln!("[SGE send] WARN: re-SELECT {resume_wire} failed ({e}) — sweep may drift");
        }
        Ok(summary)
    }

    /// Step 4d of the reconnect pass: SMTP flush after the dirty-draft
    /// replay, then uncertain reconcile. No-op without a configured
    /// [`SendFlushEnv`]. Reconcile/lookup failures never fail the pass —
    /// uncertain rows simply wait for the next one.
    async fn flush_send_queue_in_pass(
        &self,
        session: &mut dyn SyncSession,
        mailbox_name: &str,
    ) {
        let Some(env) = self.send_flush.as_ref() else {
            return;
        };
        let fsum = self.flush_send_queue(env).await;
        if fsum.skipped {
            return;
        }
        let has_uncertain: bool = {
            let guard = self.store.lock().unwrap();
            queries::list_uncertain_sends(guard.conn())
                .map(|rows| !rows.is_empty())
                .unwrap_or(false)
        };
        if !has_uncertain {
            return;
        }
        let mailboxes = match session.list_mailboxes().await {
            Ok(mbs) => mbs,
            Err(e) => {
                eprintln!(
                    "[SGE send] Sent lookup LIST failed ({e}) — uncertain rows wait for the next pass"
                );
                return;
            }
        };
        let sent_wire = resolve_sent_wire(&mailboxes);
        match self
            .reconcile_uncertain_sends(session, sent_wire.as_deref(), mailbox_name)
            .await
        {
            Ok(r) => eprintln!(
                "[SGE send] reconcile: confirmed_sent={} requeued={} still_uncertain={} skipped_no_sent={}",
                r.confirmed_sent, r.requeued, r.still_uncertain, r.skipped_no_sent
            ),
            Err(e) => eprintln!(
                "[SGE send] reconcile failed ({e}) — uncertain rows wait for the next pass"
            ),
        }
    }

    /// Best-effort Seen apply at a move destination (move-carries-toggle).
    ///
    /// The server assigns a new UID on move (no COPYUID parsing — the
    /// imap-proto 0.16 tripwire class), so the dest UID resolves by
    /// Message-ID SEARCH over `dest_mailbox`. Unresolvable rows, missing
    /// Message-IDs, and every IO error log-and-drop: the op is already
    /// acked and the next sync converges flags. Always re-SELECTs the
    /// source mailbox before returning so the sweep still lands on the
    /// right folder. Never fails the replay.
    async fn apply_seen_intent(
        &self,
        session: &mut dyn SyncSession,
        mailbox_id: u64,
        src_mailbox: &str,
        src_uid: u32,
        dest_mailbox: &str,
        seen: bool,
    ) {
        let mid: Option<String> = {
            let guard = self.store.lock().unwrap();
            queries::message_rfc_id(guard.conn(), mailbox_id, src_uid).unwrap_or(None)
        };
        let Some(mid) = mid else {
            eprintln!("[SGE sync] seen-intent uid {src_uid}: no local row left — dropped");
            return;
        };
        // Select dest; any failure restores src selection and drops.
        if let Err(e) = session.select_mailbox(dest_mailbox).await {
            eprintln!("[SGE sync] seen-intent uid {src_uid}: SELECT {dest_mailbox} failed ({e}) — dropped");
            let _ = session.select_mailbox(src_mailbox).await;
            return;
        }
        let mut dest_uid: Option<u32> = None;
        match session.search_uids().await {
            Ok(uids) => {
                'search: for chunk in uids.chunks(200) {
                    let set = chunk
                        .iter()
                        .map(|u| u.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    match session.fetch_envelopes(&set).await {
                        Ok(headers) => {
                            for h in &headers {
                                if h.message_id.as_deref() == Some(mid.as_str()) {
                                    dest_uid = Some(h.uid);
                                    break 'search;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[SGE sync] seen-intent uid {src_uid}: dest FETCH failed ({e}) — dropped");
                            break;
                        }
                    }
                }
            }
            Err(e) => eprintln!("[SGE sync] seen-intent uid {src_uid}: dest SEARCH failed ({e}) — dropped"),
        }
        if let Some(duid) = dest_uid {
            match session.set_seen(duid, seen).await {
                Ok(()) => eprintln!(
                    "[SGE sync] seen-intent uid {src_uid} applied at {dest_mailbox}:{duid} (seen={seen})"
                ),
                Err(e) => eprintln!(
                    "[SGE sync] seen-intent uid {src_uid}: STORE at {dest_mailbox}:{duid} failed ({e}) — dropped"
                ),
            }
        } else {
            eprintln!("[SGE sync] seen-intent uid {src_uid}: message-id {mid} not found in {dest_mailbox} — dropped");
        }
        if let Err(e) = session.select_mailbox(src_mailbox).await {
            eprintln!("[SGE sync] WARN: re-SELECT {src_mailbox} failed ({e}) — sweep may drift");
        }
    }

    /// Delete UIDs absent from `live_union` plus best-effort attachment-dir
    /// cleanup for the removed rows. Single choke point for the three
    /// expunge sites (bump wipe, empty shortcut, Step 6 diff) so DB rows,
    /// FTS ghosts, bodies/parts, and on-disk dirs clean together.
    /// Returns the removed-row count for the summary.
    fn expunge_absent(
        &self,
        mailbox_id: u64,
        live_union: &[u32],
        uid_validity: u32,
    ) -> Result<usize, SyncError> {
        let live_set: HashSet<u32> = live_union.iter().copied().collect();
        let local: Vec<u32> = {
            let guard = self.store.lock().unwrap();
            queries::all_local_uids(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("local uids for cleanup: {e}")))?
        };
        let removed: Vec<u32> = local
            .into_iter()
            .filter(|u| !live_set.contains(u))
            .collect();
        let n = {
            let guard = self.store.lock().unwrap();
            queries::delete_missing_uids(guard.conn(), mailbox_id, live_union)
                .map_err(|e| SyncError::Protocol(format!("delete_missing_uids: {e}")))?
        };
        self.cleanup_attachment_dirs(uid_validity, &removed);
        Ok(n)
    }

    /// Best-effort removal of `<root>/attachments/<uidv>/<uid>/` dirs for
    /// expunged UIDs. Fs errors log and never fail the sync; `None` root
    /// (tests, unset callers) skips silently.
    fn cleanup_attachment_dirs(&self, uid_validity: u32, uids: &[u32]) {
        let Some(root) = self.attachment_root.as_ref() else {
            return;
        };
        for uid in uids {
            let dir = crate::store::attachment_dir(root, uid_validity, *uid);
            if dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    eprintln!(
                        "[SGE sync] attachment cleanup uid {uid} failed ({e}) — continuing"
                    );
                }
            }
        }
    }

    /// Execute a full sync pass against an injected [`SyncSession`].
    ///
    /// The session is consumed (boxed trait object) — the caller
    /// owns connection lifecycle.  The worker emits progress via
    /// `cb` and returns an aggregate [`SyncSummary`]. Delegates to
    /// [`sync_with_borrowed`](Self::sync_with_borrowed).
    pub async fn sync_with_session(
        &self,
        mut session: Box<dyn SyncSession>,
        mailbox_name: &str,
        cb: SyncCallback,
    ) -> Result<SyncSummary, SyncError> {
        self.sync_with_borrowed(&mut *session, mailbox_name, cb)
            .await
    }

    /// Execute a full sync pass against a borrowed [`SyncSession`].
    ///
    /// Plan 10-02 precondition entry: `start_sync` holds a manager
    /// [`MailboxLease`](crate::imap::manager::MailboxLease) across the pass
    /// and passes `lease.session()` here, so the sweep SELECTs through the
    /// single owned connection instead of opening a fresh session per pass
    /// that could race destructive leases. No second IMAP connection.
    pub async fn sync_with_borrowed(
        &self,
        session: &mut dyn SyncSession,
        mailbox_name: &str,
        cb: SyncCallback,
    ) -> Result<SyncSummary, SyncError> {
        // ── Step 1: ensure mailbox row + read sync state ─────────
        let mailbox_id = {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            queries::ensure_mailbox(conn, mailbox_name)
                .map_err(|e| SyncError::Protocol(format!("ensure mailbox: {e}")))?
        };
        let (prev_uidv, _prev_next) = {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            let prev = queries::get_sync_state(conn, mailbox_name)
                .map_err(|e| SyncError::Protocol(format!("get sync state: {e}")))?;
            prev.unwrap_or((0u32, 0u32))
        };

        // ── Step 2: SELECT mailbox ──────────────────────────────────
        let summary: MailboxSummary = session.select_mailbox(mailbox_name).await?;
        eprintln!(
            "[SGE sync] SELECT {}: exists={} uid_validity={} uid_next={:?}",
            mailbox_name, summary.exists, summary.uid_validity, summary.uid_next
        );

        // ── Step 2b: STATUS triage signals (FOLD-02) ────────────────
        // Read-only: caches the server UNSEEN datum per folder for the
        // sidebar badge fallback. Graceful — a STATUS failure must not
        // fail the sync (some servers restrict it); the badge falls back
        // to the dynamic local count.
        match session.mailbox_status(mailbox_name).await {
            Ok(status) => {
                let guard = self.store.lock().unwrap();
                if let Err(e) = queries::set_mailbox_status(
                    guard.conn(),
                    mailbox_name,
                    status.uid_validity,
                    status.uid_next.unwrap_or(summary.uid_next.unwrap_or(1)),
                    status.unseen,
                ) {
                    eprintln!("[SGE sync] STATUS cache write failed ({e}) — continuing");
                } else {
                    eprintln!(
                        "[SGE sync] STATUS {mailbox_name}: uid_validity={} unseen={}",
                        status.uid_validity, status.unseen
                    );
                }
            }
            Err(e) => eprintln!("[SGE sync] STATUS {mailbox_name} failed ({e}) — continuing without unseen cache"),
        }

        // ── Step 3: UIDVALIDITY guard ─────────────────────────────
        let uid_validity_bump = prev_uidv != 0 && prev_uidv != summary.uid_validity;
        if uid_validity_bump {
            self.expunge_absent(mailbox_id, &[], summary.uid_validity)?;
            // Queued UIDs belong to the old generation — replaying them
            // would flag the wrong messages (RFC 4549, T-6-02). The wipe
            // path already dropped the imap queue; the flag queue drops here.
            let guard = self.store.lock().unwrap();
            queries::drop_outbox_for_mailbox(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("drop outbox on UIDVALIDITY bump: {e}")))?;
        }

        let mut result = SyncSummary {
            uid_validity_bump,
            ..Default::default()
        };

        // ── Step 4: search for all live UIDs on the server ────────
        let server_uids = session.search_uids().await?;
        eprintln!(
            "[SGE sync] UID SEARCH ALL: {} uids (min={:?} max={:?})",
            server_uids.len(),
            server_uids.first(),
            server_uids.last()
        );

        // Inconsistency guard: the server claims messages exist but SEARCH
        // came back empty. Wiping the local cache here would destroy data
        // on a protocol hiccup — fail loudly instead.
        if server_uids.is_empty() && summary.exists > 0 {
            return Err(SyncError::Protocol(format!(
                "server reports EXISTS={} but UID SEARCH ALL returned no UIDs — \
                 refusing to wipe local cache (check server logs / namespace)",
                summary.exists
            )));
        }

        // Empty-mailbox shortcut — no UID range to sweep.
        if server_uids.is_empty() {
            {
                let guard = self.store.lock().unwrap();
                queries::set_sync_state(
                    guard.conn(),
                    mailbox_name,
                    summary.uid_validity,
                    summary.uid_next.unwrap_or(1),
                )
                .map_err(|e| SyncError::Protocol(format!("set sync state: {e}")))?;
            }
            result.deleted = self.expunge_absent(mailbox_id, &[], summary.uid_validity)?;
            // Every queued UID is absent — both replays drop their queues.
            let empty: HashSet<u32> = HashSet::new();
            let replay = self
                .replay_outbox(&mut *session, mailbox_id, summary.uid_validity, Some(&empty))
                .await?;
            eprintln!(
                "[SGE sync] replay on empty mailbox: acked={} dropped={} failed={}",
                replay.acked, replay.dropped, replay.failed
            );
            let imap_replay = self
                .replay_imap_outbox(
                    &mut *session,
                    mailbox_id,
                    mailbox_name,
                    summary.uid_validity,
                    Some(&empty),
                )
                .await?;
            eprintln!(
                "[SGE sync] imap replay on empty mailbox: acked={} dropped={} failed={} moved={}",
                imap_replay.acked, imap_replay.dropped, imap_replay.failed, imap_replay.moved
            );
            result.moved += imap_replay.moved;
            // Drafts folder may be message-empty yet hold dirty rows —
            // flush while SELECTed here (same SELECT discipline as below).
            let draft_acked = self
                .replay_dirty_drafts(
                    &mut *session,
                    mailbox_id,
                    mailbox_name,
                    summary.uid_validity,
                )
                .await?;
            if draft_acked > 0 {
                eprintln!("[SGE sync] draft replay on empty mailbox: acked={draft_acked}");
            }
            // Same 4d flush as the normal branch (order: outbox replays,
            // then drafts, then send flush) so stranded rows drain even when
            // the mailbox is message-empty.
            self.flush_send_queue_in_pass(&mut *session, mailbox_name)
                .await;
            session.logout().await?;
            cb(SyncEvent::SyncCompleted {
                summary: result.clone(),
            });
            return Ok(result);
        }

        // ── Step 4b: pre-sweep imap_outbox replay (Plan 10-03) ──
        // NON-NEGOTIABLE ORDER: queued delete/move ops replay BEFORE the
        // header sweep so locally-moved mail is gone server-side before
        // the sweep can re-insert it (same-pass resurrection). flag_outbox
        // stays post-sync (Step 7b): disjoint by construction — enqueue
        // consumes same-key flag ops — so flag toggles never race moves.
        let live_pre: HashSet<u32> = server_uids.iter().copied().collect();
        let imap_replay = self
            .replay_imap_outbox(
                &mut *session,
                mailbox_id,
                mailbox_name,
                summary.uid_validity,
                Some(&live_pre),
            )
            .await?;
        eprintln!(
            "[SGE sync] imap replay: acked={} dropped={} failed={} moved={}",
            imap_replay.acked, imap_replay.dropped, imap_replay.failed, imap_replay.moved
        );
        result.moved += imap_replay.moved;

        // ── Step 4c: dirty-draft flush (Plan 12-02) ──
        // The session is SELECTed to `mailbox_name`; only draft rows
        // belonging to this folder flush (SELECT discipline, T-12-04 —
        // the expunge-old leg addresses the selected folder). Draft
        // APPENDs don't interact with the sweep's UID set, so pre-sweep
        // placement next to the imap replay is safe.
        let draft_acked = self
            .replay_dirty_drafts(
                &mut *session,
                mailbox_id,
                mailbox_name,
                summary.uid_validity,
            )
            .await?;
        if draft_acked > 0 {
            eprintln!("[SGE sync] draft replay: acked={draft_acked}");
        }

        // ── Step 4d: send-queue flush (Plan 13-02) ──
        // SMTP leg first (zero session use — never takes a manager lease),
        // then uncertain reconcile (SELECTs Sent, restores this mailbox).
        // Crash recovery runs at flush entry; pass order holds:
        // crash reset → imap_outbox replay (4b) → dirty drafts (4c) → flush.
        self.flush_send_queue_in_pass(&mut *session, mailbox_name)
            .await;

        // Pending-wins gate: UIDs with an unacknowledged optimistic toggle
        // (flag queue) or delete/move (imap queue) keep their local flags
        // through the sweep below (FLAG-02). Fetched once per pass — the
        // sets are small (one row per touched message).
        let pending: HashSet<u32> = {
            let guard = self.store.lock().unwrap();
            let mut pending: HashSet<u32> = queries::pending_uids(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("pending_uids: {e}")))?
                .into_iter()
                .collect();
            pending.extend(
                queries::pending_imap_uids(guard.conn(), mailbox_id)
                    .map_err(|e| SyncError::Protocol(format!("pending_imap_uids: {e}")))?,
            );
            pending
        };

        // ── Step 5: header sweep (BATCH_SIZE UIDs at a time) ──────
        // Phase 9 convergence shortcut: when the server UID set already
        // matches local, with no epoch bump and no pending outbox ops, the
        // sweep is skipped — two consecutive unchanged polls issue zero
        // message FETCHes. A periodic full sweep still runs so remote flag
        // changes on an unchanged UID set surface regularly.
        let local_uids: HashSet<u32> = {
            let guard = self.store.lock().unwrap();
            queries::all_local_uids(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("local uids: {e}")))?
                .into_iter()
                .collect()
        };
        let server_set: HashSet<u32> = server_uids.iter().copied().collect();
        let tombstoned: HashSet<u32> = {
            let guard = self.store.lock().unwrap();
            queries::prune_tombstones(guard.conn(), mailbox_id, &server_uids)
                .map_err(|e| SyncError::Protocol(format!("prune tombstones: {e}")))?;
            queries::tombstoned_uids(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("tombstoned uids: {e}")))?
                .into_iter()
                .collect()
        };
        let sweeps = {
            let guard = self.store.lock().unwrap();
            queries::sweeps_since_full(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("sweep counter: {e}")))?
        };
        let periodic_full = sweeps >= queries::FULL_SWEEP_EVERY;
        let skip_sweep = !periodic_full
            && !uid_validity_bump
            && pending.is_empty()
            && local_uids == server_set;
        // Sweep set: tombstoned UIDs are excluded (no infinite backfill
        // loop) except on the periodic full sweep, which retries them.
        // A converged skip sweeps nothing at all.
        let sweep_uids: Vec<u32> = if skip_sweep {
            Vec::new()
        } else if periodic_full {
            server_uids.clone()
        } else {
            server_uids
                .iter()
                .copied()
                .filter(|u| !tombstoned.contains(u))
                .collect()
        };
        {
            let guard = self.store.lock().unwrap();
            queries::set_sweeps_since_full(
                guard.conn(),
                mailbox_id,
                if skip_sweep { sweeps + 1 } else { 0 },
            )
            .map_err(|e| SyncError::Protocol(format!("sweep counter write: {e}")))?;
        }
        if skip_sweep {
            result.converged = true;
            eprintln!(
                "[SGE sync] converged: {} uids unchanged, no FETCH issued (skip #{})",
                server_uids.len(),
                sweeps + 1
            );
        }
        let total_messages = sweep_uids.len() as u32;

        for chunk in sweep_uids.chunks(BATCH_SIZE as usize) {
            // Cooperative cancel: abort cleanly between batches — no
            // partial sync_state write, session logged out, UI notified.
            if self.cancelled() {
                eprintln!("[SGE sync] cancelled mid-sweep — aborting without state write");
                session.logout().await?;
                cb(SyncEvent::SyncError {
                    detail: "sync cancelled".to_string(),
                });
                return Ok(result);
            }
            let range_str = chunk
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",");

            let display_range = if chunk.len() == 1 {
                format!("{}", chunk[0])
            } else {
                format!("{}-{}", chunk[0], chunk[chunk.len() - 1])
            };

            cb(SyncEvent::BatchStarted {
                range: display_range.clone(),
                server_total: total_messages,
            });

            let headers: Vec<MessageHeader> = session.fetch_envelopes(&range_str).await?;
            eprintln!(
                "[SGE sync] FETCH {}: {} headers",
                display_range,
                headers.len()
            );
            // async-imap's FETCH stream ends silently (no error) when the
            // server rejects the command with NO/BAD — that once produced a
            // "successful" 0-message sync. A non-empty request must yield
            // headers; otherwise fail loudly instead of syncing nothing.
            if headers.is_empty() {
                return Err(SyncError::Protocol(format!(
                    "UID FETCH {range_str} returned no headers for {} requested UID(s) — \
                     server may have rejected the command (see FETCH log line above)",
                    chunk.len()
                )));
            }
            result.fetched += headers.len();

            // Phase 9 range-diff: the batch asked for `chunk` but got fewer
            // headers (partial drop — arrivals mid-sweep or flaky path).
            // Re-request exactly the gap set once, in this pass, instead of
            // waiting for the next poll. Still-missing UIDs earn a strike;
            // at TOMBSTONE_STRIKES they stop being re-requested.
            let mut headers = headers;
            let returned: HashSet<u32> = headers.iter().map(|h| h.uid).collect();
            let gap: Vec<u32> = chunk.iter().copied().filter(|u| !returned.contains(u)).collect();
            if !gap.is_empty() {
                let gap_str = gap
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                eprintln!(
                    "[SGE sync] gap detected: {} of {} missing ({gap_str}) — re-fetching",
                    gap.len(),
                    chunk.len()
                );
                let gap_headers: Vec<MessageHeader> =
                    session.fetch_envelopes(&gap_str).await?;
                result.fetched += gap_headers.len();
                result.gap_refetches += 1;
                let gap_returned: HashSet<u32> =
                    gap_headers.iter().map(|h| h.uid).collect();
                headers.extend(gap_headers);
                let still_missing: Vec<u32> = gap
                    .iter()
                    .copied()
                    .filter(|u| !gap_returned.contains(u))
                    .collect();
                if !still_missing.is_empty() {
                    let guard = self.store.lock().unwrap();
                    for uid in still_missing {
                        match queries::record_fetch_strike(guard.conn(), mailbox_id, uid) {
                            Ok(strikes) => eprintln!(
                                "[SGE sync] uid {uid} still missing after refetch (strike {strikes})"
                            ),
                            Err(e) => eprintln!("[SGE sync] strike write failed ({e})"),
                        }
                    }
                }
            }

            let batch_uids: Vec<u32> = headers.iter().map(|h| h.uid).collect();
            let existing = {
                let guard = self.store.lock().unwrap();
                let conn = guard.conn();
                queries::existing_uids(conn, mailbox_id, &batch_uids)
                    .map_err(|e| SyncError::Protocol(format!("existing_uids: {e}")))?
            };
            let existing_set: HashSet<u32> = existing.into_iter().collect();

            {
                let guard = self.store.lock().unwrap();
                let conn = guard.conn();
                for header in &headers {
                    let uid = header.uid;
                    let message_id = header.message_id.as_deref();

                    // Pending-wins: a queued optimistic toggle owns this
                    // row's flags — the server sweep must not clobber it.
                    // Every other column still writes through.
                    let flags = if pending.contains(&uid) {
                        queries::message_flags(conn, mailbox_id, uid)
                            .map_err(|e| {
                                SyncError::Protocol(format!("pending flags uid {uid}: {e}"))
                            })?
                            .unwrap_or_else(|| header.flags.clone())
                    } else {
                        header.flags.clone()
                    };

                    queries::upsert_message(
                        conn,
                        mailbox_id,
                        uid,
                        message_id,
                        &header.subject,
                        &header.from_addr,
                        &header.to_addrs,
                        &header.cc_addrs,
                        &header.date_utc,
                        &flags,
                        header.has_attachments,
                        &header.preview,
                    )
                    .map_err(|e| SyncError::Protocol(format!("upsert uid {uid}: {e}")))?;

                    // Fetched OK — any prior strike is stale (recovered).
                    let _ = queries::clear_tombstone(conn, mailbox_id, uid);

                    if existing_set.contains(&uid) {
                        // Unchanged vs updated: a stored row whose flags
                        // already match the server sweep is unchanged (flags
                        // are the convergence signal; envelope bytes are not
                        // retained). Pending-wins rows always count as
                        // updated — the local optimistic write goes through.
                        if pending.contains(&uid) {
                            result.updated += 1;
                        } else {
                            let stored = queries::message_flags(conn, mailbox_id, uid)
                                .map_err(|e| {
                                    SyncError::Protocol(format!("stored flags uid {uid}: {e}"))
                                })?;
                            if stored.as_deref() == Some(header.flags.as_str()) {
                                result.unchanged += 1;
                            } else {
                                result.updated += 1;
                            }
                        }
                        cb(SyncEvent::MessageSynced {
                            uid,
                            flag: SyncFlag::Updated,
                        });
                    } else {
                        result.new += 1;
                        cb(SyncEvent::MessageSynced {
                            uid,
                            flag: SyncFlag::New,
                        });
                    }
                }
            }

            cb(SyncEvent::BatchCompleted {
                new: result.new,
                updated: result.updated,
                deleted: result.deleted,
                moved: result.moved,
                expunged: result.expunged,
            });
        }

        // ── Step 6: expunge diff (delete UIDs no longer on server) ──
        // Tombstoned UIDs are still on the server (SEARCH lists them) —
        // they join the live set so the skip doesn't read as an expunge.
        let live_union: Vec<u32> = server_uids
            .iter()
            .copied()
            .chain(tombstoned.iter().copied())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        result.deleted = self.expunge_absent(mailbox_id, &live_union, summary.uid_validity)?;

        // ── Step 7: write sync state + logout ──────────────────────
        let new_next = summary.uid_next.unwrap_or_else(|| {
            server_uids.last().map(|&u| u + 1).unwrap_or(1)
        });
        {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            queries::set_sync_state(conn, mailbox_name, summary.uid_validity, new_next)
                .map_err(|e| SyncError::Protocol(format!("set sync state: {e}")))?;
        }

        // Post-sync flag replay: queued Seen toggles go out on the
        // still-open session; failures stay queued for the next pass. The
        // imap queue already replayed pre-sweep (Step 4b) — the flag queue
        // stays here by design (disjoint queues, documented at Step 4b).
        // A replay failure never fails the sync itself.
        let live: HashSet<u32> = server_uids.iter().copied().collect();
        let replay = self
            .replay_outbox(&mut *session, mailbox_id, summary.uid_validity, Some(&live))
            .await?;
        eprintln!(
            "[SGE sync] replay: acked={} dropped={} failed={}",
            replay.acked, replay.dropped, replay.failed
        );

        session.logout().await?;
        {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            let db_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE mailbox_id = ?1",
                    rusqlite::params![mailbox_id],
                    |r| r.get(0),
                )
                .unwrap_or(-1);
            eprintln!(
                "[SGE sync] done: new={} updated={} deleted={} moved={} db_rows={} fetched={} gap_refetches={} converged={}",
                result.new, result.updated, result.deleted, result.moved, db_count,
                result.fetched, result.gap_refetches, result.converged
            );
        }
        cb(SyncEvent::SyncCompleted {
            summary: result.clone(),
        });
        Ok(result)
    }
}

// ── test fixtures ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::{MailboxInfo, MailboxStatus, PinBox};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Deterministic `MockSession` — returns canned headers and tracks
    /// call counts so tests can assert "no IMAP on cache hit".
    pub struct MockSession {
        pub summary: MailboxSummary,
        pub envelopes: Vec<MessageHeader>,
        pub fetch_calls: AtomicUsize,
        pub logout_called: AtomicBool,
        /// Recorded `(uid, seen)` pairs from `set_seen` — asserts the flag
        /// path addresses messages by UID (T-6-01), never by sequence number.
        pub set_seen_calls: Vec<(u32, bool)>,
        /// When true, `set_seen` fails — drives replay-failure tests.
        pub fail_set_seen: bool,
        /// STATUS UNSEEN datum returned by `mailbox_status` (FOLD-02).
        pub status_unseen: u32,
        /// Requested ranges per `fetch_envelopes` call (gap/tombstone tests).
        pub fetch_ranges: Mutex<Vec<String>>,
        /// UIDs omitted from the FIRST fetch only (transient gap → refetch recovers).
        pub gap_once: Mutex<Vec<u32>>,
        /// UIDs omitted from EVERY fetch (flaky server path → tombstoned).
        pub always_miss: Mutex<Vec<u32>>,
        /// Mailbox names passed to `select_mailbox` — asserts the Plan 10-02
        /// lease-hold rule (no intermediate SELECT inside a fallback run).
        pub select_calls: Vec<String>,
        /// Recorded `(uid, deleted)` pairs from `store_deleted` — asserts
        /// UID-only addressing on the Deleted path (T-10-01).
        pub deleted_calls: Vec<(u32, bool)>,
        /// Recorded `(uid_set, dest)` pairs from `uid_copy_to`.
        pub copied_calls: Vec<(String, String)>,
        /// Recorded `(uid_set, dest)` pairs from `uid_move_to`.
        pub moved_calls: Vec<(String, String)>,
        /// UID sets passed to `uid_expunge`, in call order.
        pub expunged_sets: Vec<String>,
        /// Bare-`expunge()` invocations (must stay 0 in UID-scoped paths).
        pub plain_expunge_calls: usize,
        /// Mailbox names passed to `create_mailbox` (Trash-CREATE path).
        pub created_mailboxes: Vec<String>,
        /// Recorded `(old, new)` pairs from `rename_mailbox` (Plan 11-02).
        pub renamed_calls: Vec<(String, String)>,
        /// Mailbox names passed to `delete_mailbox` (Plan 11-02).
        pub deleted_mailboxes: Vec<String>,
        /// Canned `CAPABILITY` atoms (default MOVE + UIDPLUS).
        pub canned_capabilities: Vec<String>,
        /// When true, the matching verb fails — drives fallback tests.
        pub fail_deleted: bool,
        pub fail_expunge: bool,
        pub fail_copy: bool,
        pub fail_move: bool,
        pub fail_create: bool,
        /// When true, `rename_mailbox` / `delete_mailbox` fail (Plan 11-02
        /// folder-verb arms; the worker never drives them, but the trait
        /// requires the methods).
        pub fail_rename: bool,
        pub fail_delete: bool,
        /// When true, `uid_move_to` refuses loudly (`SyncError::Refused`,
        /// the unmark-dance shape) — drives the replay refusal-cleanup test.
        pub fail_move_refused: bool,
        /// Recorded `(mailbox, flags, bytes)` from `append_message`
        /// (Plan 12-01 draft APPEND).
        pub appended_calls: Vec<(String, String, Vec<u8>)>,
        /// Canned `UID SEARCH HEADER` results (Plan 12-01 reconcile;
        /// Plan 12-02 extends with per-Message-ID results for the
        /// reconnect flush).
        pub search_header_results: Vec<u32>,
        /// Per-Message-ID canned SEARCH results, keyed by the searched
        /// value: when non-empty for a value, wins over
        /// `search_header_results` (multi-draft flush — each row
        /// reconciles its own UID).
        pub search_header_by_msgid: HashMap<String, Vec<u32>>,
        /// When true, `append_message` fails — drives draft replay tests.
        pub fail_append: bool,
    }

    impl SyncSession for MockSession {
        fn select_mailbox(
            &mut self,
            name: &str,
        ) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
            self.select_calls.push(name.to_string());
            let summary = self.summary.clone();
            Box::pin(async move { Ok(summary) })
        }

        fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            let uids = self.envelopes.iter().map(|h| h.uid).collect();
            Box::pin(async move { Ok(uids) })
        }

        fn fetch_envelopes<'a>(
            &'a mut self,
            range: &'a str,
        ) -> PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
            self.fetch_calls.fetch_add(1, Ordering::SeqCst);
            self.fetch_ranges.lock().unwrap().push(range.to_string());
            // Honor the requested set like a real server: only requested
            // UIDs come back. `gap_once` drops UIDs on the first call only
            // (transient drop → range-diff refetch recovers); `always_miss`
            // drops them every call (flaky path → strikes → tombstoned).
            let requested: HashSet<u32> = range
                .split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .collect();
            let once: Vec<u32> =
                std::mem::take(&mut *self.gap_once.lock().unwrap());
            let always = self.always_miss.lock().unwrap().clone();
            let envelopes = self
                .envelopes
                .iter()
                .filter(|h| requested.contains(&h.uid))
                .filter(|h| !once.contains(&h.uid) && !always.contains(&h.uid))
                .cloned()
                .collect();
            Box::pin(async move { Ok(envelopes) })
        }

        fn fetch_body(&mut self, _uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>> {
            self.set_seen_calls.push((uid, seen));
            if self.fail_set_seen {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock set_seen failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn store_deleted(&mut self, uid: u32, deleted: bool) -> PinBox<'_, Result<(), SyncError>> {
            self.deleted_calls.push((uid, deleted));
            if self.fail_deleted {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock store_deleted failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            self.plain_expunge_calls += 1;
            if self.fail_expunge {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock expunge failure".to_string()))
                });
            }
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn uid_expunge(&mut self, uid_set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            self.expunged_sets.push(uid_set.to_string());
            if self.fail_expunge {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock uid_expunge failure".to_string()))
                });
            }
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn uid_copy_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
            self.copied_calls.push((uid_set.to_string(), dest.to_string()));
            if self.fail_copy {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock uid_copy failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn uid_move_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
            self.moved_calls.push((uid_set.to_string(), dest.to_string()));
            if self.fail_move_refused {
                return Box::pin(async move {
                    Err(SyncError::Refused("mock unmark dance unverifiable".to_string()))
                });
            }
            if self.fail_move {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock uid_move failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn capabilities(&mut self) -> PinBox<'_, Result<Vec<String>, SyncError>> {
            let caps = self.canned_capabilities.clone();
            Box::pin(async move { Ok(caps) })
        }

        fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
            self.created_mailboxes.push(name.to_string());
            if self.fail_create {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock create failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>> {
            self.renamed_calls.push((old.to_string(), new.to_string()));
            if self.fail_rename {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock rename failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
            self.deleted_mailboxes.push(name.to_string());
            if self.fail_delete {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock delete failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn append_message(
            &mut self,
            mailbox: &str,
            flags: &str,
            bytes: &[u8],
        ) -> PinBox<'_, Result<(), SyncError>> {
            self.appended_calls.push((
                mailbox.to_string(),
                flags.to_string(),
                bytes.to_vec(),
            ));
            if self.fail_append {
                return Box::pin(async move {
                    Err(SyncError::Protocol("mock append failure".to_string()))
                });
            }
            Box::pin(async move { Ok(()) })
        }

        fn uid_search_header(
            &mut self,
            _field: &str,
            value: &str,
        ) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            let mut uids = self
                .search_header_by_msgid
                .get(value)
                .cloned()
                .unwrap_or_else(|| self.search_header_results.clone());
            uids.sort_unstable();
            Box::pin(async move { Ok(uids) })
        }

        fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
            Box::pin(async move { Ok(vec![]) })
        }

        fn mailbox_status(&mut self, _name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>> {
            let status = MailboxStatus {
                uid_validity: self.summary.uid_validity,
                uid_next: self.summary.uid_next,
                unseen: self.status_unseen,
                // The worker never drives folder verbs: no MESSAGES fixture
                // needed here (folder guards read it via the manager stub).
                messages: 0,
            };
            Box::pin(async move { Ok(status) })
        }

        fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
            self.logout_called.store(true, Ordering::SeqCst);
            Box::pin(async move { Ok(()) })
        }
    }

    /// Build a minimal `MessageHeader` with deterministic fields.
    fn mkhdr(uid: u32) -> MessageHeader {
        MessageHeader {
            uid,
            message_id: Some(format!("<msg{uid}@example.com>")),
            subject: format!("Subject {uid}"),
            from_addr: "alice@example.com".to_string(),
            to_addrs: String::new(),
            cc_addrs: String::new(),
            date_utc: "2024-10-03T12:00:00Z".to_string(),
            flags: "[]".to_string(),
            has_attachments: false,
            preview: format!("Subject {uid}"),
        }
    }

    fn cb() -> SyncCallback {
        Arc::new(|_| {})
    }

    fn mock(summary: MailboxSummary, envelopes: Vec<MessageHeader>) -> MockSession {
        MockSession {
            summary,
            envelopes,
            fetch_calls: AtomicUsize::new(0),
            logout_called: AtomicBool::new(false),
            set_seen_calls: Vec::new(),
            fail_set_seen: false,
            status_unseen: 0,
            fetch_ranges: Mutex::new(Vec::new()),
            gap_once: Mutex::new(Vec::new()),
            always_miss: Mutex::new(Vec::new()),
            select_calls: Vec::new(),
            deleted_calls: Vec::new(),
            copied_calls: Vec::new(),
            moved_calls: Vec::new(),
            expunged_sets: Vec::new(),
            plain_expunge_calls: 0,
            created_mailboxes: Vec::new(),
            renamed_calls: Vec::new(),
            deleted_mailboxes: Vec::new(),
            canned_capabilities: vec![
                "IMAP4rev1".to_string(),
                "UIDPLUS".to_string(),
                "MOVE".to_string(),
            ],
            fail_deleted: false,
            fail_expunge: false,
            fail_copy: false,
            fail_move: false,
            fail_create: false,
            fail_rename: false,
            fail_delete: false,
            fail_move_refused: false,
            appended_calls: Vec::new(),
            search_header_results: Vec::new(),
            search_header_by_msgid: HashMap::new(),
            fail_append: false,
        }
    }

    fn inbox(_summary: MailboxSummary) -> Arc<std::sync::Mutex<Store>> {
        Arc::new(std::sync::Mutex::new(
            Store::open_in_memory().expect("migration should succeed"),
        ))
    }

    /// Seed one dirty draft row in `mailbox`; returns its mailbox id.
    fn draft_row(
        store: &Arc<std::sync::Mutex<Store>>,
        mailbox: &str,
        id: &str,
        msg_id: &str,
    ) -> u64 {
        let guard = store.lock().unwrap();
        let mb = queries::ensure_mailbox(guard.conn(), mailbox).unwrap();
        queries::upsert_draft(
            guard.conn(),
            id,
            mb,
            msg_id,
            "Subject",
            "Body",
            "to@example.com",
            "",
            "",
        )
        .unwrap();
        mb
    }

    fn drafts_summary() -> MailboxSummary {
        MailboxSummary {
            selected_mailbox: "Drafts".to_string(),
            uid_validity: 100,
            uid_next: Some(2),
            exists: 0,
        }
    }

    /// Plan 12-02 reconnect flush: dirty row + canned SEARCH `[5]` →
    /// clean with `server_uid=5`. First save (no tracked UID) issues no
    /// delete/expunge legs.
    #[test]
    fn replay_dirty_drafts_acks_and_cleans() {
        let store = inbox(drafts_summary());
        let drafts_mb = draft_row(&store, "Drafts", "compose-1", "<c1@sge.local>");
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(drafts_summary(), vec![]);
        session.search_header_results = vec![5];
        let acked = async_std::task::block_on(async {
            worker
                .replay_dirty_drafts(&mut session, drafts_mb, "Drafts", 100)
                .await
        })
        .unwrap();

        assert_eq!(acked, 1);
        assert_eq!(session.appended_calls.len(), 1);
        assert_eq!(session.appended_calls[0].0, "Drafts");
        assert!(session.deleted_calls.is_empty());
        assert!(session.expunged_sets.is_empty());
        let guard = store.lock().unwrap();
        let row = queries::get_draft(guard.conn(), "compose-1")
            .unwrap()
            .unwrap();
        assert!(!row.dirty);
        assert_eq!(row.server_uid, Some(5));
    }

    /// Failing APPEND → row stays `dirty=1`, the pass itself still
    /// returns Ok (worker replay discipline — never fails the sync).
    #[test]
    fn replay_dirty_drafts_append_failure_stays_dirty() {
        let store = inbox(drafts_summary());
        let drafts_mb = draft_row(&store, "Drafts", "compose-1", "<c1@sge.local>");
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(drafts_summary(), vec![]);
        session.fail_append = true;
        let acked = async_std::task::block_on(async {
            worker
                .replay_dirty_drafts(&mut session, drafts_mb, "Drafts", 100)
                .await
        })
        .unwrap();

        assert_eq!(acked, 0);
        let guard = store.lock().unwrap();
        let row = queries::get_draft(guard.conn(), "compose-1")
            .unwrap()
            .unwrap();
        assert!(row.dirty);
        assert_eq!(row.server_uid, None);
    }

    /// SELECT discipline: rows belonging to another folder never flush
    /// on this pass (the expunge-old leg addresses the selected folder,
    /// T-12-04) — they wait for their own folder's pass.
    #[test]
    fn replay_dirty_drafts_skips_other_mailbox_rows() {
        let store = inbox(drafts_summary());
        draft_row(&store, "INBOX", "compose-9", "<c9@sge.local>");
        let drafts_mb = {
            let guard = store.lock().unwrap();
            queries::ensure_mailbox(guard.conn(), "Drafts").unwrap()
        };
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(drafts_summary(), vec![]);
        session.search_header_results = vec![5];
        let acked = async_std::task::block_on(async {
            worker
                .replay_dirty_drafts(&mut session, drafts_mb, "Drafts", 100)
                .await
        })
        .unwrap();

        assert_eq!(acked, 0);
        assert!(session.appended_calls.is_empty());
        let guard = store.lock().unwrap();
        let row = queries::get_draft(guard.conn(), "compose-9")
            .unwrap()
            .unwrap();
        assert!(row.dirty);
    }

    /// Re-save with a tracked UID APPENDs-new and expunges the
    /// superseded copy (exactly-one-copy invariant on the flush path).
    #[test]
    fn replay_dirty_drafts_expunges_superseded_copy() {
        let store = inbox(drafts_summary());
        let drafts_mb = draft_row(&store, "Drafts", "compose-1", "<c1@sge.local>");
        {
            let guard = store.lock().unwrap();
            queries::mark_draft_clean(guard.conn(), "compose-1", 7).unwrap();
            queries::upsert_draft(
                guard.conn(),
                "compose-1",
                drafts_mb,
                "<c1@sge.local>",
                "Subject v2",
                "Body v2",
                "to@example.com",
                "",
                "",
            )
            .unwrap();
            queries::set_sync_state(guard.conn(), "Drafts", 100, 8).unwrap();
        }
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(drafts_summary(), vec![]);
        session.search_header_results = vec![9];
        let acked = async_std::task::block_on(async {
            worker
                .replay_dirty_drafts(&mut session, drafts_mb, "Drafts", 100)
                .await
        })
        .unwrap();

        assert_eq!(acked, 1);
        assert!(session.deleted_calls.contains(&(7, true)));
        assert!(session.expunged_sets.contains(&"7".to_string()));
        let guard = store.lock().unwrap();
        let row = queries::get_draft(guard.conn(), "compose-1")
            .unwrap()
            .unwrap();
        assert!(!row.dirty);
        assert_eq!(row.server_uid, Some(9));
    }

    /// UIDVALIDITY bump since the last sync → the tracked `server_uid`
    /// is stale (`old_uid=None` path): APPEND fresh, no expunge of the
    /// dead-generation UID (the orphan is reaped by the sweep).
    #[test]
    fn replay_dirty_drafts_epoch_bump_drops_stale_old_uid() {
        let store = inbox(drafts_summary());
        let drafts_mb = draft_row(&store, "Drafts", "compose-1", "<c1@sge.local>");
        {
            let guard = store.lock().unwrap();
            queries::mark_draft_clean(guard.conn(), "compose-1", 7).unwrap();
            queries::upsert_draft(
                guard.conn(),
                "compose-1",
                drafts_mb,
                "<c1@sge.local>",
                "Subject v2",
                "Body v2",
                "to@example.com",
                "",
                "",
            )
            .unwrap();
            queries::set_sync_state(guard.conn(), "Drafts", 99, 8).unwrap();
        }
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(drafts_summary(), vec![]);
        session.search_header_results = vec![9];
        let acked = async_std::task::block_on(async {
            worker
                .replay_dirty_drafts(&mut session, drafts_mb, "Drafts", 100)
                .await
        })
        .unwrap();

        assert_eq!(acked, 1);
        assert!(session.deleted_calls.is_empty());
        assert!(session.expunged_sets.is_empty());
        let guard = store.lock().unwrap();
        let row = queries::get_draft(guard.conn(), "compose-1")
            .unwrap()
            .unwrap();
        assert!(!row.dirty);
        assert_eq!(row.server_uid, Some(9));
    }

    /// Multi-draft flush: per-Message-ID canned SEARCH results reconcile
    /// each row to its own UID (the shared flat vec cannot express this).
    #[test]
    fn replay_dirty_drafts_reconciles_each_row_by_message_id() {
        let store = inbox(drafts_summary());
        let drafts_mb = draft_row(&store, "Drafts", "compose-1", "<c1@sge.local>");
        draft_row(&store, "Drafts", "compose-2", "<c2@sge.local>");
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(drafts_summary(), vec![]);
        session
            .search_header_by_msgid
            .insert("<c1@sge.local>".to_string(), vec![5]);
        session
            .search_header_by_msgid
            .insert("<c2@sge.local>".to_string(), vec![8]);
        let acked = async_std::task::block_on(async {
            worker
                .replay_dirty_drafts(&mut session, drafts_mb, "Drafts", 100)
                .await
        })
        .unwrap();

        assert_eq!(acked, 2);
        assert_eq!(session.appended_calls.len(), 2);
        let guard = store.lock().unwrap();
        let one = queries::get_draft(guard.conn(), "compose-1")
            .unwrap()
            .unwrap();
        let two = queries::get_draft(guard.conn(), "compose-2")
            .unwrap()
            .unwrap();
        assert_eq!(one.server_uid, Some(5));
        assert_eq!(two.server_uid, Some(8));
        assert!(!one.dirty && !two.dirty);
    }

    /// Three new messages → all appear as `new`.
    #[test]
    fn full_sync_inserts_three_headers() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        });
        let worker = SyncWorker::new(store);

        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        };
        let session = mock(summary, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);

        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 3);
        assert_eq!(result.updated, 0);
        assert_eq!(result.deleted, 0);
        assert!(!result.uid_validity_bump);

        // Persisted rows
        let count: i64 = worker
            .store
            .lock()
            .unwrap()
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3);
    }

    /// Plan 10-02 precondition entry: the worker borrows a lease-held
    /// session (`&mut dyn SyncSession`) instead of consuming a fresh
    /// connection — the `start_sync`-under-lease path. Borrowed, not
    /// consumed: the caller still owns the session afterwards.
    #[test]
    fn sync_with_borrowed_runs_full_pass_on_lease_session() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        });
        let worker = SyncWorker::new(store);

        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        };
        let mut session = mock(summary, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);

        let result = async_std::task::block_on(async {
            worker.sync_with_borrowed(&mut session, "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 3);
        assert_eq!(result.updated, 0);
        assert_eq!(result.deleted, 0);
        // The borrowed session drove the whole pass (SELECT + sweep).
        assert_eq!(session.select_calls, vec!["INBOX".to_string()]);
        assert!(session.logout_called.load(Ordering::SeqCst));
    }

    /// Second run with same data → converged (no FETCH); the periodic full
    /// sweep still refreshes (update path intact).
    #[test]
    fn incremental_sync_reports_updates() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        });
        let worker = SyncWorker::new(store.clone());

        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        };

        // First run — inserts
        let session = mock(summary.clone(), vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        // Second run — same data → converged, zero FETCHes (Phase 9).
        let session2 = mock(summary.clone(), vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session2), "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 0);
        assert_eq!(result.updated, 0);
        assert_eq!(result.deleted, 0);
        assert!(result.converged);
        assert_eq!(result.fetched, 0);

        // Force the periodic full sweep → update path still refreshes.
        {
            let guard = worker.store.lock().unwrap();
            let mb = queries::mailbox_id(guard.conn(), "INBOX").unwrap().unwrap();
            queries::set_sweeps_since_full(guard.conn(), mb, queries::FULL_SWEEP_EVERY).unwrap();
        }
        let session3 = mock(summary, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        let result3 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session3), "INBOX", cb()).await
        })
        .unwrap();

        assert!(!result3.converged);
        assert_eq!(result3.new, 0);
        assert_eq!(result3.unchanged, 3);
        assert_eq!(result3.updated, 0);
        assert_eq!(result3.deleted, 0);
    }

    /// UIDVALIDITY change → wipe + full resync.
    #[test]
    fn uidvalidity_bump_triggers_wipe() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        });
        let worker = SyncWorker::new(store.clone());

        // First run with UIDVALIDITY=100
        let s1 = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        };
        let session = mock(s1, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        async_std::task::block_on(async { worker.sync_with_session(Box::new(session), "INBOX", cb()).await })
            .unwrap();

        // Second run with UIDVALIDITY=200 → wipe
        let s2 = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 200,
            uid_next: Some(4),
            exists: 3,
        };
        let session2 = mock(s2, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        let result = async_std::task::block_on(async {
            worker
                .sync_with_session(Box::new(session2), "INBOX", cb())
                .await
        })
        .unwrap();

        assert!(result.uid_validity_bump);
        assert_eq!(result.new, 3);
        assert_eq!(result.updated, 0);

        // Sync state should be updated to new UIDVALIDITY
        let state: (u32, u32) = worker
            .store
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT uid_validity, uid_next FROM mailboxes WHERE name = 'INBOX'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state.0, 200, "UIDVALIDITY should be updated to 200");
    }

    /// Empty mailbox → no fetches, clean state.
    #[test]
    fn empty_mailbox_shortcuts() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 50,
            uid_next: Some(1),
            exists: 0,
        });
        let worker = SyncWorker::new(store);

        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 50,
            uid_next: Some(1),
            exists: 0,
        };
        let session = mock(summary, vec![]);

        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 0);
        assert_eq!(result.deleted, 0);
        assert!(!result.uid_validity_bump);
    }

    /// The mock flag path records UID-addressed writes (T-6-01): the UID
    /// reaching `set_seen` is the message UID from the local DB row, and
    /// no sequence-number store call exists on any flag path.
    #[test]
    fn mock_set_seen_records_uid_store_calls() {
        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        };
        let mut session = mock(summary, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);

        async_std::task::block_on(async {
            session.set_seen(42, true).await.unwrap();
            session.set_seen(42, false).await.unwrap();
            session.set_seen(7, true).await.unwrap();
        });

        assert_eq!(
            session.set_seen_calls,
            vec![(42, true), (42, false), (7, true)],
            "flag writes must carry the message UID, never a sequence number"
        );
    }

    /// The Deleted flag path records UID-addressed writes (T-10-01): the
    /// UID reaching `store_deleted` is the message UID, and no
    /// sequence-number store call exists on the delete path.
    #[test]
    fn mock_store_deleted_records_uid_calls() {
        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 3,
        };
        let mut session = mock(summary, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);

        async_std::task::block_on(async {
            session.store_deleted(42, true).await.unwrap();
            session.store_deleted(42, false).await.unwrap();
            session.store_deleted(7, true).await.unwrap();
        });

        assert_eq!(
            session.deleted_calls,
            vec![(42, true), (42, false), (7, true)],
            "Deleted writes must carry the message UID, never a sequence number"
        );
    }

    /// New transport arms record their wire arguments, honor `fail_*`
    /// toggles, and report the canned capability set (T-10-01). The
    /// `select_mailbox` recording backs the Plan 10-02 lease-hold test.
    #[test]
    fn mock_delete_move_arms_record_and_fail() {
        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 2,
        };
        let mut session = mock(summary, vec![mkhdr(1), mkhdr(2)]);

        async_std::task::block_on(async {
            session.select_mailbox("INBOX").await.unwrap();
            session.uid_copy_to("1,2", "Trash").await.unwrap();
            session.uid_move_to("1,2", "Trash").await.unwrap();
            session.uid_expunge("1,2").await.unwrap();
            session.expunge().await.unwrap();
            session.create_mailbox("Trash").await.unwrap();
            let caps = session.capabilities().await.unwrap();
            assert_eq!(caps, vec!["IMAP4rev1".to_string(), "UIDPLUS".to_string(), "MOVE".to_string()]);
        });

        assert_eq!(session.select_calls, vec!["INBOX".to_string()]);
        assert_eq!(
            session.copied_calls,
            vec![("1,2".to_string(), "Trash".to_string())]
        );
        assert_eq!(
            session.moved_calls,
            vec![("1,2".to_string(), "Trash".to_string())]
        );
        assert_eq!(session.expunged_sets, vec!["1,2".to_string()]);
        assert_eq!(session.plain_expunge_calls, 1);
        assert_eq!(session.created_mailboxes, vec!["Trash".to_string()]);

        // Failure toggles surface errors without recording loss.
        session.fail_deleted = true;
        session.fail_expunge = true;
        session.fail_copy = true;
        session.fail_move = true;
        session.fail_create = true;
        async_std::task::block_on(async {
            assert!(session.store_deleted(1, true).await.is_err());
            assert!(session.uid_expunge("1").await.is_err());
            assert!(session.expunge().await.is_err());
            assert!(session.uid_copy_to("1", "Trash").await.is_err());
            assert!(session.uid_move_to("1", "Trash").await.is_err());
            assert!(session.create_mailbox("Trash").await.is_err());
        });
        // Calls still record under failure (wire attempt happened).
        assert_eq!(session.deleted_calls, vec![(1, true)]);
        assert_eq!(session.expunged_sets.len(), 2);
        assert_eq!(session.plain_expunge_calls, 2);
        assert_eq!(session.copied_calls.len(), 2);
        assert_eq!(session.moved_calls.len(), 2);
        assert_eq!(session.created_mailboxes.len(), 2);
    }

    /// Seed helper: one INBOX row plus an optimistic Seen toggle, returning
    /// the mailbox id. Mirrors what the `set_seen` command writes.
    fn seed_pending(
        store: &Arc<std::sync::Mutex<Store>>,
        uid: u32,
        seen: bool,
        epoch: u32,
    ) -> u64 {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mb_id = queries::ensure_mailbox(conn, "INBOX").unwrap();
        queries::upsert_message(
            conn,
            mb_id,
            uid,
            None,
            &format!("Subject {uid}"),
            "alice@example.com",
            "[]",
            "[]",
            "2024-10-03T12:00:00Z",
            "[]",
            false,
            &format!("Subject {uid}"),
        )
        .unwrap();
        queries::set_local_seen(conn, mb_id, uid, seen).unwrap();
        queries::enqueue_outbox(conn, mb_id, uid, seen, epoch).unwrap();
        mb_id
    }

    /// Concurrent sync preserves a pending optimistic toggle (FLAG-02):
    /// the server sweep still says unseen, but the local Seen flag wins,
    /// and the post-sync replay acks the op through the mock session.
    #[test]
    fn pending_wins_reconcile_preserves_optimistic_flags() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 2,
        });
        // UID 1: optimistic Seen + queued op at epoch 100. UID 2: untouched.
        let mb_id = seed_pending(&store, 1, true, 100);
        {
            let guard = store.lock().unwrap();
            queries::upsert_message(
                guard.conn(),
                mb_id,
                2,
                None,
                "Subject 2",
                "bob@example.com",
                "[]",
                "[]",
                "2024-10-03T12:00:00Z",
                "[]",
                false,
                "Subject 2",
            )
            .unwrap();
        }
        let worker = SyncWorker::new(store.clone());

        // Server still reports both messages unseen (stale FLAGS).
        let summary = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 2,
        };
        let mut hdr1 = mkhdr(1);
        hdr1.flags = "[]".to_string();
        let mut hdr2 = mkhdr(2);
        hdr2.flags = "[]".to_string();
        let session = mock(summary, vec![hdr1, hdr2]);

        async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();
        // The sync consumed the session box; assert through the store: the
        // post-sync replay acked the op through the mock session.
        let guard = store.lock().unwrap();
        let flags1 = queries::message_flags(guard.conn(), mb_id, 1)
            .unwrap()
            .unwrap();
        assert!(
            flags1.contains("\\Seen"),
            "pending UID must keep optimistic Seen, got: {flags1}"
        );
        let flags2 = queries::message_flags(guard.conn(), mb_id, 2)
            .unwrap()
            .unwrap();
        assert_eq!(flags2, "[]", "non-pending UID takes server flags");
        assert!(
            queries::pending_uids(guard.conn(), mb_id).unwrap().is_empty(),
            "acked op must leave the queue"
        );
    }

    /// Replay acks queued ops in creation order and deletes them.
    #[test]
    fn replay_acks_queued_ops_in_order() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 2,
        });
        let mb_id = seed_pending(&store, 5, true, 100);
        seed_pending(&store, 6, false, 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(4),
                exists: 2,
            },
            vec![],
        );
        let live: HashSet<u32> = [5, 6].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 2);
        assert_eq!(summary.dropped, 0);
        assert_eq!(summary.failed, 0);
        assert_eq!(session.set_seen_calls, vec![(5, true), (6, false)]);
        let guard = worker.store.lock().unwrap();
        assert!(queries::pending_uids(guard.conn(), mb_id).unwrap().is_empty());
    }

    /// Epoch mismatch drops the whole mailbox queue without a single STORE.
    #[test]
    fn replay_epoch_mismatch_drops_queue() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 200,
            uid_next: Some(4),
            exists: 1,
        });
        let mb_id = seed_pending(&store, 5, true, 100); // stale epoch
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 200,
                uid_next: Some(4),
                exists: 1,
            },
            vec![],
        );
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 200, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.dropped, 1);
        assert_eq!(summary.acked, 0);
        assert!(
            session.set_seen_calls.is_empty(),
            "stale UIDs must never reach the wire"
        );
    }

    /// Ops whose UID is absent from the server drop without a STORE.
    #[test]
    fn replay_absent_uid_drops_single_op() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 1,
        });
        let mb_id = seed_pending(&store, 5, true, 100);
        seed_pending(&store, 6, false, 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(4),
                exists: 1,
            },
            vec![],
        );
        // UID 6 was expunged server-side.
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 1);
        assert_eq!(summary.dropped, 1);
        assert_eq!(session.set_seen_calls, vec![(5, true)]);
    }

    /// A failed STORE stays queued with attempts + error recorded.
    #[test]
    fn replay_failure_stays_queued_with_error() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 1,
        });
        let mb_id = seed_pending(&store, 5, true, 100);
        let worker = SyncWorker::new(store);

        let summary_mm = MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 1,
        };
        let mut session = mock(summary_mm, vec![]);
        session.fail_set_seen = true;
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.acked, 0);
        let guard = worker.store.lock().unwrap();
        let ops = queries::list_outbox(guard.conn(), mb_id).unwrap();
        assert_eq!(ops.len(), 1, "failed op must stay queued");
        assert_eq!(ops[0].attempts, 1);
        assert!(ops[0].last_error.is_some());
    }

    // ── Phase 6 Plan 06-03: RFC 4549 playback + edge-case regression tests ──
    // Tests prefixed `outbox_rfc4549` are the phase-gate for verify-work; they
    // consolidate the RFC 4549 drop-rule contract in one name-space so the
    // `cargo test outbox_rfc4549` verify command matches them all.

    /// RFC 4549: a UIDVALIDITY generation change invalidates every queued UID
    /// at once — the whole mailbox queue drops before any replay executes, so
    /// stale UIDs never reach the wire against a renumbered mailbox.
    #[test]
    fn outbox_rfc4549_epoch_bump_drops_queue() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 1,
        });
        let mb_id = seed_pending(&store, 5, true, 100); // enqueued at epoch 100
        seed_pending(&store, 6, false, 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 200, // server-side bump
                uid_next: Some(7),
                exists: 2,
            },
            vec![],
        );
        let live: HashSet<u32> = [5, 6].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 200, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.dropped, 2, "both ops must drop on epoch mismatch");
        assert_eq!(summary.acked, 0);
        assert!(
            session.set_seen_calls.is_empty(),
            "stale UIDs must never reach the wire"
        );
        let guard = worker.store.lock().unwrap();
        assert!(
            queries::pending_uids(guard.conn(), mb_id).unwrap().is_empty(),
            "queue must be empty after epoch-bump drop"
        );
    }

    /// RFC 4549: a single op whose UID is absent from the server set drops
    /// without issuing a STORE; the remaining ops still acked normally.
    #[test]
    fn outbox_rfc4549_absent_uid_drops_single_op() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 1,
        });
        let mb_id = seed_pending(&store, 5, true, 100);
        seed_pending(&store, 6, false, 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(7),
                exists: 1,
            },
            vec![],
        );
        // UID 6 was expunged server-side.
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 1);
        assert_eq!(summary.dropped, 1);
        // Only the live UID got a STORE.
        assert_eq!(session.set_seen_calls, vec![(5, true)]);
    }

    /// RFC 4549: replay preserves per-mailbox creation order — ops fire in
    /// the sequence they were enqueued (by `id`), not by UID sorted order.
    #[test]
    fn outbox_rfc4549_preserves_creation_order() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(7),
            exists: 0,
        });
        let mb_id = seed_pending(&store, 5, true, 100); // enrolled first
        seed_pending(&store, 6, false, 100);            // enrolled second
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(7),
                exists: 0,
            },
            vec![],
        );
        let live: HashSet<u32> = [5, 6].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 2);
        // Creation order: 5 then 6, not 6 then 5.
        assert_eq!(session.set_seen_calls, vec![(5, true), (6, false)]);
    }

    /// RFC 4549: rapid flap (read→unread→read) collapses to the latest toggle
    /// through the UNIQUE(mailbox_id, uid) constraint, replaying once with the
    /// final state.
    #[test]
    fn outbox_rfc4549_rapid_flap_collapses_to_latest() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(2),
            exists: 1,
        });
        let mb_id = seed_pending(&store, 1, true, 100); // read
        seed_pending(&store, 1, false, 100); // unread (collapses)
        seed_pending(&store, 1, true, 100); // read again (final)
        let worker = SyncWorker::new(store);

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(2),
                exists: 1,
            },
            vec![],
        );
        let live: HashSet<u32> = [1].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_outbox(&mut session, mb_id, 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 1, "flap must collapse to one stored op");
        // Latest-wins: seen=true is the final toggle.
        assert_eq!(session.set_seen_calls, vec![(1, true)]);
    }

    /// Edge: a message with no prior flags (`"[]"`) converges to the target
    /// Seen state through a full sync round-trip including replay.
    #[test]
    fn outbox_rfc4549_empty_prior_flags_roundtrip() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 1,
        });
        let worker = SyncWorker::new(store.clone());
        let mb_id = seed_pending(&store, 1, true, 100); // optimistic read + op
        // Server reports flags = "[]" (truly unread).
        let mut hdr = mkhdr(1);
        hdr.flags = "[]".to_string();

        let session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(4),
                exists: 1,
            },
            vec![hdr],
        );
        let result = async_std::task::block_on(async {
            worker
                .sync_with_session(Box::new(session), "INBOX", cb())
                .await
        })
        .unwrap();

        assert_eq!(result.updated, 1, "message should be updated, not new");
        // Pending-wins: server said "[]" but local \Seen survives the sweep.
        let guard = store.lock().unwrap();
        let flags = queries::message_flags(guard.conn(), mb_id, 1)
            .unwrap()
            .unwrap();
        assert!(
            flags.contains("\\Seen"),
            "empty prior flags must converge to target state, got: {flags}"
        );
        // And the op was acked (deleted from queue).
        assert!(
            queries::pending_uids(guard.conn(), mb_id).unwrap().is_empty(),
            "op must be acked after successful replay"
        );
    }

    /// Canonical `\Seen` encoding round-trip: the store arg and the JSON
    /// flags column both use the exact backslash form.
    #[test]
    fn outbox_rfc4549_canonical_seen_encoding() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(2),
            exists: 1,
        });
        let mb_id = queries::ensure_mailbox(store.lock().unwrap().conn(), "INBOX").unwrap();
        {
            let guard = store.lock().unwrap();
            queries::upsert_message(
                guard.conn(), mb_id, 1, None, "Subj",
                "a@x.com", "[]", "[]", "2024-01-01T00:00:00Z",
                "[]", false, "p",
            )
            .unwrap();
        }
        // Apply Seen via the optimistic local write path.
        {
            let guard = store.lock().unwrap();
            queries::set_local_seen(guard.conn(), mb_id, 1, true).unwrap();
        }
        let guard = store.lock().unwrap();
        let flags: Vec<String> =
            serde_json::from_str(&queries::message_flags(guard.conn(), mb_id, 1).unwrap().unwrap())
                .unwrap();
        assert_eq!(flags, vec!["\\Seen".to_string()]);

        // The STORE arg must be the same canonical token.
        assert_eq!(
            crate::imap::seen_store_arg(true),
            "+FLAGS.SILENT (\\Seen)"
        );
        assert_eq!(queries::SEEN_FLAG, "\\Seen");
    }

    // ── Phase 7 Plan 07-02: per-folder sync gate tests ──
    // `sync_command_mailbox` is the plan's verify command; these tests are
    // the command-layer contract at worker/store level (Tauri `State`
    // cannot be constructed in unit tests, so the gate lives here).

    /// Two folders sync independently; a UIDVALIDITY bump in one wipes only
    /// that folder (FOLD-03 criterion 4).
    #[test]
    fn sync_command_mailbox_per_folder_isolation() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: 100,
            uid_next: Some(4),
            exists: 2,
        });
        let worker = SyncWorker::new(store.clone());

        let folder_a = |uidv: u32| MailboxSummary {
            selected_mailbox: "FolderA".to_string(),
            uid_validity: uidv,
            uid_next: Some(4),
            exists: 2,
        };
        let folder_b = MailboxSummary {
            selected_mailbox: "FolderB".to_string(),
            uid_validity: 200,
            uid_next: Some(3),
            exists: 1,
        };

        // Sync A (uidv 100) then B (uidv 200).
        for (summary, box_name, uids) in [
            (folder_a(100), "FolderA", vec![mkhdr(1), mkhdr(2)]),
            (folder_b, "FolderB", vec![mkhdr(9)]),
        ] {
            let session = mock(summary, uids);
            async_std::task::block_on(async {
                worker
                    .sync_with_session(Box::new(session), box_name, cb())
                    .await
            })
            .unwrap();
        }

        // Bump A's epoch → wipe + resync touches only A.
        let session_a2 = mock(folder_a(999), vec![mkhdr(1), mkhdr(2)]);
        let result = async_std::task::block_on(async {
            worker
                .sync_with_session(Box::new(session_a2), "FolderA", cb())
                .await
        })
        .unwrap();
        assert!(result.uid_validity_bump);

        // B's cache and sync state are untouched.
        let guard = worker.store.lock().unwrap();
        let b_count: i64 = guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages m JOIN mailboxes mb ON m.mailbox_id = mb.id WHERE mb.name = 'FolderB'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b_count, 1, "FolderB cache must survive FolderA's epoch bump");
        let b_uidv: u32 = guard
            .conn()
            .query_row(
                "SELECT uid_validity FROM mailboxes WHERE name = 'FolderB'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b_uidv, 200, "FolderB epoch must be untouched");
        let a_uidv: u32 = guard
            .conn()
            .query_row(
                "SELECT uid_validity FROM mailboxes WHERE name = 'FolderA'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_uidv, 999, "FolderA epoch must advance to 999");
    }

    /// STATUS UNSEEN datum is cached per folder and the badge query returns
    /// it alongside the dynamic local count (FOLD-02).
    #[test]
    fn sync_command_mailbox_unseen_cache() {
        let store = inbox(MailboxSummary {
            selected_mailbox: "Sent".to_string(),
            uid_validity: 300,
            uid_next: Some(3),
            exists: 2,
        });
        let worker = SyncWorker::new(store.clone());

        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "Sent".to_string(),
                uid_validity: 300,
                uid_next: Some(3),
                exists: 2,
            },
            vec![mkhdr(1), mkhdr(2)],
        );
        session.status_unseen = 2; // server says both unseen
        async_std::task::block_on(async {
            worker
                .sync_with_session(Box::new(session), "Sent", cb())
                .await
        })
        .unwrap();

        let guard = worker.store.lock().unwrap();
        let rows = queries::list_mailboxes(guard.conn()).unwrap();
        let sent = rows.iter().find(|r| r.name == "Sent").expect("Sent row cached");
        assert_eq!(sent.unseen_count, 2, "STATUS UNSEEN must be cached");
        assert_eq!(sent.unread_count, 2, "local count agrees pre-toggle");
        assert!(sent.last_sync_at.is_some());
    }

    /// `set_mailbox_status` upserts STATUS data without disturbing messages.
    #[test]
    fn set_mailbox_status_upsert_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        queries::set_mailbox_status(conn, "Drafts", 42, 7, 3).unwrap();
        queries::set_mailbox_status(conn, "Drafts", 42, 9, 1).unwrap();
        let rows = queries::list_mailboxes(conn).unwrap();
        let drafts = rows.iter().find(|r| r.name == "Drafts").expect("Drafts row");
        assert_eq!(drafts.uid_validity, 42);
        assert_eq!(drafts.uid_next, 9);
        assert_eq!(drafts.unseen_count, 1);
        assert_eq!(drafts.unread_count, 0, "no messages → local count 0");
    }

    // ── Phase 9 Plan 09-01: UID gap + convergence gate tests ──

    fn gap_summary(uidv: u32, exists: u32) -> MailboxSummary {
        MailboxSummary {
            selected_mailbox: "INBOX".to_string(),
            uid_validity: uidv,
            uid_next: Some(exists + 1),
            exists,
        }
    }

    /// A UID dropped from one batch FETCH is re-requested in the same pass
    /// (range-diff) — no silent gap waiting for the next poll.
    #[test]
    fn uid_gap() {
        let store = inbox(gap_summary(100, 3));
        let worker = SyncWorker::new(store.clone());

        let session = mock(gap_summary(100, 3), vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        session.gap_once.lock().unwrap().push(2); // transient drop
        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 3, "dropped uid must be recovered in-pass");
        assert_eq!(result.gap_refetches, 1);
        assert!(!result.converged);
        let guard = worker.store.lock().unwrap();
        let cached = queries::existing_uids(guard.conn(), 1, &[1, 2, 3]).unwrap();
        assert_eq!(cached.len(), 3);
    }

    /// Double-poll-zero-FETCH convergence: two consecutive unchanged polls
    /// issue no message FETCHes, and an arrival between syncs still lands.
    #[test]
    fn convergence_test() {
        let store = inbox(gap_summary(100, 2));
        let worker = SyncWorker::new(store.clone());

        // Pass 1 — full sweep.
        let s1 = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
        let r1 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s1), "INBOX", cb()).await
        })
        .unwrap();
        assert!(!r1.converged);
        assert!(r1.fetched > 0);

        // Pass 2 — nothing changed → converged, zero FETCHes.
        let s2 = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
        let r2 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s2), "INBOX", cb()).await
        })
        .unwrap();
        assert!(r2.converged, "unchanged second poll must converge");
        assert_eq!(r2.fetched, 0, "converged pass issues no FETCHes");
        assert_eq!(r2.new, 0);
        assert_eq!(r2.deleted, 0);

        // Pass 3 — uid 3 arrived between syncs → fetched despite convergence.
        let s3 = mock(gap_summary(100, 3), vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        let r3 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s3), "INBOX", cb()).await
        })
        .unwrap();
        assert!(!r3.converged);
        assert_eq!(r3.new, 1, "arrival between syncs must land");
    }

    /// A UID missing from every FETCH earns strikes, then stops being
    /// re-requested (no infinite backfill loop); expunge prunes the record.
    #[test]
    fn tombstoned_uids_stop_being_requested() {
        let store = inbox(gap_summary(100, 2));
        let worker = SyncWorker::new(store.clone());

        // Passes 1–3: uid 2 in SEARCH, never in FETCH → strikes 1, 2, 3.
        for pass in 1..=3 {
            let session = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
            session.always_miss.lock().unwrap().push(2);
            let result = async_std::task::block_on(async {
                worker.sync_with_session(Box::new(session), "INBOX", cb()).await
            })
            .unwrap();
            assert!(!result.converged, "changed sets must sweep (pass {pass})");
            let guard = worker.store.lock().unwrap();
            let mb = queries::mailbox_id(guard.conn(), "INBOX").unwrap().unwrap();
            let strikes: i64 = guard
                .conn()
                .query_row(
                    "SELECT strikes FROM fetch_tombstones WHERE mailbox_id = ?1 AND uid = 2",
                    rusqlite::params![mb],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(strikes, pass, "strike {pass} recorded");
        }

        // Pass 4: uid 2 is tombstoned → sweep requests only uid 1.
        let session4 = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
        let r4 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session4), "INBOX", cb()).await
        })
        .unwrap();
        assert!(!r4.converged);
        {
            let guard = worker.store.lock().unwrap();
            let cached = queries::existing_uids(guard.conn(), 1, &[1, 2]).unwrap();
            assert_eq!(cached, vec![1], "flaky uid stays uncached, good uid intact");
        }

        // Pass 5: uid 2 expunged server-side → tombstone pruned, no residue.
        let session5 = mock(gap_summary(100, 1), vec![mkhdr(1)]);
        async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session5), "INBOX", cb()).await
        })
        .unwrap();
        let guard = worker.store.lock().unwrap();
        let mb = queries::mailbox_id(guard.conn(), "INBOX").unwrap().unwrap();
        let remaining = queries::tombstoned_uids(guard.conn(), mb).unwrap();
        assert!(remaining.is_empty(), "expunged uid's tombstone must prune");
    }

    /// A set cancel flag aborts the sweep before any fetch: no messages
    /// cached, no sync state stamped.
    #[test]
    fn cancel_sync_aborts_without_state_write() {
        use std::sync::atomic::AtomicBool;
        let store = inbox(gap_summary(100, 3));
        let flag = Arc::new(AtomicBool::new(true)); // cancelled before start
        let worker = SyncWorker::with_cancel(store.clone(), flag);

        let session = mock(gap_summary(100, 3), vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 0, "aborted pass caches nothing");
        assert!(!result.converged);
        let guard = worker.store.lock().unwrap();
        // STATUS caching (step 2b) legitimately records the epoch, but the
        // message sync must stamp nothing and cache nothing.
        let last: Option<String> = guard
            .conn()
            .query_row(
                "SELECT last_sync_at FROM mailboxes WHERE name = 'INBOX'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last, None, "aborted pass must not stamp last_sync_at");
        let count: i64 = guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages m JOIN mailboxes mb ON m.mailbox_id = mb.id WHERE mb.name = 'INBOX'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "aborted pass caches no messages");
    }

    // ── Phase 10 Plan 10-03 Wave 2: imap_outbox pre-sweep replay ──

    /// Seed helper: one INBOX row + hidden flag + queued delete/move op,
    /// mirroring what the delete/move commands write. Returns mailbox id.
    fn seed_imap_op(
        store: &Arc<std::sync::Mutex<Store>>,
        uid: u32,
        op: &str,
        dest: Option<&str>,
        epoch: u32,
    ) -> u64 {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mb_id = queries::ensure_mailbox(conn, "INBOX").unwrap();
        queries::upsert_message(
            conn,
            mb_id,
            uid,
            Some(&format!("<msg{uid}@example.com>")),
            &format!("Subject {uid}"),
            "alice@example.com",
            "[]",
            "[]",
            "2024-10-03T12:00:00Z",
            "[]",
            false,
            &format!("Subject {uid}"),
        )
        .unwrap();
        queries::set_pending_delete(conn, mb_id, uid, true).unwrap();
        queries::enqueue_imap_outbox(conn, mb_id, uid, op, dest, epoch).unwrap();
        mb_id
    }

    /// RFC 4549 parity: epoch bump drops the whole imap queue with zero
    /// verb calls, and optimistically hidden rows become visible again
    /// (their op is dead — the sweep reconciles them as ordinary mail).
    #[test]
    fn imap_replay_rfc4549_epoch_bump_drops_queue_and_unhides() {
        let store = inbox(gap_summary(200, 1));
        let mb_id = seed_imap_op(&store, 5, queries::IMAP_OP_DELETE, Some("Trash"), 100);
        seed_imap_op(&store, 6, queries::IMAP_OP_MOVE, Some("Archive"), 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(gap_summary(200, 1), vec![]);
        let live: HashSet<u32> = [5, 6].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_imap_outbox(&mut session, mb_id, "INBOX", 200, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.dropped, 2);
        assert_eq!(summary.acked, 0);
        assert_eq!(summary.moved, 0);
        assert!(
            session.moved_calls.is_empty() && session.copied_calls.is_empty(),
            "stale UIDs must never reach the wire"
        );
        let guard = worker.store.lock().unwrap();
        assert!(queries::pending_imap_uids(guard.conn(), mb_id).unwrap().is_empty());
        assert!(!queries::is_pending_delete(guard.conn(), mb_id, 5).unwrap());
    }

    /// RFC 4549 parity: an absent UID drops its single op; the live op
    /// still replays as a server-side move to the STORED dest.
    #[test]
    fn imap_replay_rfc4549_absent_uid_drops_single_op() {
        let store = inbox(gap_summary(100, 1));
        let mb_id = seed_imap_op(&store, 5, queries::IMAP_OP_DELETE, Some("Trash"), 100);
        seed_imap_op(&store, 6, queries::IMAP_OP_MOVE, Some("Archive"), 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(gap_summary(100, 1), vec![mkhdr(5)]);
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_imap_outbox(&mut session, mb_id, "INBOX", 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 1);
        assert_eq!(summary.dropped, 1);
        assert_eq!(summary.moved, 1);
        assert_eq!(
            session.moved_calls,
            vec![("5".to_string(), "Trash".to_string())],
            "delete replays as move-to-Trash with the stored dest"
        );
        let guard = worker.store.lock().unwrap();
        assert!(
            queries::pending_imap_uids(guard.conn(), mb_id).unwrap().is_empty(),
            "acked + absent-dropped ops must leave the queue empty"
        );
    }

    /// RFC 4549 parity: a failed move stays queued with attempts recorded,
    /// and the row stays hidden.
    #[test]
    fn imap_replay_rfc4549_failure_stays_queued() {
        let store = inbox(gap_summary(100, 1));
        let mb_id = seed_imap_op(&store, 5, queries::IMAP_OP_MOVE, Some("Archive"), 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(gap_summary(100, 1), vec![mkhdr(5)]);
        session.fail_move = true;
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_imap_outbox(&mut session, mb_id, "INBOX", 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.acked, 0);
        let guard = worker.store.lock().unwrap();
        let ops = queries::list_imap_outbox(guard.conn(), mb_id).unwrap();
        assert_eq!(ops.len(), 1, "failed op must stay queued");
        assert_eq!(ops[0].attempts, 1);
        assert!(ops[0].last_error.is_some());
        assert!(queries::is_pending_delete(guard.conn(), mb_id, 5).unwrap());
    }

    /// Wave-B residual: a loud refusal after the COPY leg drops the op with
    /// a log (never retries — retrying would re-COPY duplicates only to
    /// refuse again) and un-hides the untouched src row.
    #[test]
    fn imap_replay_refusal_drops_with_log() {
        let store = inbox(gap_summary(100, 1));
        let mb_id = seed_imap_op(&store, 5, queries::IMAP_OP_MOVE, Some("Archive"), 100);
        let worker = SyncWorker::new(store);

        let mut session = mock(gap_summary(100, 1), vec![mkhdr(5)]);
        session.fail_move_refused = true;
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_imap_outbox(&mut session, mb_id, "INBOX", 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.dropped, 1);
        assert_eq!(summary.failed, 0, "refusal must not linger as retryable");
        assert_eq!(session.moved_calls.len(), 1, "wire attempt happened");
        let guard = worker.store.lock().unwrap();
        assert!(queries::pending_imap_uids(guard.conn(), mb_id).unwrap().is_empty());
        assert!(!queries::is_pending_delete(guard.conn(), mb_id, 5).unwrap());
    }

    /// Move-carries-toggle: a move enqueued over a pending Seen toggle
    /// applies the captured intent at the destination UID (resolved by
    /// Message-ID), then re-SELECTs the source mailbox for the sweep.
    #[test]
    fn imap_replay_move_applies_seen_intent_at_dest() {
        let store = inbox(gap_summary(100, 1));
        let mb_id = {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let mb_id = queries::ensure_mailbox(conn, "INBOX").unwrap();
            queries::upsert_message(
                conn, mb_id, 5, Some("<msg5@example.com>"), "Subject 5",
                "alice@example.com", "[]", "[]", "2024-10-03T12:00:00Z",
                "[]", false, "Subject 5",
            )
            .unwrap();
            // Pending Seen toggle first — the move consumes it as intent.
            queries::set_local_seen(conn, mb_id, 5, true).unwrap();
            queries::enqueue_outbox(conn, mb_id, 5, true, 100).unwrap();
            queries::set_pending_delete(conn, mb_id, 5, true).unwrap();
            queries::enqueue_imap_outbox(conn, mb_id, 5, queries::IMAP_OP_MOVE, Some("Archive"), 100)
                .unwrap();
            mb_id
        };
        let worker = SyncWorker::new(store);

        let mut session = mock(gap_summary(100, 1), vec![mkhdr(5)]);
        let live: HashSet<u32> = [5].into_iter().collect();
        let summary = async_std::task::block_on(async {
            worker
                .replay_imap_outbox(&mut session, mb_id, "INBOX", 100, Some(&live))
                .await
        })
        .unwrap();

        assert_eq!(summary.acked, 1);
        assert_eq!(
            session.set_seen_calls,
            vec![(5, true)],
            "captured Seen intent must reach the dest UID"
        );
        assert_eq!(
            session.select_calls,
            vec!["Archive".to_string(), "INBOX".to_string()],
            "dest resolution must bracket with a source re-SELECT"
        );
    }

    /// Pre-sweep order: an offline delete (queued, failing replay) survives
    /// a full sync pass still hidden — the sweep never resurrects it.
    #[test]
    fn imap_presweep_order_no_same_pass_resurrection() {
        let store = inbox(gap_summary(100, 1));
        let worker = SyncWorker::new(store.clone());
        seed_imap_op(&store, 5, queries::IMAP_OP_DELETE, Some("Trash"), 100);

        // Server still lists the UID (move not yet replayed); replay fails.
        let mut session = mock(gap_summary(100, 1), vec![mkhdr(5)]);
        session.fail_move = true;
        async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        let guard = store.lock().unwrap();
        let rows = queries::list_messages(guard.conn(), "INBOX", 100, 0).unwrap();
        assert!(
            rows.iter().all(|r| r.uid != 5),
            "locally-deleted row must stay hidden after the pass"
        );
        assert_eq!(
            queries::pending_imap_uids(
                guard.conn(),
                queries::mailbox_id(guard.conn(), "INBOX").unwrap().unwrap()
            )
            .unwrap(),
            vec![5],
            "failed op stays queued for the next pass"
        );
    }

    /// Convergence gate: a queued imap op blocks the converged shortcut —
    /// a pending delete never reads as "unchanged".
    #[test]
    fn imap_pending_blocks_convergence() {
        let store = inbox(gap_summary(100, 2));
        let worker = SyncWorker::new(store.clone());

        // Pass 1 — full sweep, converges the UID set.
        let s1 = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
        async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s1), "INBOX", cb()).await
        })
        .unwrap();

        // Offline delete of uid 2 (replay will fail — stays queued).
        {
            let guard = store.lock().unwrap();
            let mb_id = queries::mailbox_id(guard.conn(), "INBOX").unwrap().unwrap();
            queries::set_pending_delete(guard.conn(), mb_id, 2, true).unwrap();
            queries::enqueue_imap_outbox(
                guard.conn(), mb_id, 2, queries::IMAP_OP_DELETE, Some("Trash"), 100,
            )
            .unwrap();
        }
        let mut s2 = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
        s2.fail_move = true;
        let r2 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s2), "INBOX", cb()).await
        })
        .unwrap();
        assert!(!r2.converged, "queued delete must block convergence");
        assert!(r2.fetched > 0, "sweep must run while ops are queued");
    }

    /// Acked pre-sweep moves feed `SyncSummary.moved` (both delete and move
    /// move server-side).
    #[test]
    fn imap_replay_success_counts_moved_in_summary() {
        let store = inbox(gap_summary(100, 2));
        let worker = SyncWorker::new(store.clone());
        seed_imap_op(&store, 1, queries::IMAP_OP_DELETE, Some("Trash"), 100);
        seed_imap_op(&store, 2, queries::IMAP_OP_MOVE, Some("Archive"), 100);

        let session = mock(gap_summary(100, 2), vec![mkhdr(1), mkhdr(2)]);
        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session), "INBOX", cb()).await
        })
        .unwrap();

        assert_eq!(result.moved, 2);
        assert_eq!(result.deleted, 0, "moves are not server-side disappearances");
        let guard = store.lock().unwrap();
        let mb_id = queries::mailbox_id(guard.conn(), "INBOX").unwrap().unwrap();
        assert!(queries::pending_imap_uids(guard.conn(), mb_id).unwrap().is_empty());
    }

    /// Expunge cleanup: a server-side disappearance removes the row, its FTS
    /// ghost, bodies/parts, AND the on-disk attachment dir (best-effort).
    #[test]
    fn expunge_cleans_row_fts_bodies_and_attachment_dir() {
        let store = inbox(gap_summary(100, 1));
        let worker = SyncWorker::new(store.clone());

        // Pass 1 — cache uid 7 with body + attachment metadata.
        let s1 = mock(gap_summary(100, 1), vec![mkhdr(7)]);
        async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s1), "INBOX", cb()).await
        })
        .unwrap();
        let mb_id = {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let mb_id = queries::mailbox_id(conn, "INBOX").unwrap().unwrap();
            let msg_id = queries::find_message_id(conn, mb_id, 7).unwrap().unwrap();
            queries::insert_body(conn, msg_id, Some("t"), None).unwrap();
            queries::insert_attachment_meta(conn, msg_id, "2", "f.pdf", "application/pdf", 5)
                .unwrap();
            mb_id
        };
        // Fake the on-disk dir the reader cache would own.
        let root = std::env::temp_dir().join(format!("sge_att_test_{}", std::process::id()));
        let dir = root.join("attachments").join("100").join("7");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.pdf"), b"bytes").unwrap();
        let worker = SyncWorker::with_attachment_root(
            SyncWorker::new(store.clone()),
            root.clone(),
        );
        let _ = mb_id;

        // Pass 2 — server expunged uid 7 (empty mailbox, EXISTS=0).
        let s2 = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(8),
                exists: 0,
            },
            vec![],
        );
        let r2 = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(s2), "INBOX", cb()).await
        })
        .unwrap();
        assert_eq!(r2.deleted, 1);

        let guard = store.lock().unwrap();
        assert!(queries::get_message_by_uid(guard.conn(), "INBOX", 7).unwrap().is_none());
        assert!(queries::fts_search(guard.conn(), Some("INBOX"), "Subject 7").unwrap().is_empty());
        let bodies: i64 = guard
            .conn()
            .query_row("SELECT COUNT(*) FROM message_bodies", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bodies, 0);
        let parts: i64 = guard
            .conn()
            .query_row("SELECT COUNT(*) FROM attachment_parts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(parts, 0);
        assert!(!dir.exists(), "attachment dir must be removed best-effort");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── send-queue flush tests (Plan 13-02) ──────────────────────────

    use crate::send_queue::EnqueueInput;
    use crate::smtp::SmtpOutcome;

    /// One recorded transport call: envelope + the exact bytes handed over.
    pub struct FakeCall {
        pub from: String,
        pub to: Vec<String>,
        pub bytes: Vec<u8>,
    }

    /// Record-and-replay SMTP transport (mirrors the MockSession pattern):
    /// every `send_raw` records its args and answers from the script in
    /// order; past the script end every send succeeds.
    pub struct FakeTransport {
        pub calls: Mutex<Vec<FakeCall>>,
        pub script: Vec<SmtpOutcome>,
        pub next: AtomicUsize,
    }

    impl FakeTransport {
        fn scripted(script: Vec<SmtpOutcome>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script,
                next: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl crate::smtp::SmtpTransport for FakeTransport {
        fn send_raw(
            &self,
            _account: &crate::smtp::SmtpAccount,
            envelope_from: &str,
            envelope_to: &[String],
            bytes: &[u8],
        ) -> SmtpOutcome {
            self.calls.lock().unwrap().push(FakeCall {
                from: envelope_from.to_string(),
                to: envelope_to.to_vec(),
                bytes: bytes.to_vec(),
            });
            let i = self.next.fetch_add(1, Ordering::SeqCst);
            self.script.get(i).cloned().unwrap_or(SmtpOutcome::Sent)
        }
    }

    static FLUSH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn flush_dir(name: &str) -> PathBuf {
        let n = FLUSH_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sge-flush-{}-{name}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_account() -> crate::smtp::SmtpAccount {
        crate::smtp::SmtpAccount {
            host: "smtp.utfpr.edu.br".to_string(),
            port: 587,
            username: "alice".to_string(),
            password: "pw".to_string(),
        }
    }

    fn flush_env(manager: Option<Arc<crate::imap::manager::SessionManager>>, sent_wire: Option<String>, script: Vec<SmtpOutcome>) -> (SendFlushEnv, Arc<FakeTransport>) {
        let tx = Arc::new(FakeTransport::scripted(script));
        let transport: Arc<dyn crate::smtp::SmtpTransport> = tx.clone();
        let env = SendFlushEnv {
            gate: SendGate::default(),
            transport,
            account: test_account(),
            manager,
            sent_wire,
        };
        (env, tx)
    }

    /// Drive the async flush synchronously (no manager lease in these
    /// tests, so the filing leg is skipped — same `block_on` pattern the
    /// production `spawn_blocking` path uses).
    fn flush_blocking(worker: &SyncWorker, env: &SendFlushEnv) -> FlushSummary {
        async_std::task::block_on(async { worker.flush_send_queue(env).await })
    }

    /// Enqueue one mail to `to` (+cc/+bcc) and return the stored row.
    fn queued_mail(
        store: &Arc<std::sync::Mutex<Store>>,
        app_data: &std::path::Path,
        to: &str,
    ) -> queries::SendRow {
        queued_mail_cc(store, app_data, to, &[], &[])
    }

    fn queued_mail_cc(
        store: &Arc<std::sync::Mutex<Store>>,
        app_data: &std::path::Path,
        to: &str,
        cc: &[&str],
        bcc: &[&str],
    ) -> queries::SendRow {
        let input = EnqueueInput {
            from: "eu@utfpr.edu.br".to_string(),
            to: vec![to.to_string()],
            cc: cc.iter().map(|s| s.to_string()).collect(),
            bcc: bcc.iter().map(|s| s.to_string()).collect(),
            subject: "Oi".to_string(),
            body: "corpo".to_string(),
            draft_id: None,
        };
        let guard = store.lock().unwrap();
        let out = crate::send_queue::enqueue_send(guard.conn(), app_data, input).unwrap();
        queries::get_send_row(guard.conn(), &out.queue_id)
            .unwrap()
            .unwrap()
    }

    fn send_state_of(store: &Arc<std::sync::Mutex<Store>>, id: &str) -> String {
        let guard = store.lock().unwrap();
        queries::get_send_row(guard.conn(), id)
            .unwrap()
            .unwrap()
            .state
    }

    /// Test 1: a due row with transport Sent marks the row sent (SMTP-success
    /// leg only — Sent filing itself is Plan 13-03).
    #[test]
    fn flush_sent_marks_row_sent_with_identical_bytes() {
        let store = inbox(drafts_summary());
        let app_data = flush_dir("sent");
        let row = queued_mail_cc(
            &store,
            &app_data,
            "amigo@example.com",
            &["copia@example.com"],
            &["oculta@example.com"],
        );
        let (env, tx) = flush_env(None, None, vec![]);
        let worker = SyncWorker::new(store.clone());
        let summary = flush_blocking(&worker, &env);
        assert_eq!(
            (summary.sent, summary.deferred, summary.failed, summary.uncertain),
            (1, 0, 0, 0)
        );
        assert!(!summary.skipped);
        assert_eq!(send_state_of(&store, &row.id), SEND_STATE_SENT);
        assert_eq!(tx.call_count(), 1);
        let calls = tx.calls.lock().unwrap();
        let call = &calls[0];
        assert_eq!(call.from, "eu@utfpr.edu.br");
        // Envelope carries To + Cc + BCC (envelope-only BCC).
        for want in ["amigo@example.com", "copia@example.com", "oculta@example.com"] {
            assert!(
                call.to.contains(&want.to_string()),
                "envelope missing {want}: {:?}",
                call.to
            );
        }
        // Identical immutable bytes: what SMTP got == what enqueue wrote.
        let file_bytes = std::fs::read(&row.eml_path).unwrap();
        assert_eq!(call.bytes, file_bytes);
        drop(calls);
        // Second flush: terminal rows never re-send.
        let again = flush_blocking(&worker, &env);
        assert_eq!(tx.call_count(), 1, "sent rows must never re-send");
        assert_eq!(
            (again.sent, again.deferred, again.failed, again.uncertain),
            (0, 0, 0, 0)
        );
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Test 2: transport Transient keeps the row queued with attempts+1 and
    /// a backoff schedule; terminal failed rows are skipped by the pass and
    /// only manual retry requeues them; the attempts cap parks as failed.
    #[test]
    fn flush_transient_backs_off_failed_terminal_and_skips_failed() {
        let store = inbox(drafts_summary());
        let app_data = flush_dir("transient");
        let row = queued_mail(&store, &app_data, "amigo@example.com");
        // A terminal failed row is invisible to the pass.
        let failed = queued_mail(&store, &app_data, "falho@example.com");
        {
            let guard = store.lock().unwrap();
            crate::send_queue::mark_send_failed(guard.conn(), &failed.id, "refused").unwrap();
        }
        let (env, tx) = flush_env(None, None, vec![SmtpOutcome::Transient {
            message: "try later".to_string(),
        }]);
        let worker = SyncWorker::new(store.clone());
        let before = chrono::Utc::now();
        let summary = flush_blocking(&worker, &env);
        let after = chrono::Utc::now();
        assert_eq!(
            (summary.sent, summary.deferred, summary.failed, summary.uncertain),
            (0, 1, 0, 0)
        );
        assert_eq!(
            tx.call_count(),
            1,
            "only the due row sends; failed rows never auto-retry"
        );
        {
            let guard = store.lock().unwrap();
            let updated = queries::get_send_row(guard.conn(), &row.id)
                .unwrap()
                .unwrap();
            assert_eq!(updated.attempts, 1);
            // attempts=1 → nominal 60 s ±20 % → [before+48s, after+72s].
            let scheduled = updated.next_retry_at.clone().unwrap();
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
            let failed_row = queries::get_send_row(guard.conn(), &failed.id)
                .unwrap()
                .unwrap();
            assert_eq!(
                failed_row.state,
                crate::store::queries::SEND_STATE_FAILED
            );
            assert_eq!(failed_row.attempts, 1, "failed row untouched by the pass");
        }
        // Still scheduled in the future: an immediate second flush finds
        // nothing due.
        let again = flush_blocking(&worker, &env);
        assert_eq!(tx.call_count(), 1, "backoff schedule must hold the row");
        assert_eq!(
            (again.sent, again.deferred, again.failed, again.uncertain),
            (0, 0, 0, 0)
        );
        // Attempts cap: the 10th consecutive transient parks as failed.
        let capped = queued_mail(&store, &app_data, "teimoso@example.com");
        {
            let guard = store.lock().unwrap();
            guard
                .conn()
                .execute(
                    "UPDATE send_queue SET attempts = 9, next_retry_at = NULL WHERE id = ?1",
                    rusqlite::params![capped.id],
                )
                .unwrap();
        }
        let (env2, tx2) = flush_env(None, None, vec![SmtpOutcome::Transient {
            message: "still down".to_string(),
        }]);
        let capped_summary = flush_blocking(&worker, &env2);
        assert_eq!(tx2.call_count(), 1);
        assert_eq!(
            (
                capped_summary.sent,
                capped_summary.deferred,
                capped_summary.failed,
                capped_summary.uncertain
            ),
            (0, 0, 1, 0)
        );
        assert_eq!(
            send_state_of(&store, &capped.id),
            crate::store::queries::SEND_STATE_FAILED
        );
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Test 3: transport Uncertain triggers reconcile-not-resend — a Sent
    /// SEARCH hit marks sent with no second transport call; a miss requeues
    /// exactly once for a single re-send; no Sent folder skips with zero IMAP.
    #[test]
    fn flush_uncertain_reconciles_before_any_resend() {
        let store = inbox(drafts_summary());
        let app_data = flush_dir("uncertain");
        let row = queued_mail(&store, &app_data, "amigo@example.com");
        let (env, tx) = flush_env(None, None, vec![SmtpOutcome::Uncertain {
            reason: "DATA timeout".to_string(),
        }]);
        let worker = SyncWorker::new(store.clone());
        let summary = flush_blocking(&worker, &env);
        assert_eq!(
            (summary.sent, summary.deferred, summary.failed, summary.uncertain),
            (0, 0, 0, 1)
        );
        assert_eq!(
            send_state_of(&store, &row.id),
            crate::store::queries::SEND_STATE_UNCERTAIN
        );
        assert_eq!(tx.call_count(), 1);

        // Sent SEARCH hit → confirmed sent with NO second transport call.
        let mut session = mock(drafts_summary(), vec![]);
        session
            .search_header_by_msgid
            .insert(row.message_id.clone(), vec![42]);
        let r = async_std::task::block_on(async {
            worker
                .reconcile_uncertain_sends(&mut session, Some("Sent"), "INBOX")
                .await
        })
        .unwrap();
        assert_eq!((r.confirmed_sent, r.requeued, r.still_uncertain), (1, 0, 0));
        assert!(!r.skipped_no_sent);
        assert_eq!(send_state_of(&store, &row.id), SEND_STATE_SENT);
        assert_eq!(tx.call_count(), 1, "reconcile must never re-send on a hit");
        assert_eq!(
            session.select_calls,
            vec!["Sent".to_string(), "INBOX".to_string()],
            "reconcile SELECTs Sent then restores the pass mailbox"
        );

        // Miss path: a second uncertain row requeues exactly once, attempts
        // counted with a backoff schedule.
        let row2 = queued_mail(&store, &app_data, "outro@example.com");
        {
            let guard = store.lock().unwrap();
            queries::mark_send_uncertain(guard.conn(), &row2.id, "drop after send").unwrap();
        }
        let mut session2 = mock(drafts_summary(), vec![]);
        let r2 = async_std::task::block_on(async {
            worker
                .reconcile_uncertain_sends(&mut session2, Some("Sent"), "INBOX")
                .await
        })
        .unwrap();
        assert_eq!((r2.confirmed_sent, r2.requeued, r2.still_uncertain), (0, 1, 0));
        {
            let guard = store.lock().unwrap();
            let back = queries::get_send_row(guard.conn(), &row2.id)
                .unwrap()
                .unwrap();
            assert_eq!(back.state, SEND_STATE_QUEUED);
            assert_eq!(back.attempts, 1, "reconcile miss counts the attempt");
            assert!(
                back.next_retry_at.is_some(),
                "requeue carries a backoff schedule"
            );
        }

        // No Sent folder → skip with zero IMAP verbs, rows untouched.
        let row3 = queued_mail(&store, &app_data, "terceiro@example.com");
        {
            let guard = store.lock().unwrap();
            queries::mark_send_uncertain(guard.conn(), &row3.id, "drop after send").unwrap();
        }
        let mut session3 = mock(drafts_summary(), vec![]);
        let r3 = async_std::task::block_on(async {
            worker
                .reconcile_uncertain_sends(&mut session3, None, "INBOX")
                .await
        })
        .unwrap();
        assert!(r3.skipped_no_sent);
        assert!(
            session3.select_calls.is_empty(),
            "no Sent → zero IMAP verbs"
        );
        assert_eq!(
            send_state_of(&store, &row3.id),
            crate::store::queries::SEND_STATE_UNCERTAIN
        );
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Test 4: the flush holds the SendGate (a concurrent flush skips with
    /// zero sends) and delivers each row exactly once — and the SMTP leg
    /// takes no IMAP lease by construction (it receives no session).
    #[test]
    fn flush_serializes_on_send_gate_one_delivery_per_row() {
        let store = inbox(drafts_summary());
        let app_data = flush_dir("gate");
        let a = queued_mail(&store, &app_data, "a@example.com");
        let b = queued_mail(&store, &app_data, "b@example.com");
        let (env, tx) = flush_env(None, None, vec![]);
        let worker = SyncWorker::new(store.clone());
        // A pass already in flight: flush skips with zero sends.
        let _held = env.gate.try_begin().expect("hold the gate");
        let skipped = flush_blocking(&worker, &env);
        assert!(skipped.skipped);
        assert_eq!(tx.call_count(), 0);
        drop(_held);
        // Next pass delivers each row exactly once.
        let summary = flush_blocking(&worker, &env);
        assert_eq!(summary.sent, 2);
        assert_eq!(tx.call_count(), 2);
        assert_eq!(send_state_of(&store, &a.id), SEND_STATE_SENT);
        assert_eq!(send_state_of(&store, &b.id), SEND_STATE_SENT);
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Test 5: crash-recovery reset runs before the first flush in the launch
    /// sequence — and stranded `sending` rows triage to `uncertain`
    /// (reconcile-pending), never blindly re-sent.
    #[test]
    fn crash_reset_runs_before_first_flush() {
        let store = inbox(drafts_summary());
        let app_data = flush_dir("crash");
        // Stranded `sending` row: may have been SMTP-accepted pre-crash.
        let stranded = queued_mail(&store, &app_data, "quase@example.com");
        {
            let guard = store.lock().unwrap();
            queries::set_send_state(
                guard.conn(),
                &stranded.id,
                crate::store::queries::SEND_STATE_SENDING,
            )
            .unwrap();
        }
        let queued = queued_mail(&store, &app_data, "novo@example.com");
        let (env, tx) = flush_env(None, None, vec![]);
        let worker = SyncWorker::new(store.clone());
        let summary = flush_blocking(&worker, &env);
        // Only the queued row went over SMTP; the stranded row triaged to
        // uncertain (reconcile-pending), never blindly re-sent.
        assert_eq!(tx.call_count(), 1);
        assert_eq!(
            tx.calls.lock().unwrap()[0].to,
            vec!["novo@example.com".to_string()]
        );
        assert_eq!(send_state_of(&store, &queued.id), SEND_STATE_SENT);
        assert_eq!(
            send_state_of(&store, &stranded.id),
            crate::store::queries::SEND_STATE_UNCERTAIN
        );
        // The recovered row triages outside the per-row loop (no transport
        // call), so the pass summary counts only the SMTP verdict.
        assert_eq!((summary.sent, summary.uncertain), (1, 0));
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Step 4d wiring, empty-mailbox branch: draft replay (4c) and send
    /// flush (4d) run in one pass; the SMTP leg issues zero IMAP verbs.
    #[test]
    fn empty_pass_flushes_after_draft_replay() {
        let store = inbox(drafts_summary());
        let _ = draft_row(&store, "INBOX", "compose-1", "<c1@sge.local>");
        let app_data = flush_dir("emptypass");
        let row = queued_mail(&store, &app_data, "amigo@example.com");
        let (env, tx) = flush_env(None, None, vec![]);
        let worker = SyncWorker::new(store.clone()).with_send_flush(env);
        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(1),
                exists: 0,
            },
            vec![],
        );
        session.search_header_results = vec![5];
        async_std::task::block_on(async {
            worker
                .sync_with_borrowed(&mut session, "INBOX", cb())
                .await
        })
        .unwrap();
        assert_eq!(session.appended_calls.len(), 1, "draft replay (4c) ran");
        assert_eq!(
            send_state_of(&store, &row.id),
            SEND_STATE_SENT,
            "send flush (4d) ran"
        );
        assert_eq!(tx.call_count(), 1);
        assert!(session.logout_called.load(Ordering::SeqCst));
        assert_eq!(
            session.select_calls,
            vec!["INBOX".to_string()],
            "SMTP leg took no IMAP lease: only the pass SELECT, no Sent touch"
        );
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Step 4d wiring, normal branch: same ordering guarantees on a
    /// non-empty mailbox.
    #[test]
    fn normal_pass_flushes_after_draft_replay() {
        let store = inbox(drafts_summary());
        let _ = draft_row(&store, "INBOX", "compose-1", "<c1@sge.local>");
        let app_data = flush_dir("normalpass");
        let row = queued_mail(&store, &app_data, "amigo@example.com");
        let (env, tx) = flush_env(None, None, vec![]);
        let worker = SyncWorker::new(store.clone()).with_send_flush(env);
        let mut session = mock(
            MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(2),
                exists: 1,
            },
            vec![mkhdr(1)],
        );
        session.search_header_results = vec![5];
        async_std::task::block_on(async {
            worker
                .sync_with_borrowed(&mut session, "INBOX", cb())
                .await
        })
        .unwrap();
        assert_eq!(session.appended_calls.len(), 1, "draft replay (4c) ran");
        assert_eq!(
            send_state_of(&store, &row.id),
            SEND_STATE_SENT,
            "send flush (4d) ran"
        );
        assert_eq!(tx.call_count(), 1);
        assert_eq!(
            session.select_calls,
            vec!["INBOX".to_string()],
            "SMTP leg took no IMAP lease: only the pass SELECT, no Sent touch"
        );
        let _ = std::fs::remove_dir_all(&app_data);
    }

    /// Sent-wire resolution follows the Phase 11 role table; unknown folders
    /// resolve to no-Sent (the reconcile pass waits for Plan 13-03).
    #[test]
    fn resolve_sent_wire_picks_role_sent() {
        fn mb(name: &str, attrs: &[&str]) -> MailboxInfo {
            MailboxInfo {
                name: name.to_string(),
                display_name: name.to_string(),
                delimiter: "/".to_string(),
                attributes: attrs.iter().map(|s| s.to_string()).collect(),
            }
        }
        let folders = vec![
            mb("INBOX", &[]),
            mb("[Gmail]/Sent Mail", &[]),
            mb("Trash", &[]),
        ];
        assert_eq!(
            resolve_sent_wire(&folders).as_deref(),
            Some("[Gmail]/Sent Mail")
        );
        let folders = vec![mb("INBOX", &[]), mb("Archive", &["\\Sent"])];
        assert_eq!(resolve_sent_wire(&folders).as_deref(), Some("Archive"));
        let folders = vec![mb("INBOX", &[])];
        assert_eq!(resolve_sent_wire(&folders), None);
    }}
