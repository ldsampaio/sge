//! Sync worker: the 7-step header sweep + body fetch engine.
//!
//! Algorithm (Plan 02-02, Wave 2):
//! 1. Ensure mailbox row + read sync state
//! 2. SELECT INBOX → MailboxSummary (UIDVALIDITY, UIDNEXT, exists)
//! 3. UIDVALIDITY guard — wipe if changed
//! 4. Compute fetch_from (incremental or full resync)
//! 5. Header sweep — 200-UID batches via fetch_envelopes
//! 6. Expunge diff — delete UIDs not on server
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
        Self { store }
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
        cb: SyncCallback,
    ) -> Result<SyncSummary, SyncError> {
        // ── Step 1: ensure mailbox row + read sync state ─────────
        let mailbox_id = {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            queries::ensure_mailbox(conn, "INBOX")
                .map_err(|e| SyncError::Protocol(format!("ensure mailbox: {e}")))?
        };
        let (prev_uidv, _prev_next) = {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            let prev = queries::get_sync_state(conn, "INBOX")
                .map_err(|e| SyncError::Protocol(format!("get sync state: {e}")))?;
            prev.unwrap_or((0u32, 0u32))
        };

        // ── Step 2: SELECT INBOX ──────────────────────────────────
        let summary: MailboxSummary = session.select_inbox().await?;
        eprintln!(
            "[SGE sync] SELECT INBOX: exists={} uid_validity={} uid_next={:?}",
            summary.exists, summary.uid_validity, summary.uid_next
        );

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
                    "INBOX",
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
        let total_messages = server_uids.len() as u32;

        for chunk in server_uids.chunks(BATCH_SIZE as usize) {
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

                    if existing_set.contains(&uid) {
                        result.updated += 1;
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
        result.deleted = {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            queries::delete_missing_uids(conn, mailbox_id, &server_uids)
                .map_err(|e| SyncError::Protocol(format!("delete_missing_uids: {e}")))?
        };

        // ── Step 7: write sync state + logout ──────────────────────
        let new_next = summary.uid_next.unwrap_or_else(|| {
            server_uids.last().map(|&u| u + 1).unwrap_or(1)
        });
        {
            let guard = self.store.lock().unwrap();
            let conn = guard.conn();
            queries::set_sync_state(conn, "INBOX", summary.uid_validity, new_next)
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
                "[SGE sync] done: new={} updated={} deleted={} db_rows={}",
                result.new, result.updated, result.deleted, db_count
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
    use crate::imap::PinBox;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
    }

    impl SyncSession for MockSession {
        fn select_inbox(&mut self) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
            let summary = self.summary.clone();
            Box::pin(async move { Ok(summary) })
        }

        fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            let uids = self.envelopes.iter().map(|h| h.uid).collect();
            Box::pin(async move { Ok(uids) })
        }

        fn fetch_envelopes<'a>(
            &'a mut self,
            _range: &'a str,
        ) -> PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
            self.fetch_calls.fetch_add(1, Ordering::SeqCst);
            let envelopes = self.envelopes.clone();
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
            worker.sync_with_session(Box::new(session), cb()).await
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

    /// Second run with same data → no new, all updated.
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
            worker.sync_with_session(Box::new(session), cb()).await
        })
        .unwrap();

        // Second run — same data, should be all updates
        let session2 = mock(summary, vec![mkhdr(1), mkhdr(2), mkhdr(3)]);
        let result = async_std::task::block_on(async {
            worker.sync_with_session(Box::new(session2), cb()).await
        })
        .unwrap();

        assert_eq!(result.new, 0);
        assert_eq!(result.updated, 3);
        assert_eq!(result.deleted, 0);
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
        async_std::task::block_on(async { worker.sync_with_session(Box::new(session), cb()).await })
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
                .sync_with_session(Box::new(session2), cb())
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
            worker.sync_with_session(Box::new(session), cb()).await
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
            worker.sync_with_session(Box::new(session), cb()).await
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
}