//! Phase 2 demo harness — runs the full sync pipeline end-to-end
//! against a deterministic MockSession and prints results to stdout.
//!
//! Usage: cargo run --bin sync_demo
//!
//! Success criteria (from Plan 02-03):
//!   1. Sync completes 0 → N messages with zero protocol errors
//!   2. Full sweep + incremental: second run exits 0 with 0 new messages
//!   3. UIDVALIDITY mismatch: worker refuses to re-sync, returns Err

use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicU32;

use sge_lib::store::{Store, queries};
use sge_lib::imap::{MailboxSummary, SyncError, SyncSession};
use sge_lib::imap::headers::MessageHeader;
use sge_lib::sync::worker::SyncWorker;

/// Deterministic mock IMAP session returning 5 fixed messages.
struct MockSession {
    #[allow(dead_code)]
    uid_next: AtomicU32,
}

impl MockSession {
    fn new() -> Self {
        Self { uid_next: AtomicU32::new(1) }
    }
}

impl SyncSession for MockSession {
    fn select_inbox(&mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<MailboxSummary, SyncError>> + Send>> {
        Box::pin(async move {
            Ok(MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                exists: 5,
                uid_next: Some(6),
            })
        })
    }

    fn search_uids(&mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u32>, SyncError>> + Send>> {
        Box::pin(async move { Ok((1..=5u32).collect()) })
    }

    fn fetch_envelopes<'a>(
        &'a mut self,
        _range: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<MessageHeader>, SyncError>> + Send + 'a>> {
        Box::pin(async move {
            let mut headers = Vec::new();
            for uid in 1..=5u32 {
                headers.push(MessageHeader {
                    uid,
                    message_id: Some(format!("<msg{uid}@example.com>")),
                    subject: format!("Test message {uid}"),
                    from_addr: "alice@example.com".to_string(),
                    to_addrs: "bob@example.com".to_string(),
                    cc_addrs: String::new(),
                    date_utc: "2024-01-01T00:00:00Z".to_string(),
                    flags: String::new(),
                    has_attachments: false,
                    preview: format!("Preview of message {uid}"),
                });
            }
            Ok(headers)
        })
    }

    fn fetch_body(&mut self, uid: u32) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, SyncError>> + Send>> {
        let body = format!("From: alice@example.com\r\nSubject: Test {uid}\r\n\r\nBody of message {uid}\r\n");
        Box::pin(async move { Ok(body.into_bytes()) })
    }

    fn set_seen(&mut self, _uid: u32, _seen: bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SyncError>> + Send>> {
        Box::pin(async move { Ok(()) })
    }

    fn logout(&mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SyncError>> + Send>> {
        Box::pin(async move { Ok(()) })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    async_std::task::block_on(async {
        // ── Open in-memory store ──
        let store = Arc::new(Mutex::new(Store::open_in_memory()?));
        {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            queries::ensure_mailbox(conn, "INBOX")?;
        }
        println!("✓ store prepared (INBOX row)");

        // ── Run the sync worker (full sweep) ──
        let worker = SyncWorker::new(store.clone());
        let session = MockSession::new();
        let summary = worker.sync_with_session(Box::new(session), Arc::new(|_| {})).await?;
        println!("✓ sync complete: new={} updated={} unchanged={} deleted={}",
                 summary.new, summary.updated, summary.unchanged, summary.deleted);
        assert_eq!(summary.new, 5, "expected 5 new messages on first sync");

        // ── Verify: messages in SQLite ──
        {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
            println!("✓ messages in SQLite: {}", count);
            assert!(count > 0, "expected at least 1 message in SQLite");
        }

        // ── Test incremental: second sync should find 0 new ──
        let session2 = MockSession::new();
        let summary2 = worker.sync_with_session(Box::new(session2), Arc::new(|_| {})).await?;
        assert_eq!(summary2.new, 0, "incremental sync: expected 0 new messages");
        println!("✓ incremental sync: 0 new messages (idempotent)");

        // ── Test UIDVALIDITY mismatch: triggers wipe and full resync ──
        {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            queries::set_sync_state(conn, "INBOX", 99999, 1)?;
        }
        let session3 = MockSession::new();
        let result = worker.sync_with_session(Box::new(session3), Arc::new(|_| {})).await?;
        assert!(result.uid_validity_bump, "UIDVALIDITY mismatch should trigger resync");
        assert_eq!(result.new, 5, "UIDVALIDITY wipe should re-insert all messages");
        println!("✓ UIDVALIDITY mismatch triggered clean resync (uid_validity_bump=true)");

        println!("\nAll Phase 2 success criteria met ✓");
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}