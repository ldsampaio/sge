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
    messages: HashMap<u32, (String, Vec<u8>, bool)>,
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
                .map(|(_, bytes, _)| bytes.clone())
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
                    entry.2 = deleted;
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
                .filter(|(_, (_, _, d))| *d)
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
                    if matches!(self.messages.get(&uid), Some((_, _, true))) {
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
            let uid = self.next_uid;
            self.next_uid += 1;
            self.messages.insert(uid, (mailbox, bytes, false));
            Ok(())
        })
    }

    fn uid_search_header(
        &mut self,
        _field: &str,
        _value: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move {
            let mut uids: Vec<u32> = self.messages.keys().copied().collect();
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
        name: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<MailboxStatus, SyncError>> {
        let name = name.to_string();
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
