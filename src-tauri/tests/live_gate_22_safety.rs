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
    /// uid -> seen flag (models server-side \Seen for the Seen doubles)
    seen: HashMap<u32, bool>,
    append_calls: Vec<(String, String, Vec<u8>)>,
    search_calls: Vec<(String, String)>,
    seen_calls: Vec<(u32, bool)>,
    moved_calls: Vec<(String, String)>,
}

impl FakeSession {
    fn new() -> Self {
        Self {
            mailboxes: HashSet::new(),
            selected: None,
            next_uid: 1,
            messages: HashMap::new(),
            seen: HashMap::new(),
            append_calls: Vec::new(),
            search_calls: Vec::new(),
            seen_calls: Vec::new(),
            moved_calls: Vec::new(),
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
        let mut names: Vec<&str> = Vec::new();
        if deleted {
            names.push("\\Deleted");
        }
        if self.seen.get(&uid).copied().unwrap_or(false) {
            names.push("\\Seen");
        }
        let flags = format!(
            "[{}]",
            names
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
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
    /// Model a server-side expunge (the copy is gone for the next SEARCH).
    fn remove_message(&mut self, uid: u32) {
        self.messages.remove(&uid);
        self.seen.remove(&uid);
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

    fn set_seen(&mut self, uid: u32, seen: bool) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            if self.messages.contains_key(&uid) {
                self.seen_calls.push((uid, seen));
                self.seen.insert(uid, seen);
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
        uid_set: &str,
        dest: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        let (uid_set, dest) = (uid_set.to_string(), dest.to_string());
        Box::pin(async move {
            self.moved_calls.push((uid_set, dest));
            Ok(())
        })
    }

    fn capabilities(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<String>, SyncError>> {
        Box::pin(async move { Ok(vec!["IMAP4rev1".to_string(), "UIDPLUS".to_string(), "MOVE".to_string()]) })
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

// ── 22-02 expansion doubles, grouped so the plan's verify filters match ──
// verbatim (`live_gate_22::replay`, `live_gate_22::resurrection`,
// `live_gate_22::seen`, `live_gate_22::safety`, `live_gate_22::cleanup`).
mod live_gate_22 {
    pub mod replay {
        use super::super::{block_on, harness, FakeSession};
        use sge_lib::store::{queries, Store};
        use sge_lib::sync::worker::SyncWorker;
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};

        /// Shared seed: one store row + hidden flag + queued delete op.
        /// Returns the store and mailbox id; asserts pending-visible.
        pub(super) fn replay_store(uid: u32) -> (Arc<Mutex<Store>>, u64) {
            let store = Arc::new(Mutex::new(Store::open_in_memory().expect("mem store")));
            let mb_id = {
                let guard = store.lock().unwrap();
                let id = queries::ensure_mailbox(guard.conn(), "SGE-Test-replay-1").unwrap();
                queries::upsert_message(
                    guard.conn(),
                    id,
                    uid,
                    Some("<replay-double@x>"),
                    "SGE-TEST-replay",
                    "me@example.com",
                    "me@example.com",
                    "",
                    "2026-10-13T12:00:00Z",
                    "[]",
                    false,
                    "replay",
                )
                .unwrap();
                queries::set_pending_delete(guard.conn(), id, uid, true).unwrap();
                queries::enqueue_imap_outbox(
                    guard.conn(),
                    id,
                    uid,
                    queries::IMAP_OP_DELETE,
                    Some("Trash"),
                    4242,
                )
                .unwrap();
                assert!(
                    harness::pending_depth(guard.conn(), id) > 0,
                    "pending indicator visible until acked"
                );
                id
            };
            (store, mb_id)
        }

// ── Replay-order + pending-until-acked (D-10, T-22-03, zero network) ───────

#[test]
fn live_gate_22_replay_order_predicate() {
    let ordered = [
        "[SGE sync] SELECT SGE-Test-1: exists=1",
        "[SGE sync] imap replay: acked=1 dropped=0 failed=0 moved=1",
        "[SGE sync] FETCH 1:1: 1 headers",
        "sweep done",
    ];
    assert!(harness::replay_logged_before_sweep(&ordered));
    let reversed = [
        "sweep done",
        "[SGE sync] imap replay: acked=1 dropped=0 failed=0 moved=1",
    ];
    assert!(!harness::replay_logged_before_sweep(&reversed));
    let no_replay = ["[SGE sync] SELECT x", "sweep done"];
    assert!(!harness::replay_logged_before_sweep(&no_replay));
    let no_sweep = ["[SGE sync] imap replay: acked=0 dropped=0 failed=0 moved=0"];
    assert!(!harness::replay_logged_before_sweep(&no_sweep));
}

#[test]
fn live_gate_22_replay_drains_pending_depth_to_zero() {
    let (store, mb_id) = replay_store(1);
    let worker = SyncWorker::new(store.clone());
    let mut session = FakeSession::with_sandbox("SGE-Test-replay-1");
    let live: HashSet<u32> = [1].into_iter().collect();
    let summary = block_on(worker.replay_imap_outbox(
        &mut session,
        mb_id,
        "SGE-Test-replay-1",
        4242,
        Some(&live),
    ))
    .unwrap();
    assert_eq!(summary.acked, 1);
    assert_eq!(summary.moved, 1);
    assert_eq!(
        session.moved_calls,
        vec![("1".to_string(), "Trash".to_string())],
        "delete replays as a server-side move to the stored dest"
    );
    let guard = store.lock().unwrap();
    assert_eq!(
        harness::pending_depth(guard.conn(), mb_id),
        0,
        "queue depth returns to zero after the ack"
    );
    assert!(queries::pending_imap_uids(guard.conn(), mb_id).unwrap().is_empty());
}

    } // pub mod replay

    pub mod resurrection {
        use super::super::{block_on, FakeSession};
        use super::replay::replay_store;
        use sge_lib::imap::SyncSession;
        use sge_lib::store::queries;
        use sge_lib::sync::worker::SyncWorker;
        use std::collections::HashSet;

// ── Double-sync resurrection (D-11, T-22-03, zero network) ─────────────────

#[test]
fn live_gate_22_double_sync_uid_stays_gone() {
    let (store, mb_id) = replay_store(1);
    let worker = SyncWorker::new(store.clone());
    let mut session = FakeSession::with_sandbox("SGE-Test-replay-1");
    // Server holds the copy; the test bytes carry the same Message-ID the
    // store row tracks (FakeSession assigns uid 1 to its first APPEND).
    block_on(session.append_message(
        "SGE-Test-replay-1",
        "flags",
        b"Message-ID: <replay-double@x>\r\n\r\nb",
    ))
    .unwrap();

    // Pass 1: the queued delete acks (op acked, row hidden for the sweep).
    let live: HashSet<u32> = [1].into_iter().collect();
    let summary = block_on(worker.replay_imap_outbox(
        &mut session,
        mb_id,
        "SGE-Test-replay-1",
        4242,
        Some(&live),
    ))
    .unwrap();
    assert_eq!(summary.acked, 1);

    // The sweep's expunge-diff removes the local row (server no longer
    // holds the UID — tombstoned, never re-fetched).
    session.remove_message(1);
    {
        let guard = store.lock().unwrap();
        let gone = queries::expunge_uids_local(guard.conn(), mb_id, &[1]).unwrap();
        assert_eq!(gone, vec![1]);
    }

    // Pass 2: UID absent from SEARCH and no local row left to resurrect.
    assert!(!block_on(session.search_uids()).unwrap().contains(&1));
    let guard = store.lock().unwrap();
    assert!(!queries::all_local_uids(guard.conn(), mb_id).unwrap().contains(&1));
    assert!(!queries::is_pending_delete(guard.conn(), mb_id, 1).unwrap());
}

    } // pub mod resurrection

    pub mod seen {
        use super::super::{block_on, harness, FakeSession};
        use sge_lib::imap::SyncSession;
        use sge_lib::store::{queries, Store};
        use sge_lib::sync::worker::SyncWorker;
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};

// ── Seen both directions through queued replay (D-12, zero network) ────────

#[test]
fn live_gate_22_seen_round_trips_both_directions_through_replay() {
    let store = Arc::new(Mutex::new(Store::open_in_memory().expect("mem store")));
    let mb_id = {
        let guard = store.lock().unwrap();
        queries::ensure_mailbox(guard.conn(), "SGE-Test-seen-1").unwrap()
    };
    let worker = SyncWorker::new(store.clone());
    let mut session = FakeSession::with_sandbox("SGE-Test-seen-1");
    block_on(session.append_message(
        "SGE-Test-seen-1",
        "flags",
        b"Message-ID: <seen-double@x>\r\n\r\nb",
    ))
    .unwrap();

    for seen in [true, false] {
        {
            let guard = store.lock().unwrap();
            queries::enqueue_outbox(guard.conn(), mb_id, 1, seen, 4242).unwrap();
            assert!(
                harness::pending_depth(guard.conn(), mb_id) > 0,
                "pending visible until acked (seen={seen})"
            );
        }
        let live: HashSet<u32> = [1].into_iter().collect();
        let summary =
            block_on(worker.replay_outbox(&mut session, mb_id, 4242, Some(&live))).unwrap();
        assert_eq!(summary.acked, 1, "seen={seen} must ack");
        let guard = store.lock().unwrap();
        assert_eq!(harness::pending_depth(guard.conn(), mb_id), 0);
        let flags = block_on(session.fetch_envelopes("1")).unwrap();
        let has_seen = flags.iter().any(|h| h.uid == 1 && h.flags.contains("\\Seen"));
        assert_eq!(has_seen, seen, "FLAGS must read seen={seen} after replay");
    }
    assert_eq!(session.seen_calls, vec![(1, true), (1, false)]);
}

#[test]
fn live_gate_22_seen_replay_honors_epoch_and_absent_drop_rules() {
    let store = Arc::new(Mutex::new(Store::open_in_memory().expect("mem store")));
    let mb_id = {
        let guard = store.lock().unwrap();
        queries::ensure_mailbox(guard.conn(), "SGE-Test-seen-2").unwrap()
    };
    let worker = SyncWorker::new(store.clone());
    let mut session = FakeSession::with_sandbox("SGE-Test-seen-2");

    // Epoch mismatch: whole-mailbox drop, zero wire writes.
    {
        let guard = store.lock().unwrap();
        queries::enqueue_outbox(guard.conn(), mb_id, 1, true, 4242).unwrap();
    }
    let live: HashSet<u32> = [1].into_iter().collect();
    let summary =
        block_on(worker.replay_outbox(&mut session, mb_id, 9999, Some(&live))).unwrap();
    assert_eq!(summary.dropped, 1);
    assert_eq!(summary.acked, 0);
    assert!(session.seen_calls.is_empty(), "stale-epoch UIDs never reach the wire");

    // Absent UID: single drop, the live op is untouched.
    {
        let guard = store.lock().unwrap();
        queries::enqueue_outbox(guard.conn(), mb_id, 77, true, 4242).unwrap();
        queries::enqueue_outbox(guard.conn(), mb_id, 1, true, 4242).unwrap();
    }
    let live: HashSet<u32> = [1].into_iter().collect();
    block_on(session.append_message(
        "SGE-Test-seen-2",
        "flags",
        b"Message-ID: <s@x>\r\n\r\nb",
    ))
    .unwrap();
    let summary =
        block_on(worker.replay_outbox(&mut session, mb_id, 4242, Some(&live))).unwrap();
    assert_eq!(summary.dropped, 1, "absent uid drops its single op");
    assert_eq!(summary.acked, 1, "live op still replays");
    assert_eq!(session.seen_calls, vec![(1, true)], "only the live UID is written");
}

    } // pub mod seen

    pub mod safety {
        use super::super::harness;

// ── Secret scan + prefix audit predicates (D-15/D-16, T-22-04) ─────────────

#[test]
fn live_gate_22_secret_scan_flags_canary_and_passes_clean() {
    // Password-shaped fixtures carry regex metachars — the scan is a fixed
    // substring match, never a regex, so they still hit literally.
    let secrets = ["hunter2$.*[x]", "smtp-secret", "user@example.com"];
    assert_eq!(
        harness::scan_text_for_secrets("log line hunter2$.*[x] here", &secrets),
        Some(0)
    );
    assert_eq!(
        harness::scan_text_for_secrets("smtp-secret leaked", &secrets),
        Some(1)
    );
    assert_eq!(
        harness::scan_text_for_secrets("clean log, no creds", &secrets),
        None
    );
    assert_eq!(harness::scan_text_for_secrets("anything", &[""]), None);
}

#[test]
fn live_gate_22_prefix_audit_rejects_unprefixed_and_foreign() {
    let me_ = "me@example.com";
    assert!(harness::audit_test_mail(&["SGE-TEST-a"], &[me_], me_).is_ok());
    assert!(harness::audit_test_mail(&["plain subject"], &[me_], me_).is_err());
    assert!(harness::audit_test_mail(&["SGE-TEST-a"], &["other@example.com"], me_).is_err());
    assert!(harness::audit_test_mail(&[], &[], me_).is_ok());
}

    } // pub mod safety

    pub mod cleanup {
        use super::super::harness;
        use sge_lib::store::{queries, Store};

// ── Cleanup discipline doubles (D-03, zero network) ────────────────────────

#[test]
fn live_gate_22_cleanup_backoff_is_three_retries() {
    assert_eq!(harness::cleanup_backoff_secs(), [5, 15, 45]);
}

#[test]
fn live_gate_22_sqlite_dump_covers_all_tables() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir();
    let db_path = dir.join(format!("sge-live22-test-{}-{tag}.sqlite", std::process::id()));
    let dump_path = dir.join(format!("sge-live22-test-{}-{tag}.dump", std::process::id()));
    {
        let store = Store::open(&db_path).expect("temp store");
        let guard = store;
        let conn = guard.conn();
        let mb_id = queries::ensure_mailbox(conn, "SGE-Test-dump-1").unwrap();
        queries::enqueue_outbox(conn, mb_id, 3, true, 4242).unwrap();
    }
    let bytes = harness::write_sqlite_dump_text(&db_path, &dump_path).expect("dump writes");
    assert!(bytes > 0);
    let text = std::fs::read_to_string(&dump_path).expect("dump readable");
    assert!(text.contains("-- table mailboxes"), "dump covers schema tables");
    assert!(text.contains("-- table flag_outbox"), "dump covers the Seen queue");
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(&dump_path);
}
    } // pub mod cleanup
} // mod live_gate_22
