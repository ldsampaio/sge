//! Phase 22 offline safety doubles (`live_gate_22_safety`).
//!
//! Zero-network contract tests for the live-gate harness: they drive the
//! REAL harness helpers (prefix guard, max-wins, one-copy assert, offline
//! gate decorator — included straight from the bin, so there is exactly one
//! implementation) plus the REAL prod verbs (`save_draft_on_session`, the
//! draft codec) against an in-memory `FakeSession`. CI-green with no creds.

#[path = "../src/bin/live_gate_22.rs"]
#[allow(dead_code)]
mod harness;

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::AtomicBool,
    Arc,
};

use sge_lib::imap::manager::save_draft_on_session;
use sge_lib::imap::{
    MailboxInfo, MailboxStatus, MailboxSummary, SyncError, SyncSession,
};
use sge_lib::imap::headers::MessageHeader;

/// In-memory IMAP double: one mailbox map, UID assignment on APPEND, real
/// removal on scoped expunge. Models just enough server behavior for the
/// one-copy / reconcile / gate proofs.
struct FakeSession {
    mailboxes: HashSet<String>,
    selected: Option<String>,
    next_uid: u32,
    /// uid -> (mailbox, message_id, bytes, deleted)
    messages: HashMap<u32, (String, String, Vec<u8>, bool)>,
    append_calls: Vec<(String, String, Vec<u8>)>,
    search_calls: Vec<(String, String)>,
}

impl FakeSession {
    fn new() -> Self {
        Self {
            mailboxes: HashSet::new(),
            selected: None,
            next_uid: 1,
            messages: HashMap::new(),
            append_calls: Vec::new(),
            search_calls: Vec::new(),
        }
    }

    fn with_sandbox(sandbox: &str) -> Self {
        let mut s = Self::new();
        s.mailboxes.insert(sandbox.to_string());
        s.selected = Some(sandbox.to_string());
        s
    }

    fn live_uids(&self, mailbox: &str) -> Vec<u32> {
        let mut uids: Vec<u32> = self
            .messages
            .iter()
            .filter(|(_, (mb, _, _, _))| mb == mailbox)
            .map(|(u, _)| *u)
            .collect();
        uids.sort_unstable();
        uids
    }

    fn header_envelope(&self, uid: u32) -> MessageHeader {
        let (_, msg_id, _, deleted) = self.messages.get(&uid).cloned().unwrap_or_default();
        let flags = if deleted { r#"["\Deleted"]"#.to_string() } else { "[]".to_string() };
        MessageHeader {
            uid,
            message_id: Some(msg_id),
            subject: format!("SGE-TEST-subject-{uid}"),
            from_addr: "me@example.com".to_string(),
            to_addrs: "me@example.com".to_string(),
            cc_addrs: String::new(),
            date_utc: "2026-10-13T12:00:00Z".to_string(),
            flags,
            has_attachments: false,
            preview: String::new(),
        }
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
                exists: self.live_uids(self.selected.as_deref().unwrap_or("")).len() as u32,
                uid_next: Some(self.next_uid),
            })
        })
    }

    fn search_uids(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move {
            let mb = self.selected.clone().unwrap_or_default();
            Ok(self.live_uids(&mb))
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
                        out.push(self.header_envelope(uid));
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
        flags: &str,
        bytes: &[u8],
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        let mailbox = mailbox.to_string();
        let flags = flags.to_string();
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
            self.append_calls.push((mailbox.clone(), flags, bytes.clone()));
            self.messages.insert(uid, (mailbox, msg_id, bytes, false));
            Ok(())
        })
    }

    fn uid_search_header(
        &mut self,
        field: &str,
        value: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
        let field = field.to_string();
        let value = value.to_string();
        Box::pin(async move {
            self.search_calls.push((field, value.clone()));
            let mut hits: Vec<u32> = self
                .messages
                .iter()
                .filter(|(_, (_, id, _, _))| *id == value)
                .map(|(u, _)| *u)
                .collect();
            hits.sort_unstable();
            Ok(hits)
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
                messages: self.live_uids(&name).len() as u32,
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

// ── Pre-flight prefix guard (T-22-01, zero network) ───────────────────────

#[test]
fn live_gate_22_preflight_refuses_out_of_sandbox_targets() {
    for wire in [
        "INBOX",
        "Drafts",
        "Sent",
        "SGE-Test",
        "Archive/SGE-Test-1",
        "",
    ] {
        assert!(
            harness::assert_sandbox_prefix(wire).is_err(),
            "pre-flight must refuse {wire:?}"
        );
    }
    assert!(harness::assert_sandbox_prefix("SGE-Test-1699999999-1234").is_ok());
}

#[test]
fn live_gate_22_preflight_absent_sandbox_detection() {
    // Mirrors the bin's start guard: a sandbox already in LIST means a prior
    // failed cleanup — abort with manual steps, never reuse.
    let sandbox = "SGE-Test-1699999999-1234";
    let before = vec!["INBOX".to_string(), sandbox.to_string()];
    assert!(
        before.iter().any(|n| n == sandbox),
        "fixture must model the present-sandbox abort case"
    );
    assert!(harness::unexpected_list_delta(&[], &before, sandbox)
        .iter()
        .all(|n| n != sandbox));
}

// ── Offline gate decorator (D-09, zero network) ──────────────────────────

#[test]
fn live_gate_22_gate_closed_fails_writes_forwards_reads() {
    let sandbox = "SGE-Test-gate-1";
    let mut inner = FakeSession::with_sandbox(sandbox);
    let flag = Arc::new(AtomicBool::new(true));
    let mut gate = harness::GateSession::new(&mut inner, flag.clone());
    gate.set_closed(true);
    assert!(gate.is_closed());

    // Writes fail while closed …
    assert!(block_on(gate.append_message(sandbox, "flags", b"x")).is_err());
    assert!(block_on(gate.set_seen(1, true)).is_err());
    assert!(block_on(gate.store_deleted(1, true)).is_err());
    assert!(block_on(gate.uid_expunge("1")).is_err());
    assert!(block_on(gate.uid_copy_to("1", sandbox)).is_err());
    assert!(block_on(gate.uid_move_to("1", sandbox)).is_err());
    assert!(block_on(gate.create_mailbox("SGE-Test-new")).is_err());
    assert!(block_on(gate.delete_mailbox(sandbox)).is_err());

    // … while reads forward untouched.
    assert!(block_on(gate.search_uids()).unwrap().is_empty());
    assert!(block_on(gate.capabilities()).unwrap().contains(&"UIDPLUS".to_string()));
    assert!(block_on(gate.list_mailboxes()).unwrap().iter().any(|m| m.name == sandbox));

    // Reopen: writes forward to the inner session.
    gate.set_closed(false);
    assert!(!gate.is_closed());
    assert!(block_on(gate.append_message(sandbox, "flags", b"Message-ID: <a@x>\r\n\r\nb")).is_ok());
    assert_eq!(block_on(gate.search_uids()).unwrap(), vec![1]);
    assert_eq!(
        block_on(gate.uid_search_header("Message-ID", "<a@x>")).unwrap(),
        vec![1]
    );
}

// ── One-copy + reconcile over the REAL prod verb (zero network) ──────────
// (Extended in Task 2: count+identity across two saves + 3-copy collapse.)

#[test]
fn live_gate_22_one_copy_first_save_reconciles() {
    let sandbox = "SGE-Test-one-copy-1";
    let mut session = FakeSession::with_sandbox(sandbox);
    let msg_id = harness::run_message_id("safety-first-save");
    let bytes = harness::render_test_draft(
        "me@example.com",
        "me@example.com",
        "first-save",
        "v1 body",
        &msg_id,
        "Tue, 13 Oct 2026 12:00:00 +0000",
    );
    let caps = vec!["IMAP4rev1".to_string(), "UIDPLUS".to_string()];
    let uid_a = block_on(save_draft_on_session(&mut session, sandbox, &msg_id, &bytes, None, &caps))
        .expect("first save must reconcile");
    let hits = block_on(session.uid_search_header("Message-ID", &msg_id)).unwrap();
    harness::assert_one_copy(&hits, uid_a).expect("exactly [uid_a]");
}
