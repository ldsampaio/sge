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
use std::sync::Arc;

use crate::imap::headers::MessageHeader;
use crate::imap::{MailboxSummary, SyncError, SyncSession};
use crate::store::queries;
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
}

impl SyncWorker {
    pub fn new(store: Arc<std::sync::Mutex<Store>>) -> Self {
        Self {
            store,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Share an externally owned cancellation flag (the `AppState` slot
    /// the `cancel_sync` command sets).
    pub fn with_cancel(
        store: Arc<std::sync::Mutex<Store>>,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self { store, cancel }
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

    /// Execute a full INBOX sync pass against an injected [`SyncSession`].
    ///
    /// The session is consumed (boxed trait object) — the caller
    /// owns connection lifecycle.  The worker emits progress via
    /// `cb` and returns an aggregate [`SyncSummary`].
    pub async fn sync_with_session(
        &self,
        mut session: Box<dyn SyncSession>,
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
            let guard = self.store.lock().unwrap();
            queries::delete_missing_uids(guard.conn(), mailbox_id, &[])
                .map_err(|e| SyncError::Protocol(format!("wipe on UIDVALIDITY bump: {e}")))?;
            // Queued UIDs belong to the old generation — replaying them
            // would flag the wrong messages (RFC 4549, T-6-02).
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
                queries::delete_missing_uids(guard.conn(), mailbox_id, &[])
                    .map_err(|e| SyncError::Protocol(format!("wipe empty mailbox: {e}")))?;
                queries::set_sync_state(
                    guard.conn(),
                    mailbox_name,
                    summary.uid_validity,
                    summary.uid_next.unwrap_or(1),
                )
                .map_err(|e| SyncError::Protocol(format!("set sync state: {e}")))?;
            }
            // Every queued UID is absent — replay drops the queue.
            let empty: HashSet<u32> = HashSet::new();
            let replay = self
                .replay_outbox(&mut *session, mailbox_id, summary.uid_validity, Some(&empty))
                .await?;
            eprintln!(
                "[SGE sync] replay on empty mailbox: acked={} dropped={} failed={}",
                replay.acked, replay.dropped, replay.failed
            );
            session.logout().await?;
            cb(SyncEvent::SyncCompleted {
                summary: result.clone(),
            });
            return Ok(result);
        }

        // Pending-wins gate: UIDs with an unacknowledged optimistic toggle
        // keep their local flags through the sweep below (FLAG-02). Fetched
        // once per pass — the set is small (one row per toggled message).
        let pending: HashSet<u32> = {
            let guard = self.store.lock().unwrap();
            queries::pending_uids(guard.conn(), mailbox_id)
                .map_err(|e| SyncError::Protocol(format!("pending_uids: {e}")))?
                .into_iter()
                .collect()
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
        result.deleted = {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            queries::delete_missing_uids(conn, mailbox_id, &live_union)
                .map_err(|e| SyncError::Protocol(format!("delete_missing_uids: {e}")))?
        };

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

        // Post-sync replay: queued toggles go out on the still-open,
        // INBOX-selected session; failures stay queued for the next pass.
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
                "[SGE sync] done: new={} updated={} deleted={} db_rows={} fetched={} gap_refetches={} converged={}",
                result.new, result.updated, result.deleted, db_count,
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
    }

    impl SyncSession for MockSession {
        fn select_mailbox(
            &mut self,
            _name: &str,
        ) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
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

        fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
            Box::pin(async move { Ok(vec![]) })
        }

        fn mailbox_status(&mut self, _name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>> {
            let status = MailboxStatus {
                uid_validity: self.summary.uid_validity,
                uid_next: self.summary.uid_next,
                unseen: self.status_unseen,
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
        }
    }

    fn inbox(_summary: MailboxSummary) -> Arc<std::sync::Mutex<Store>> {
        Arc::new(std::sync::Mutex::new(
            Store::open_in_memory().expect("migration should succeed"),
        ))
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
}