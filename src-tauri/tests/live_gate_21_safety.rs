//! Phase 21 offline safety doubles (`live_gate_21_safety`).
//!
//! Zero-network contract tests for the live-gate harness: they drive the
//! REAL harness helpers (recipient guard, prefix guard, LIST-delta,
//! exactly-one, verbatim compare, offline gate decorator — included straight
//! from the bin, so there is exactly one implementation) plus the REAL prod
//! verbs (`enqueue_send`, `flush_send_queue`, the send cap, BCC
//! envelope-only) against in-memory doubles. CI-green with no creds.

#[path = "../src/bin/live_gate_21.rs"]
#[allow(dead_code)]
mod harness;

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use sge_lib::imap::{
    headers::MessageHeader, MailboxInfo, MailboxStatus, MailboxSummary, SyncError, SyncSession,
};
use sge_lib::send_queue::{self, EnqueueInput};
use sge_lib::smtp::{SmtpAccount, SmtpTransport};
use sge_lib::store::{queries, Store};
use sge_lib::sync::worker::SyncWorker;

/// Minimal in-memory IMAP double: just enough server behavior for the gate
/// open/closed proof (append + reads + expunge scoping).
struct FakeSession {
    mailboxes: HashSet<String>,
    selected: Option<String>,
    next_uid: u32,
    /// uid -> (mailbox, message_id, bytes, deleted)
    messages: HashMap<u32, (String, String, Vec<u8>, bool)>,
}

impl FakeSession {
    fn new() -> Self {
        Self {
            mailboxes: HashSet::new(),
            selected: None,
            next_uid: 1,
            messages: HashMap::new(),
        }
    }

    fn with_mailbox(mb: &str) -> Self {
        let mut s = Self::new();
        s.mailboxes.insert(mb.to_string());
        s.selected = Some(mb.to_string());
        s
    }
}

impl SyncSession for FakeSession {
    fn select_mailbox(
        &mut self,
        name: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<MailboxSummary, SyncError>> {
        let name = name.to_string();
        Box::pin(async move {
            if !self.mailboxes.contains(&name) {
                return Err(SyncError::Protocol(format!("no such mailbox {name}")));
            }
            self.selected = Some(name.clone());
            Ok(MailboxSummary {
                selected_mailbox: name,
                uid_validity: 4242,
                exists: self.messages.len() as u32,
                uid_next: Some(self.next_uid),
            })
        })
    }

    fn search_uids(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move {
            let mut uids: Vec<u32> = self.messages.keys().copied().collect();
            uids.sort_unstable();
            Ok(uids)
        })
    }

    fn fetch_envelopes<'a>(
        &'a mut self,
        range: &'a str,
    ) -> sge_lib::imap::PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
        Box::pin(async move {
            let mut out = Vec::new();
            for part in range.split(',') {
                if let Ok(uid) = part.trim().parse::<u32>() {
                    if self.messages.contains_key(&uid) {
                        out.push(MessageHeader {
                            uid,
                            message_id: Some("<a@x>".to_string()),
                            subject: format!("SGE-TEST-subject-{uid}"),
                            from_addr: "me@example.com".to_string(),
                            to_addrs: "me@example.com".to_string(),
                            cc_addrs: String::new(),
                            date_utc: "2026-10-13T12:00:00Z".to_string(),
                            flags: "[]".to_string(),
                            has_attachments: false,
                            preview: String::new(),
                        });
                    }
                }
            }
            Ok(out)
        })
    }

    fn fetch_body(&mut self, uid: u32) -> sge_lib::imap::PinBox<'_, Result<Vec<u8>, SyncError>> {
        Box::pin(async move {
            self.messages
                .get(&uid)
                .map(|(_, _, bytes, _)| bytes.clone())
                .ok_or_else(|| SyncError::Protocol(format!("no such uid {uid}")))
        })
    }

    fn set_seen(&mut self, uid: u32, _seen: bool) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            if self.messages.contains_key(&uid) {
                Ok(())
            } else {
                Err(SyncError::Protocol(format!("no such uid {uid}")))
            }
        })
    }

    fn store_deleted(
        &mut self,
        uid: u32,
        deleted: bool,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            match self.messages.get_mut(&uid) {
                Some(entry) => {
                    entry.3 = deleted;
                    Ok(())
                }
                None => Err(SyncError::Protocol(format!("no such uid {uid}"))),
            }
        })
    }

    fn expunge(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move {
            let gone: Vec<u32> = self
                .messages
                .iter()
                .filter(|(_, (_, _, _, d))| *d)
                .map(|(u, _)| *u)
                .collect();
            for u in &gone {
                self.messages.remove(u);
            }
            Ok(gone)
        })
    }

    fn uid_expunge(&mut self, uid_set: &str) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        let uid_set = uid_set.to_string();
        Box::pin(async move {
            let mut gone = Vec::new();
            for part in uid_set.split(',') {
                if let Ok(uid) = part.trim().parse::<u32>() {
                    if matches!(self.messages.get(&uid), Some((_, _, _, true))) {
                        self.messages.remove(&uid);
                        gone.push(uid);
                    }
                }
            }
            Ok(gone)
        })
    }

    fn uid_copy_to(
        &mut self,
        _uid_set: &str,
        _dest: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move { Ok(()) })
    }

    fn uid_move_to(
        &mut self,
        _uid_set: &str,
        _dest: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<String>, SyncError>> {
        Box::pin(async move { Ok(vec!["IMAP4rev1".to_string(), "UIDPLUS".to_string()]) })
    }

    fn create_mailbox(&mut self, name: &str) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        let name = name.to_string();
        Box::pin(async move {
            self.mailboxes.insert(name);
            Ok(())
        })
    }

    fn rename_mailbox(
        &mut self,
        _old: &str,
        _new: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move { Ok(()) })
    }

    fn delete_mailbox(&mut self, name: &str) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        let name = name.to_string();
        Box::pin(async move {
            self.mailboxes.remove(&name);
            Ok(())
        })
    }

    fn append_message(
        &mut self,
        mailbox: &str,
        _flags: &str,
        bytes: &[u8],
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        let mailbox = mailbox.to_string();
        let bytes = bytes.to_vec();
        Box::pin(async move {
            if !self.mailboxes.contains(&mailbox) {
                return Err(SyncError::Protocol(format!("no such mailbox {mailbox}")));
            }
            // Record the Message-ID like a server would index it.
            let text = String::from_utf8_lossy(&bytes);
            let msg_id = text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("message-id:"))
                .map(|l| l["message-id:".len()..].trim().to_string())
                .unwrap_or_default();
            let uid = self.next_uid;
            self.next_uid += 1;
            self.messages.insert(uid, (mailbox, msg_id, bytes, false));
            Ok(())
        })
    }

    fn uid_search_header(
        &mut self,
        _field: &str,
        value: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        let value = value.to_string();
        Box::pin(async move {
            let mut uids: Vec<u32> = self
                .messages
                .iter()
                .filter(|(_, (_, id, _, _))| *id == value)
                .map(|(u, _)| *u)
                .collect();
            uids.sort_unstable();
            Ok(uids)
        })
    }

    fn list_mailboxes(
        &mut self,
    ) -> sge_lib::imap::PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
        Box::pin(async move {
            let mut infos: Vec<MailboxInfo> = self
                .mailboxes
                .iter()
                .map(|n| MailboxInfo {
                    name: n.clone(),
                    display_name: n.clone(),
                    delimiter: "/".to_string(),
                    attributes: vec![],
                })
                .collect();
            infos.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(infos)
        })
    }

    fn mailbox_status(
        &mut self,
        _name: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<MailboxStatus, SyncError>> {
        Box::pin(async move {
            Ok(MailboxStatus {
                uid_validity: 4242,
                uid_next: Some(self.next_uid),
                unseen: 0,
                messages: self.messages.len() as u32,
            })
        })
    }

    fn logout(&mut self) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move { Ok(()) })
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    async_std::task::block_on(f)
}

fn test_app_data(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sge-live21-safety-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn self_input(subject_suffix: &str) -> EnqueueInput {
    EnqueueInput {
        from: "me@example.com".to_string(),
        to: vec!["me@example.com".to_string()],
        cc: vec![],
        bcc: vec![],
        subject: format!("{}live21-{subject_suffix}", harness::SUBJECT_PREFIX),
        body: "live gate probe".to_string(),
        draft_id: None,
    }
}

// ── Recipient guard (D-01, T-21-01 — zero network) ─────────────────────────

#[test]
fn live_gate_21_guard_accepts_self_only() {
    let me = "me@example.com".to_string();
    assert!(harness::assert_self_only(&me, &[me.clone()], &[], &[], &me).is_ok());
}

#[test]
fn live_gate_21_guard_refuses_foreign_to_cc_bcc() {
    let me = "me@example.com".to_string();
    let other = "other@example.com".to_string();
    assert!(harness::assert_self_only(&me, &[other.clone()], &[], &[], &me).is_err());
    assert!(harness::assert_self_only(&me, &[me.clone(), other.clone()], &[], &[], &me).is_err());
    assert!(harness::assert_self_only(&me, &[], &[], &[], &me).is_err());
    assert!(harness::assert_self_only(&me, &[me.clone()], &[other.clone()], &[], &me).is_err());
    assert!(harness::assert_self_only(&me, &[me.clone()], &[], &[other.clone()], &me).is_err());
    assert!(harness::assert_self_only(&other, &[me.clone()], &[], &[], &me).is_err());
}

#[test]
fn live_gate_21_guard_accepts_case_and_whitespace_variants() {
    assert!(harness::assert_self_only(
        "  ME@EXAMPLE.COM ",
        &["me@example.com".to_string()],
        &[],
        &[],
        "Me@Example.com"
    )
    .is_ok());
}

// ── Pre-flight prefix + LIST-delta (D-03, zero network) ────────────────────

#[test]
fn live_gate_21_preflight_prefix_and_delta() {
    assert!(harness::assert_sandbox_prefix("SGE-Test-1").is_ok());
    for wire in ["INBOX", "Sent", "SGE-Test", ""] {
        assert!(harness::assert_sandbox_prefix(wire).is_err());
    }
    let before = vec!["INBOX".to_string()];
    let after = vec!["INBOX".to_string(), "Evil".to_string()];
    assert_eq!(
        harness::unexpected_list_delta(&before, &after, "SGE-Test-never-x"),
        vec!["Evil".to_string()]
    );
}

// ── Offline gate decorator (zero network) ───────────────────────────────────

#[test]
fn live_gate_21_gate_closed_fails_writes_forwards_reads() {
    let mut inner = FakeSession::with_mailbox("INBOX");
    let flag = Arc::new(AtomicBool::new(true));
    let mut gate = harness::GateSession::new(&mut inner, flag.clone());
    gate.set_closed(true);
    assert!(gate.is_closed());

    assert!(block_on(gate.append_message("INBOX", "\\Seen", b"x")).is_err());
    assert!(block_on(gate.set_seen(1, true)).is_err());
    assert!(block_on(gate.store_deleted(1, true)).is_err());
    assert!(block_on(gate.uid_expunge("1")).is_err());

    assert!(block_on(gate.search_uids()).unwrap().is_empty());
    assert!(block_on(gate.capabilities()).unwrap().contains(&"UIDPLUS".to_string()));

    gate.set_closed(false);
    assert!(!gate.is_closed());
    assert!(block_on(gate.append_message("INBOX", "\\Seen", b"Message-ID: <a@x>\r\n\r\nb")).is_ok());
    assert_eq!(block_on(gate.search_uids()).unwrap(), vec![1]);
}

// ── Arrival + Sent-copy shapes (D-05/D-06 — zero network) ─────────────────
// SEARCH-count (exactly 1 UID per Message-ID) + verbatim byte-compare, the
// same shapes the live legs assert. Plus the offline flush shape: enqueue →
// snapshot pending → flush (offline transport, no manager) → sent == 1 →
// snapshot clean.

#[test]
fn live_gate_21_arrival_shape_search_count_and_verbatim() {
    let mut session = FakeSession::with_mailbox("INBOX");
    let mid_a = "<live21-shape-a@sge.local>".to_string();
    let mid_b = "<live21-shape-b@sge.local>".to_string();
    let bytes_a = format!("Message-ID: {mid_a}\r\nSubject: {0}shape\r\n\r\nbody-a", harness::SUBJECT_PREFIX).into_bytes();
    let bytes_b = format!("Message-ID: {mid_b}\r\nSubject: {0}shape\r\n\r\nbody-b", harness::SUBJECT_PREFIX).into_bytes();
    block_on(session.append_message("INBOX", "", &bytes_a)).unwrap();
    block_on(session.append_message("INBOX", "", &bytes_b)).unwrap();

    // SEARCH isolates per Message-ID: exactly 1 UID each, never the union.
    let hits_a = block_on(session.uid_search_header("Message-ID", &mid_a)).unwrap();
    let uid_a = harness::assert_exactly_one(&hits_a, "arrival-shape").unwrap();
    let hits_b = block_on(session.uid_search_header("Message-ID", &mid_b)).unwrap();
    let uid_b = harness::assert_exactly_one(&hits_b, "arrival-shape").unwrap();
    assert_ne!(uid_a, uid_b);

    // Verbatim byte-compare of the fetched copy against the queued bytes.
    let fetched = block_on(session.fetch_body(uid_a)).unwrap();
    assert!(harness::verbatim_equal(&fetched, &bytes_a));
    assert!(!harness::verbatim_equal(&fetched, &bytes_b));

    // Unknown Message-ID: zero hits → exactly-one refuses.
    let none = block_on(session.uid_search_header("Message-ID", "<nope@sge.local>")).unwrap();
    assert!(none.is_empty());
    assert!(harness::assert_exactly_one(&none, "arrival-shape").is_err());
}

#[test]
fn live_gate_21_offline_flush_shape_enqueue_to_sent() {
    use sge_lib::send_queue::SendGate;
    use sge_lib::sync::worker::{FlushSummary, SendFlushEnv};

    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let app_data = test_app_data("flush-shape");
    let mid = {
        let guard = store.lock().unwrap();
        let out = send_queue::enqueue_send(guard.conn(), &app_data, self_input("flush")).unwrap();
        assert!(!out.deduped);
        // A-01 shape: pending visible before any network.
        let snap = send_queue::send_status_snapshot(guard.conn()).unwrap();
        assert!(snap.pending_count >= 1 && snap.queued >= 1);
        out.message_id
    };
    let (transport, calls) = harness::CountingTransport::offline();
    let transport: Arc<dyn SmtpTransport> = Arc::new(transport);
    let worker = SyncWorker::new(store.clone());
    let env = SendFlushEnv {
        gate: SendGate::default(),
        transport,
        account: SmtpAccount {
            host: "smtp.example".to_string(),
            port: 587,
            username: "u".to_string(),
            password: "p".to_string(),
        },
        manager: None,
        sent_wire: None,
    };
    let summary = block_on(worker.flush_send_queue(&env));
    assert_eq!(summary.sent, 1, "offline flush must send the one queued row");
    assert_eq!(summary, FlushSummary { sent: 1, ..FlushSummary::default() });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // A-02/A-03 shape: clean state after ack.
    {
        let guard = store.lock().unwrap();
        let snap = send_queue::send_status_snapshot(guard.conn()).unwrap();
        assert_eq!(snap.pending_count, 0);
        assert_eq!(snap.failed, 0);
        assert_eq!(snap.uncertain, 0);
        let row = queries::get_send_row_by_message_id(guard.conn(), &mid).unwrap().unwrap();
        assert_eq!(row.state, "sent");
    }
    std::fs::remove_dir_all(&app_data).ok();
}
// ── No-duplicate + sent-unfiled shapes (D-07 — zero network) ─────────────
// Double-enqueue of one Message-ID (via the draft anchor, the DRAFT-03
// link) dedupes to one row + one .eml through the REAL prod path; a
// server-side twin (two APPENDs, one Message-ID) is DETECTED by the
// exactly-one assert (2 hits refuse); parked sent rows are never re-sent
// (second flush: empty summary, zero new transport calls). The live legs
// assert the same shapes against the real server.

#[test]
fn live_gate_21_double_enqueue_same_draft_dedupes_to_one_row() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let app_data = test_app_data("dedupe");
    let mb = queries::ensure_mailbox(conn, "INBOX").unwrap();
    let draft_id = "live21-safety-nondup";
    let mid = sge_lib::drafts::new_message_id("live21-safety-nondup");
    queries::upsert_draft(
        conn,
        draft_id,
        mb,
        &mid,
        "SGE-TEST-safety-nondup",
        "probe",
        "me@example.com",
        "",
        "",
    )
    .unwrap();
    let mk = || EnqueueInput {
        from: "me@example.com".to_string(),
        to: vec!["me@example.com".to_string()],
        cc: vec![],
        bcc: vec![],
        subject: format!("{}safety-nondup", harness::SUBJECT_PREFIX),
        body: "probe".to_string(),
        draft_id: Some(draft_id.to_string()),
    };
    let first = send_queue::enqueue_send(conn, &app_data, mk()).unwrap();
    assert!(!first.deduped);
    assert_eq!(first.message_id, mid, "draft anchor reuses the draft Message-ID");
    let second = send_queue::enqueue_send(conn, &app_data, mk()).unwrap();
    assert!(second.deduped, "second enqueue of one Message-ID must dedupe");
    assert_eq!(second.queue_id, first.queue_id);
    assert_eq!(second.message_id, first.message_id);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM send_queue WHERE message_id = ?1", [mid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1, "one Message-ID → exactly one row");
    let eml_count = std::fs::read_dir(app_data.join("outbox")).map(|rd| rd.count()).unwrap_or(0);
    assert_eq!(eml_count, 1, "one Message-ID → exactly one .eml");
    std::fs::remove_dir_all(&app_data).ok();
}

#[test]
fn live_gate_21_server_side_twin_is_detected_not_collapsed() {
    // A server holding TWO copies of one Message-ID (the duplicate-delivery
    // failure) must FAIL the exactly-one assert — the harness never
    // "collapses" twins, it refuses them.
    let mut session = FakeSession::with_mailbox("INBOX");
    let mid = "<live21-twin@sge.local>".to_string();
    let bytes = format!("Message-ID: {mid}\r\nSubject: {0}twin\r\n\r\nb", harness::SUBJECT_PREFIX).into_bytes();
    block_on(session.append_message("INBOX", "\\Seen", &bytes)).unwrap();
    block_on(session.append_message("INBOX", "\\Seen", &bytes)).unwrap();
    let hits = block_on(session.uid_search_header("Message-ID", &mid)).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(
        harness::assert_exactly_one(&hits, "no-duplicate").is_err(),
        "twin delivery must refuse, never collapse"
    );
}

#[test]
fn live_gate_21_sent_unfiled_shape_never_resends() {
    use sge_lib::send_queue::SendGate;
    use sge_lib::sync::worker::{FlushSummary, SendFlushEnv};

    // Gate-closed APPEND fails (the offline analogue of the APPEND-failure
    // half of SentUnfiled) with zero transport involvement.
    let mut inner = FakeSession::with_mailbox("Sent");
    let flag = Arc::new(AtomicBool::new(true));
    let mut gate = harness::GateSession::new(&mut inner, flag.clone());
    gate.set_closed(true);
    assert!(block_on(gate.append_message("Sent", "\\Seen", b"bytes")).is_err());

    // Parked sent rows are never re-sent: first flush sends once, second
    // flush is empty with zero new transport calls.
    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let app_data = test_app_data("unfiled-shape");
    {
        let guard = store.lock().unwrap();
        send_queue::enqueue_send(guard.conn(), &app_data, self_input("unfiled")).unwrap();
    }
    let (transport, calls) = harness::CountingTransport::offline();
    let transport: Arc<dyn SmtpTransport> = Arc::new(transport);
    let worker = SyncWorker::new(store.clone());
    let env = SendFlushEnv {
        gate: SendGate::default(),
        transport,
        account: SmtpAccount {
            host: "smtp.example".to_string(),
            port: 587,
            username: "u".to_string(),
            password: "p".to_string(),
        },
        manager: None,
        sent_wire: None,
    };
    let first = block_on(worker.flush_send_queue(&env));
    assert_eq!(first.sent, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let second = block_on(worker.flush_send_queue(&env));
    assert_eq!(second, FlushSummary::default(), "parked rows must never be due");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no second send_raw after park");
    // A-07 observability gap, pinned (RESEARCH Pitfall 5): the snapshot
    // hard-codes sent_unfiled 0 — A-07 asserts via flush outcomes +
    // call counts, never via this field.
    {
        let guard = store.lock().unwrap();
        let snap = send_queue::send_status_snapshot(guard.conn()).unwrap();
        assert_eq!(snap.sent_unfiled, 0, "gap pin: snapshot hard-codes 0");
    }
    std::fs::remove_dir_all(&app_data).ok();
}

// ── D-12 doubles (explicit, offline only — never attempted live) ───────────
// (a) oversize input over the 25 MB cap rejected before any transport call
// (zero send_raw calls); (b) envelope log of a self-send contains no Bcc:
// line.

#[test]
fn live_gate_21_oversize_rejected_before_any_transport_call() {
    use sge_lib::send_queue::SendGate;
    use sge_lib::sync::worker::{FlushSummary, SendFlushEnv};

    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let app_data = test_app_data("oversize");
        let mut bad = self_input("oversize");
        bad.body = "x".repeat(send_queue::SEND_MAX_BYTES + 1);
        let err = send_queue::enqueue_send(conn, &app_data, bad).unwrap_err();
        assert!(
            err.to_string().contains("send-too-large"),
            "unexpected: {err}"
        );
        // Refusal leaves neither row nor file behind.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM send_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "refusal must leave no partial row");
        std::fs::remove_dir_all(&app_data).ok();
    }
    // A flush over the refused store finds nothing due: the transport sees
    // zero send_raw calls (rejected before any transport call).
    let (transport, calls) = harness::CountingTransport::offline();
    let transport: Arc<dyn SmtpTransport> = Arc::new(transport);
    let worker = SyncWorker::new(store);
    let acct = SmtpAccount {
        host: "smtp.example".to_string(),
        port: 587,
        username: "u".to_string(),
        password: "p".to_string(),
    };
    let env = SendFlushEnv {
        gate: SendGate::default(),
        transport,
        account: acct,
        manager: None,
        sent_wire: None,
    };
    let summary = block_on(worker.flush_send_queue(&env));
    assert_eq!(summary, FlushSummary::default());
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no row due → zero transport calls");
}

#[test]
fn live_gate_21_envelope_log_has_no_bcc_line_but_envelope_keeps_bcc() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let app_data = test_app_data("bcc");
    // BCC rides the envelope only: rendered headers must not leak it.
    let mut input = self_input("bcc");
    input.bcc = vec!["hidden@example.com".to_string()];
    let out = send_queue::enqueue_send(conn, &app_data, input).unwrap();
    let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
    let eml = std::fs::read(&row.eml_path).unwrap();
    let text = String::from_utf8_lossy(&eml);
    assert!(
        !text.lines().any(|l| l.to_ascii_lowercase().starts_with("bcc:")),
        "rendered .eml must contain no Bcc: header line"
    );
    assert!(
        row.bcc_addrs.contains("hidden@example.com"),
        "envelope column must retain the BCC recipient"
    );
    let line = harness::envelope_log_line(&row.from_addr, &["me@example.com".to_string()]);
    assert!(!line.to_ascii_lowercase().contains("bcc:"));
    std::fs::remove_dir_all(&app_data).ok();
}

// ── Retry / uncertain / crash shapes (D-09/D-10/D-11 — zero network) ─────
// The REAL prod path under a fault decorator: armed flush marks `uncertain`
// with zero transport calls; reconcile SEARCH-hit confirms without resend;
// SEARCH-miss requeues once with a bounded backoff; a stranded `sending`
// row is triaged to `uncertain` by the flush pass (never back to `queued` —
// RESEARCH Pitfall 4) and `requeue_failed` refuses non-failed rows (A-06:
// uncertain never offers resend-as-Retentar).

fn offline_flush_env(
    transport: Arc<dyn SmtpTransport>,
) -> sge_lib::sync::worker::SendFlushEnv {
    use sge_lib::send_queue::SendGate;
    sge_lib::sync::worker::SendFlushEnv {
        gate: SendGate::default(),
        transport,
        account: SmtpAccount {
            host: "smtp.example".to_string(),
            port: 587,
            username: "u".to_string(),
            password: "p".to_string(),
        },
        manager: None,
        sent_wire: None,
    }
}

#[test]
fn live_gate_21_retry_armed_flush_marks_uncertain_with_zero_calls() {
    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let app_data = test_app_data("retry-armed");
    let (qid, mid) = {
        let guard = store.lock().unwrap();
        let out = send_queue::enqueue_send(guard.conn(), &app_data, self_input("armed")).unwrap();
        (out.queue_id, out.message_id)
    };
    let (inner, calls) = harness::CountingTransport::offline();
    let (fault, armed, forwarded) = harness::TimeoutTransport::new(Arc::new(inner));
    armed.store(true, Ordering::SeqCst);
    let worker = SyncWorker::new(store.clone());
    let summary = block_on(worker.flush_send_queue(&offline_flush_env(Arc::new(fault))));
    assert_eq!(summary.uncertain, 1, "armed fault must mark the row uncertain");
    assert_eq!(summary.sent, 0, "faulted attempt must not send");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "zero inner calls while armed");
    assert_eq!(forwarded.load(Ordering::SeqCst), 0, "zero forwarded calls while armed");
    // A-04/A-06 shape: uncertain hold visible, never failed.
    {
        let guard = store.lock().unwrap();
        let snap = send_queue::send_status_snapshot(guard.conn()).unwrap();
        assert!(snap.uncertain >= 1 && snap.failed == 0);
        let row = queries::get_send_row(guard.conn(), &qid).unwrap().unwrap();
        assert_eq!(row.state, "uncertain");
        assert_eq!(row.message_id, mid);
    }
    std::fs::remove_dir_all(&app_data).ok();
}

#[test]
fn live_gate_21_retry_reconcile_hit_confirms_without_resend() {
    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let app_data = test_app_data("retry-hit");
    let mid = {
        let guard = store.lock().unwrap();
        let out = send_queue::enqueue_send(guard.conn(), &app_data, self_input("hit")).unwrap();
        // Drive the row to `uncertain` through the REAL armed-fault path.
        let (inner, _) = harness::CountingTransport::offline();
        let (fault, armed, _) = harness::TimeoutTransport::new(Arc::new(inner));
        armed.store(true, Ordering::SeqCst);
        drop(guard);
        let worker = SyncWorker::new(store.clone());
        let summary = block_on(worker.flush_send_queue(&offline_flush_env(Arc::new(fault))));
        assert_eq!(summary.uncertain, 1);
        out.message_id
    };
    // Simulated ambiguous server-accept: the bytes sit in Sent.
    let mut fake = FakeSession::with_mailbox("Sent");
    let bytes = format!("Message-ID: {mid}\r\nSubject: {0}hit\r\n\r\nb", harness::SUBJECT_PREFIX).into_bytes();
    block_on(fake.append_message("Sent", "\\Seen", &bytes)).unwrap();
    let (inner, calls) = harness::CountingTransport::offline();
    let worker = SyncWorker::new(store.clone());
    let before = calls.load(Ordering::SeqCst);
    let rec = block_on(worker.reconcile_uncertain_sends(&mut fake, Some("Sent"), "INBOX")).unwrap();
    assert_eq!(rec.confirmed_sent, 1, "SEARCH hit must confirm");
    assert_eq!(rec.requeued, 0);
    assert_eq!(rec.still_uncertain, 0);
    assert_eq!(calls.load(Ordering::SeqCst), before, "reconcile must not call the transport");
    {
        let guard = store.lock().unwrap();
        let row = queries::get_send_row_by_message_id(guard.conn(), &mid).unwrap().unwrap();
        assert_eq!(row.state, "sent");
    }
    let _ = inner;
    std::fs::remove_dir_all(&app_data).ok();
}

#[test]
fn live_gate_21_retry_reconcile_miss_requeues_once_with_bounded_backoff() {
    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let app_data = test_app_data("retry-miss");
    let (qid, mid) = {
        let guard = store.lock().unwrap();
        let out = send_queue::enqueue_send(guard.conn(), &app_data, self_input("miss")).unwrap();
        queries::mark_send_uncertain(guard.conn(), &out.queue_id, "injected timeout").unwrap();
        (out.queue_id, out.message_id)
    };
    // Sent exists but holds nothing for this MID → SEARCH miss.
    let mut fake = FakeSession::with_mailbox("Sent");
    let worker = SyncWorker::new(store.clone());
    let rec = block_on(worker.reconcile_uncertain_sends(&mut fake, Some("Sent"), "INBOX")).unwrap();
    assert_eq!(rec.requeued, 1, "SEARCH miss must requeue exactly once");
    assert_eq!(rec.confirmed_sent, 0);
    assert_eq!(rec.still_uncertain, 0);
    {
        let guard = store.lock().unwrap();
        let row = queries::get_send_row(guard.conn(), &qid).unwrap().unwrap();
        assert_eq!(row.state, "queued");
        assert_eq!(row.attempts, 1, "miss counts the attempt");
        let now = chrono::Utc::now().timestamp();
        assert!(
            harness::retry_within_bounds(&row.next_retry_at, 1, now, 15),
            "requeue backoff must sit inside backoff_bounds(1): {:?}",
            row.next_retry_at
        );
        assert_eq!(row.message_id, mid);
    }
    std::fs::remove_dir_all(&app_data).ok();
}

#[test]
fn live_gate_21_retry_crash_claim_triaged_to_uncertain_never_queued() {
    let store = Arc::new(std::sync::Mutex::new(Store::open_in_memory().unwrap()));
    let app_data = test_app_data("retry-crash");
    let qid = {
        let guard = store.lock().unwrap();
        let out = send_queue::enqueue_send(guard.conn(), &app_data, self_input("crash")).unwrap();
        // Kill between claim and verdict: the exact first write the flush
        // performs, then no verdict ever lands.
        queries::set_send_state(guard.conn(), &out.queue_id, queries::SEND_STATE_SENDING).unwrap();
        out.queue_id
    };
    // Restart #1: launch-discipline crash_recover resets to queued (D-11).
    {
        let guard = store.lock().unwrap();
        assert_eq!(send_queue::crash_recover(guard.conn()).unwrap(), 1);
        let row = queries::get_send_row(guard.conn(), &qid).unwrap().unwrap();
        assert_eq!(row.state, queries::SEND_STATE_QUEUED);
    }
    // Kill #2 strands the row again; restart straight into a flush pass —
    // the pass header triage promotes stranded `sending` to `uncertain`
    // (Pitfall 4: never assert `queued` here).
    {
        let guard = store.lock().unwrap();
        queries::set_send_state(guard.conn(), &qid, queries::SEND_STATE_SENDING).unwrap();
    }
    let (transport, calls) = harness::CountingTransport::offline();
    let worker = SyncWorker::new(store.clone());
    let summary = block_on(worker.flush_send_queue(&offline_flush_env(Arc::new(transport))));
    assert_eq!(summary, sge_lib::sync::worker::FlushSummary::default(), "triage emits no send outcome");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "triage must send nothing");
    {
        let guard = store.lock().unwrap();
        let row = queries::get_send_row(guard.conn(), &qid).unwrap().unwrap();
        assert_eq!(row.state, queries::SEND_STATE_UNCERTAIN, "stranded sending triages to uncertain, not queued");
        let snap = send_queue::send_status_snapshot(guard.conn()).unwrap();
        assert!(snap.uncertain >= 1 && snap.failed == 0, "A-05/A-06: uncertain hold, never failed");
        // A-06 contract: retry never yanks a row out of `uncertain` (the
        // Retentar backend only requeues genuinely failed rows).
        assert!(
            !send_queue::requeue_failed(guard.conn(), &qid).unwrap(),
            "requeue_failed must refuse uncertain rows"
        );
    }
    std::fs::remove_dir_all(&app_data).ok();
}

#[test]
fn live_gate_21_retry_recorded_failure_schedules_inside_bounds() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let app_data = test_app_data("retry-bounds");
    let out = send_queue::enqueue_send(conn, &app_data, self_input("bounds")).unwrap();
    let attempts = send_queue::record_send_failure(conn, &out.queue_id, "boom").unwrap();
    assert_eq!(attempts, 1);
    let row = queries::get_send_row(conn, &out.queue_id).unwrap().unwrap();
    let now = chrono::Utc::now().timestamp();
    assert!(
        harness::retry_within_bounds(&row.next_retry_at, 1, now, 15),
        "next_retry_at must sit inside backoff_bounds(1): {:?}",
        row.next_retry_at
    );
    std::fs::remove_dir_all(&app_data).ok();
}

// ── Send-audit + secret-scan fixtures (D-13/D-14 — zero network) ─────────
// Each violation shape the wrapper re-checks from the log is detected by
// the bin helpers first: foreign recipients, non-empty cc/bcc, unprefixed
// subjects, Bcc: header lines (header block only — body mentions pass),
// reused Message-IDs, inflated call counts, planted credential substrings
// (regex metachars included — matching is fixed-substring).

#[test]
fn live_gate_21_safety_audit_recipients_self_only_rule() {
    let me = "me@example.com";
    assert!(harness::audit_recipients_ok(r#"["me@example.com"]"#, "[]", "[]", me));
    // Case + whitespace tolerance mirrors assert_self_only.
    assert!(harness::audit_recipients_ok(r#"["  ME@EXAMPLE.COM  "]"#, "[]", "[]", me));
    // Foreign To, extra To, empty To.
    assert!(!harness::audit_recipients_ok(r#"["other@example.com"]"#, "[]", "[]", me));
    assert!(!harness::audit_recipients_ok(r#"["me@example.com","other@example.com"]"#, "[]", "[]", me));
    assert!(!harness::audit_recipients_ok("[]", "[]", "[]", me));
    // Non-empty Cc / Bcc (whitespace-only still counts as empty, per D-01).
    assert!(!harness::audit_recipients_ok(r#"["me@example.com"]"#, r#"["x@y.z"]"#, "[]", me));
    assert!(!harness::audit_recipients_ok(r#"["me@example.com"]"#, "[]", r#"["x@y.z"]"#, me));
    assert!(harness::audit_recipients_ok(r#"["me@example.com"]"#, r#"["  "]"#, "[]", me));
    // Malformed JSON never passes.
    assert!(!harness::audit_recipients_ok("not-json", "[]", "[]", me));
}

#[test]
fn live_gate_21_safety_audit_subject_bcc_mid_rules() {
    // Rule 2: sandbox prefix on every subject.
    assert!(harness::audit_subject_prefixed("SGE-TEST-123-arrival"));
    assert!(!harness::audit_subject_prefixed("hello world"));
    assert!(!harness::audit_subject_prefixed("sge-test-lowercase"));
    // Rule 3: Bcc: header line detected; body mentions and folded
    // continuations are not headers.
    assert!(harness::eml_has_bcc_header(b"From: a@x\r\nBcc: hidden@y\r\nSubject: SGE-TEST-1\r\n\r\nbody"));
    assert!(harness::eml_has_bcc_header(b"from: a@x\nbcc: hidden@y\n\nbody"));
    assert!(!harness::eml_has_bcc_header(b"From: a@x\r\nSubject: SGE-TEST-1\r\n\r\nwrite Bcc: in the body freely"));
    assert!(!harness::eml_has_bcc_header(b"From: a@x\r\nSubject: Re: Bcc discussion\r\n\r\nbody"));
    assert!(!harness::eml_has_bcc_header(b"From: a@x\r\n Subject: folded bcc: not-a-header\r\n\r\nbody"));
    assert!(!harness::eml_has_bcc_header(b"From: a@x\r\nSubject: SGE-TEST-1\r\n"));
    // Rule 4: duplicate Message-IDs detected.
    assert_eq!(harness::audit_first_duplicate_mid(&["<a@x>", "<b@x>"]), None);
    assert_eq!(
        harness::audit_first_duplicate_mid(&["<a@x>", "<b@x>", "<a@x>"]),
        Some("<a@x>".to_string())
    );
}

#[test]
fn live_gate_21_safety_scan_detects_planted_credential_substring() {
    // Secrets carry regex metachars — matching is fixed-substring (contains).
    let secrets = ["p@$$w0rd[.*]+foo", "second-secret"];
    assert!(harness::scan_text_has_secret("log line p@$$w0rd[.*]+foo leaked", &secrets));
    assert!(harness::scan_text_has_secret("has second-secret inside", &secrets));
    assert!(!harness::scan_text_has_secret("clean log with RCPT=<me@example.com> only", &secrets));
    // Partial passwords do NOT match (full-value scan, no prefix guessing).
    assert!(!harness::scan_text_has_secret("log line p@$$w0rd leaked", &secrets));
    // Empty secrets never match (unset env must not fail the scan).
    assert!(!harness::scan_text_has_secret("anything at all", &["", "   ".trim()]));
    assert!(!harness::scan_text_has_secret("anything at all", &[]));
}

#[test]
fn live_gate_21_safety_dump_covers_all_temp_tables_for_scan() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let app_data = test_app_data("safety-dump");
    let mb = queries::ensure_mailbox(conn, "INBOX").unwrap();
    let mid = sge_lib::drafts::new_message_id("safety-dump");
    queries::upsert_draft(conn, "draft-dump", mb, &mid, "SGE-TEST-dump", "probe", "me@example.com", "", "").unwrap();
    let out = send_queue::enqueue_send(conn, &app_data, self_input("dump")).unwrap();
    let (tables, rows, dump) = harness::dump_store_for_scan(conn);
    assert!(tables.contains(&"send_queue".to_string()), "dump must cover send_queue: {:?}", tables);
    assert!(tables.contains(&"drafts".to_string()), "dump must cover drafts: {:?}", tables);
    assert!(rows > 0, "dump must carry rows");
    assert!(dump.contains(&out.message_id), "dump must carry queue content");
    assert!(dump.contains(&mid), "dump must carry drafts content");
    // The dump text is what the password scan runs over.
    assert!(!harness::scan_text_has_secret(&dump, &["definitely-not-present-secret"]));
    assert!(harness::scan_text_has_secret(
        &format!("{}\nplanted-secret-xyz", dump),
        &["planted-secret-xyz"]
    ));
    std::fs::remove_dir_all(&app_data).ok();
}
