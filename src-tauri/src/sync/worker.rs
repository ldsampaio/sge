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

impl SyncWorker {
    pub fn new(store: Arc<std::sync::Mutex<Store>>) -> Self {
        Self { store }
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
            session.logout().await?;
            cb(SyncEvent::SyncCompleted {
                summary: result.clone(),
            });
            return Ok(result);
        }

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
                        &header.flags,
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
}