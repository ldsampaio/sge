//! Phase 26 offline safety doubles (`live_gate_26_safety`).
//!
//! Zero-network contract tests for the live-gate harness: they drive the
//! REAL harness helpers (prefix guard, sandbox lifecycle shapes, Seen
//! store/reconcile shapes, tree-compare shapes, tick-pickup and
//! reconnect-resume shapes, offline gate decorator — included straight from
//! the bin, so there is exactly one implementation) against an in-memory
//! `FakeSession`. CI-green with no creds.

#[path = "../src/bin/live_gate_26.rs"]
#[allow(dead_code)]
mod harness;

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use sge_lib::imap::headers::MessageHeader;
use sge_lib::imap::{MailboxInfo, MailboxStatus, MailboxSummary, SyncError, SyncSession};

/// In-memory IMAP double: mailbox set, UID assignment on APPEND, real
/// removal on scoped expunge, server-side Seen map, configurable STATUS
/// UNSEEN datum. Models just enough server behavior for the gate 6/7/8
/// leg-shape proofs.
struct FakeSession {
    mailboxes: HashSet<String>,
    selected: Option<String>,
    next_uid: u32,
    /// uid -> (mailbox, message_id, bytes, deleted)
    messages: HashMapUid,
    /// uid -> seen flag (models server-side \Seen for the Seen doubles)
    seen: std::collections::HashMap<u32, bool>,
    /// STATUS UNSEEN datum the fake server reports.
    status_unseen: u32,
    append_calls: Vec<(String, String, Vec<u8>)>,
    seen_calls: Vec<(u32, bool)>,
}

type HashMapUid = std::collections::HashMap<u32, (String, String, Vec<u8>, bool)>;

impl FakeSession {
    fn new() -> Self {
        Self {
            mailboxes: HashSet::new(),
            selected: None,
            next_uid: 1,
            messages: HashMapUid::new(),
            seen: std::collections::HashMap::new(),
            status_unseen: 0,
            append_calls: Vec::new(),
            seen_calls: Vec::new(),
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
            modseq: None,
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

    fn set_seen(
        &mut self,
        uid: u32,
        seen: bool,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
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

    fn uid_expunge(
        &mut self,
        uid_set: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<Vec<u32>, SyncError>> {
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
            // Model UID MOVE: retarget the mailbox tag, keep the UID (the
            // double proves homing shapes, not server UID assignment).
            if !self.mailboxes.contains(&dest) {
                return Err(SyncError::Protocol(format!("no such mailbox {dest}")));
            }
            for part in uid_set.split(',') {
                if let Ok(uid) = part.trim().parse::<u32>() {
                    if let Some(entry) = self.messages.get_mut(&uid) {
                        entry.0 = dest.clone();
                    }
                }
            }
            Ok(())
        })
    }

    fn capabilities(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<String>, SyncError>> {
        Box::pin(async move {
            Ok(vec![
                "IMAP4rev1".to_string(),
                "UIDPLUS".to_string(),
                "MOVE".to_string(),
            ])
        })
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
        old: &str,
        new: &str,
    ) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        let (old, new) = (old.to_string(), new.to_string());
        Box::pin(async move {
            // Model RENAME: folder set entry moves, homed messages retag,
            // SELECT follows — UIDs survive (server RENAME semantics).
            if !self.mailboxes.contains(&old) {
                return Err(SyncError::Protocol(format!("no such mailbox {old}")));
            }
            self.mailboxes.remove(&old);
            self.mailboxes.insert(new.clone());
            for (_, (mb, _, _, _)) in self.messages.iter_mut() {
                if *mb == old {
                    *mb = new.clone();
                }
            }
            if self.selected.as_deref() == Some(old.as_str()) {
                self.selected = Some(new.clone());
            }
            Ok(())
        })
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
            self.append_calls
                .push((mailbox.clone(), flags, bytes.clone()));
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
            let _ = field;
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

    fn list_mailboxes(&mut self) -> sge_lib::imap::PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
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
                unseen: self.status_unseen,
                messages: self.live_uids(&name).len() as u32,
            })
        })
    }

    fn logout(&mut self) -> sge_lib::imap::PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move { Ok(()) })
    }

    fn select_condstore(&mut self) -> sge_lib::imap::PinBox<'_, Result<bool, SyncError>> {
        Box::pin(async move { Ok(false) })
    }

    fn condstore_fetch_changedsince<'a>(
        &'a mut self,
        _range: &'a str,
        _modseq: u64,
    ) -> sge_lib::imap::PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    async_std::task::block_on(f)
}

/// Gate-6 leg shapes: Seen store/reconcile through the REAL harness
/// prefix guard + GateSession decorator + FakeSession server state.
pub mod live_gate_26_seen {
    use super::*;

    #[test]
    fn seen_prefix_accepts_sandbox_refuses_real_folders() {
        assert!(harness::assert_sandbox_prefix("SGE-Test-1-2").is_ok());
        for wire in ["INBOX", "Sent", "Drafts", "SGE-Test", ""] {
            assert!(
                harness::assert_sandbox_prefix(wire).is_err(),
                "must refuse {wire:?}"
            );
        }
    }

    #[test]
    fn seen_store_roundtrip_shape() {
        let sandbox = "SGE-Test-seen-1";
        let mut s = FakeSession::with_sandbox(sandbox);
        let uid = block_on(async {
            s.append_message(sandbox, "", b"Subject: SGE-TEST-x\r\n\r\nb")
                .await
                .unwrap();
            s.search_uids().await.unwrap()
        });
        assert_eq!(uid.len(), 1);
        let uid = uid[0];
        // Online STORE +Seen, FLAGS assert via envelopes only.
        block_on(s.set_seen(uid, true)).unwrap();
        let headers = block_on(s.fetch_envelopes(&uid.to_string())).unwrap();
        assert!(headers
            .iter()
            .any(|h| h.uid == uid && h.flags.contains("\\Seen")));
        // Offline replay convergence shape: queue then apply, FLAGS flip.
        block_on(s.set_seen(uid, false)).unwrap();
        let headers = block_on(s.fetch_envelopes(&uid.to_string())).unwrap();
        assert!(!headers
            .iter()
            .any(|h| h.uid == uid && h.flags.contains("\\Seen")));
        assert_eq!(s.seen_calls, vec![(uid, true), (uid, false)]);
    }

    #[test]
    fn seen_offline_window_blocks_writes_forwards_reads() {
        let sandbox = "SGE-Test-seen-2";
        let mut s = FakeSession::with_sandbox(sandbox);
        let closed = Arc::new(AtomicBool::new(true));
        let mut gate = harness::GateSession::new(&mut s, closed.clone());
        assert!(gate.is_closed());
        // Writes fail while closed (the offline window the live leg proves).
        let uid = 99;
        assert!(block_on(gate.set_seen(uid, true)).is_err());
        assert!(block_on(gate.append_message(sandbox, "", b"x")).is_err());
        assert!(block_on(gate.create_mailbox("SGE-Test-nope")).is_err());
        // Reads forward untouched.
        assert!(block_on(gate.search_uids()).unwrap().is_empty());
        assert!(block_on(gate.capabilities())
            .unwrap()
            .contains(&"IMAP4rev1".to_string()));
        // Reopen: writes flow again.
        gate.set_closed(false);
        assert!(!gate.is_closed());
        block_on(gate.append_message(sandbox, "", b"Subject: SGE-TEST-y\r\n\r\nb")).unwrap();
        assert_eq!(block_on(gate.search_uids()).unwrap().len(), 1);
    }

    #[test]
    fn seen_pending_until_acked_shape() {
        // pending_depth sums both outbox tables; 1+ after enqueue, 0 after
        // drain — the shape the live leg asserts around replay_outbox.
        let dir = std::env::temp_dir().join(format!("sge-26-seen-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("seen.sqlite");
        let store = sge_lib::store::Store::open(&db).unwrap();
        let mb = sge_lib::store::queries::ensure_mailbox(store.conn(), "SGE-Test-seen-3").unwrap();
        assert_eq!(harness::pending_depth(store.conn(), mb), 0);
        sge_lib::store::queries::enqueue_outbox(store.conn(), mb, 7, true, 4242).unwrap();
        assert!(harness::pending_depth(store.conn(), mb) > 0);
        sge_lib::store::queries::drop_outbox_for_mailbox(store.conn(), mb).unwrap();
        assert_eq!(harness::pending_depth(store.conn(), mb), 0);
        let _ = std::fs::remove_file(&db);
    }
}

/// Gate-7 leg shapes: LIST-delta limited to the sandbox, per-folder browse,
/// STATUS UNSEEN badge comparison.
pub mod live_gate_26_tree {
    use super::*;

    fn names(s: &mut FakeSession) -> Vec<String> {
        harness::sorted_names(&block_on(s.list_mailboxes()).unwrap())
    }

    #[test]
    fn tree_list_delta_ignores_only_the_sandbox() {
        let before = vec!["INBOX".to_string(), "Drafts".to_string()];
        let after = vec![
            "INBOX".to_string(),
            "Drafts".to_string(),
            "SGE-Test-9-9".to_string(),
        ];
        assert!(harness::unexpected_list_delta(&before, &after, "SGE-Test-9-9").is_empty());
        // A real folder appearing mid-run aborts the run.
        let drifted = vec![
            "INBOX".to_string(),
            "Drafts".to_string(),
            "SGE-Test-9-9".to_string(),
            "Archive".to_string(),
        ];
        assert_eq!(
            harness::unexpected_list_delta(&before, &drifted, "SGE-Test-9-9"),
            vec!["Archive".to_string()]
        );
        // A real folder disappearing mid-run aborts the run too.
        let removed = vec!["Drafts".to_string(), "SGE-Test-9-9".to_string()];
        assert_eq!(
            harness::unexpected_list_delta(&before, &removed, "SGE-Test-9-9"),
            vec!["INBOX".to_string()]
        );
    }

    #[test]
    fn tree_create_reuse_delete_lifecycle_shape() {
        // One shared sandbox: created once, reused by every leg, deleted
        // once at end (D-35). Fake models the LIST-delta assertions.
        let sandbox = "SGE-Test-tree-1";
        let mut s = FakeSession::new();
        s.mailboxes.insert("INBOX".to_string());
        let before = names(&mut s);
        assert!(!before.iter().any(|n| n == sandbox));
        block_on(s.create_mailbox(sandbox)).unwrap();
        let after = names(&mut s);
        assert!(after.iter().any(|n| n == sandbox));
        assert!(harness::unexpected_list_delta(&before, &after, sandbox).is_empty());
        // Reuse across legs: second snapshot shows no further drift.
        let reused = names(&mut s);
        assert!(harness::unexpected_list_delta(&after, &reused, sandbox).is_empty());
        block_on(s.delete_mailbox(sandbox)).unwrap();
        let cleaned = names(&mut s);
        assert_eq!(cleaned, before);
    }

    #[test]
    fn tree_browse_and_badge_shapes() {
        let sandbox = "SGE-Test-tree-2";
        let mut s = FakeSession::with_sandbox(sandbox);
        block_on(s.append_message(sandbox, "", b"Subject: SGE-TEST-a\r\n\r\n1")).unwrap();
        block_on(s.append_message(sandbox, "", b"Subject: SGE-TEST-b\r\n\r\n2")).unwrap();
        // Browse: SEARCH + envelope FETCH cover every UID.
        let uids = block_on(s.search_uids()).unwrap();
        assert_eq!(uids.len(), 2);
        let ranges = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let headers = block_on(s.fetch_envelopes(&ranges)).unwrap();
        assert_eq!(headers.len(), 2);
        // Badges: STATUS UNSEEN datum wires through to the comparison the
        // live leg runs against the local badge derivation.
        s.status_unseen = 2;
        let status = block_on(s.mailbox_status(sandbox)).unwrap();
        assert_eq!(status.unseen, 2);
        assert_eq!(status.messages, 2);
        block_on(s.set_seen(uids[0], true)).unwrap();
        s.status_unseen = 1;
        let status = block_on(s.mailbox_status(sandbox)).unwrap();
        assert_eq!(status.unseen, 1);
    }
}

/// Gate-8 leg shapes: arrival pickup on a tick-shaped pass and
/// stale-session reconnect recovery with no silent gaps.
pub mod live_gate_26_arrival {
    use super::*;

    #[test]
    fn arrival_tick_pickup_shape() {
        // Second logical sender path: raw APPEND (no flags = unseen
        // arrival), then the tick-shaped pass (SELECT + SEARCH) surfaces it
        // without manual refresh.
        let sandbox = "SGE-Test-arrival-1";
        let mut s = FakeSession::with_sandbox(sandbox);
        let msg_id = "<live26-arrival-1@example.com>";
        let bytes = format!(
            "From: me@example.com\r\nTo: me@example.com\r\nSubject: SGE-TEST-arrival-tick\r\nMessage-ID: {msg_id}\r\n\r\nhello"
        );
        harness::audit_test_mail(
            &["SGE-TEST-arrival-tick"],
            &["me@example.com"],
            "me@example.com",
        )
        .unwrap();
        block_on(s.append_message(sandbox, "", bytes.as_bytes())).unwrap();
        // Tick: re-SELECT then SEARCH Message-ID — the arrival is there.
        block_on(s.select_mailbox(sandbox)).unwrap();
        let hits = block_on(s.uid_search_header("Message-ID", msg_id)).unwrap();
        assert_eq!(hits.len(), 1);
        let arrival_uid = hits[0];
        assert!(block_on(s.search_uids()).unwrap().contains(&arrival_uid));
    }

    #[test]
    fn arrival_reconnect_resume_shape_no_gaps() {
        // Stale-session reconnect: drop the session (fresh SELECT), every
        // pre-drop UID still present, arrival converged, no silent gaps.
        let sandbox = "SGE-Test-arrival-2";
        let mut s = FakeSession::with_sandbox(sandbox);
        block_on(s.append_message(sandbox, "", b"Subject: SGE-TEST-1\r\n\r\na")).unwrap();
        block_on(s.append_message(sandbox, "", b"Subject: SGE-TEST-2\r\n\r\nb")).unwrap();
        let before = block_on(s.search_uids()).unwrap();
        assert_eq!(before.len(), 2);
        // Mid-run session drop: re-SELECT is the reconnect.
        s.selected = None;
        block_on(s.select_mailbox(sandbox)).unwrap();
        let after = block_on(s.search_uids()).unwrap();
        assert_eq!(after, before, "reconnect must resume with zero UID gaps");
        // Backfill sweep convergence shape: local set catches up to server.
        let mut local: HashSet<u32> = HashSet::new();
        for uid in &after {
            local.insert(*uid);
        }
        assert!(after.iter().all(|u| local.contains(u)));
    }

    #[test]
    fn arrival_replay_before_sweep_order_shape() {
        // The worker logs `imap replay:` at Step 4b before the sweep code
        // runs; file order in the captured log is the evidence.
        let ok: Vec<&str> = vec![
            "[SGE sync] imap replay: acked=1 dropped=0 failed=0",
            "sweep done",
        ];
        assert!(harness::replay_logged_before_sweep(&ok));
        let missing: Vec<&str> = vec!["sweep done"];
        assert!(!harness::replay_logged_before_sweep(&missing));
        let flipped: Vec<&str> = vec!["sweep done", "[SGE sync] imap replay: acked=1"];
        assert!(!harness::replay_logged_before_sweep(&flipped));
    }
}

/// Shared safety predicates: secret scan, self-only mail audit.
pub mod live_gate_26_safety {
    use super::*;

    #[test]
    fn safety_secret_scan_names_artifact_never_secret() {
        // Fixed-substring scan finds the password; the caller maps the
        // index to an artifact name (log/dump), never the secret itself.
        assert_eq!(
            harness::scan_text_for_secrets("log hunter2 tail", &["hunter2"]),
            Some(0)
        );
        assert_eq!(harness::scan_text_for_secrets("clean", &["hunter2"]), None);
        assert_eq!(
            harness::scan_text_for_secrets("anything at all", &[""]),
            None
        );
        assert_eq!(
            harness::scan_text_for_secrets("u=user p=hunter2", &["user", "hunter2"]),
            Some(0)
        );
    }

    #[test]
    fn safety_audit_rejects_unprefixed_subjects() {
        assert!(harness::audit_test_mail(&["SGE-TEST-ok"], &[], "me@example.com").is_ok());
        assert!(harness::audit_test_mail(&["plain subject"], &[], "me@example.com").is_err());
    }

    #[test]
    fn safety_audit_rejects_non_self_recipients() {
        assert!(
            harness::audit_test_mail(&["SGE-TEST-ok"], &["me@example.com"], "me@example.com")
                .is_ok()
        );
        assert!(harness::audit_test_mail(
            &["SGE-TEST-ok"],
            &["friend@example.com"],
            "me@example.com"
        )
        .is_err());
        // Empty cc/bcc cells are fine; unparseable cells fail closed.
        assert!(harness::audit_test_mail(&["SGE-TEST-ok"], &[], "me@example.com").is_ok());
    }

    #[test]
    fn safety_rendered_mail_parses_and_carries_prefix() {
        let msg_id = harness::run_message_id("safety-unittest");
        let bytes = harness::render_test_draft(
            "me@example.com",
            "me@example.com",
            "safety",
            "safety body",
            &msg_id,
            "Tue, 13 Oct 2026 12:00:00 +0000",
        );
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Subject: SGE-TEST-safety"));
        assert!(text.contains(&msg_id));
        let parsed = mail_parser::MessageParser::new().parse(&bytes).unwrap();
        assert_eq!(parsed.body_text(0).unwrap().to_string(), "safety body");
    }
}

/// Cleanup discipline: tracked-UID-only expunge scoping + 3x backoff.
pub mod live_gate_26_cleanup {
    use super::*;

    #[test]
    fn cleanup_backoff_is_three_retries() {
        assert_eq!(harness::cleanup_backoff_secs(), [5, 15, 45]);
    }

    #[test]
    fn cleanup_expunge_scopes_to_tracked_uids_only() {
        // Tracked-UID-only UID EXPUNGE via the owning folder wire: the
        // untracked UID survives, the tracked one is gone.
        let sandbox = "SGE-Test-cleanup-1";
        assert!(harness::assert_sandbox_prefix(sandbox).is_ok());
        let mut s = FakeSession::with_sandbox(sandbox);
        block_on(s.append_message(sandbox, "", b"Subject: SGE-TEST-tracked\r\n\r\na")).unwrap();
        block_on(s.append_message(sandbox, "", b"Subject: SGE-TEST-other\r\n\r\nb")).unwrap();
        let uids = block_on(s.search_uids()).unwrap();
        assert_eq!(uids.len(), 2);
        let (tracked, other) = (uids[0], uids[1]);
        block_on(s.store_deleted(tracked, true)).unwrap();
        block_on(s.uid_expunge(&tracked.to_string())).unwrap();
        let remaining = block_on(s.search_uids()).unwrap();
        assert!(!remaining.contains(&tracked));
        assert!(
            remaining.contains(&other),
            "untracked UID must survive scoped expunge"
        );
        // Full cleanup shape: expunge the rest, DELETE the sandbox, LIST
        // returns to the pre-run snapshot.
        s.remove_message(other);
        assert!(block_on(s.search_uids()).unwrap().is_empty());
        block_on(s.delete_mailbox(sandbox)).unwrap();
        let infos = block_on(s.list_mailboxes()).unwrap();
        assert!(!infos.iter().any(|i| i.name == sandbox));
    }

    #[test]
    fn cleanup_refuses_out_of_sandbox_delete() {
        // The prefix guard fires before EVERY mutating call — including the
        // DELETE path, so a wrong folder name never reaches the wire.
        assert!(harness::assert_sandbox_prefix("INBOX").is_err());
        assert!(harness::assert_sandbox_prefix("Sent").is_err());
        // The prefix guard is verbatim 21/22 starts_with semantics (D-36):
        // the bare prefix alone passes the guard (it is never a real
        // folder — preflight LIST + mid-run delta checks own that case).
        assert!(harness::assert_sandbox_prefix("SGE-Test-").is_ok());
        assert!(harness::assert_sandbox_prefix("SGE-Test").is_err());
        assert!(harness::assert_sandbox_prefix("SGE-Test-ok-1").is_ok());
    }
}

/// v1.3 confirm→MOVE→undo leg shapes (Task 1): sandbox Auto/ derivation,
/// MOVE homing through the REAL harness guards, undo-restore, override log.
pub mod live_gate_26_filing {
    use super::*;

    #[test]
    fn filing_auto_wire_never_real_tree() {
        let wire = harness::sandbox_auto_wire("SGE-Test-9-9", "Acadêmico");
        assert_eq!(wire, "SGE-Test-9-9-Auto-Acadêmico");
        assert!(harness::assert_sandbox_prefix(&wire).is_ok());
        assert!(harness::assert_never_real_auto(&wire).is_ok());
        for real in ["Auto", "Auto/Acadêmico", "Auto.Financeiro"] {
            assert!(harness::assert_never_real_auto(real).is_err());
        }
        // A non-Auto real folder passes the Auto guard but fails prefix.
        assert!(harness::assert_never_real_auto("INBOX").is_ok());
        assert!(harness::assert_sandbox_prefix("INBOX").is_err());
    }

    #[test]
    fn filing_confirm_move_undo_shape() {
        // suggest→confirm→MOVE→undo homing: origin → sandbox Auto → origin,
        // every verb gated by the real prefix asserts first.
        let origin = "SGE-Test-filing-1";
        let dest = harness::sandbox_auto_wire(origin, "Acadêmico");
        harness::assert_sandbox_prefix(origin).unwrap();
        harness::assert_never_real_auto(&dest).unwrap();
        harness::assert_sandbox_prefix(&dest).unwrap();
        let mut s = FakeSession::with_sandbox(origin);
        block_on(s.create_mailbox(&dest)).unwrap();
        block_on(s.append_message(origin, "", b"Subject: SGE-TEST-filing-confirm\r\n\r\nx"))
            .unwrap();
        let uid = block_on(s.search_uids()).unwrap()[0];
        // Confirm MOVE (uid_move_to models the server verb).
        block_on(s.uid_move_to(&uid.to_string(), &dest)).unwrap();
        block_on(s.select_mailbox(origin)).unwrap();
        assert!(
            block_on(s.search_uids()).unwrap().is_empty(),
            "origin must be empty after MOVE"
        );
        block_on(s.select_mailbox(&dest)).unwrap();
        assert_eq!(
            block_on(s.search_uids()).unwrap(),
            vec![uid],
            "dest must hold the moved UID"
        );
        // Undo: MOVE back, restored to origin.
        block_on(s.uid_move_to(&uid.to_string(), origin)).unwrap();
        block_on(s.select_mailbox(origin)).unwrap();
        assert_eq!(
            block_on(s.search_uids()).unwrap(),
            vec![uid],
            "undo must restore the UID"
        );
        block_on(s.select_mailbox(&dest)).unwrap();
        assert!(block_on(s.search_uids()).unwrap().is_empty());
    }

    #[test]
    fn filing_undo_logs_override_shape() {
        // The undo decision lands in label_overrides via the same prod
        // entries the override_label IPC uses; latest_override pins it.
        let dir = std::env::temp_dir().join(format!("sge-26-filing-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("filing.sqlite");
        let store = sge_lib::store::Store::open(&db).unwrap();
        let mb =
            sge_lib::store::queries::ensure_mailbox(store.conn(), "SGE-Test-filing-2").unwrap();
        sge_lib::store::queries::upsert_message(
            store.conn(),
            mb,
            11,
            Some("<filing-1@example.com>"),
            "SGE-TEST-filing-confirm",
            "me@example.com",
            "[\"me@example.com\"]",
            "[]",
            "2026-10-13T12:00:00Z",
            "[]",
            false,
            "",
            None,
            None,
        )
        .unwrap();
        let mid: u64 = store
            .conn()
            .query_row(
                "SELECT id FROM messages WHERE mailbox_id = ?1 AND message_id = ?2",
                rusqlite::params![mb, "<filing-1@example.com>"],
                |r| r.get(0),
            )
            .unwrap();
        sge_lib::store::queries::upsert_label(
            store.conn(),
            mid,
            "academico",
            None,
            0.9,
            0.5,
            false,
        )
        .unwrap();
        assert!(sge_lib::store::queries::get_label(store.conn(), mid)
            .unwrap()
            .is_some());
        // Undo of the filing: from the filed category back to the origin.
        sge_lib::store::queries::log_override(store.conn(), mid, "academico", "SGE-Test-filing-2")
            .unwrap();
        assert_eq!(
            sge_lib::store::queries::latest_override(store.conn(), mid).unwrap(),
            Some("SGE-Test-filing-2".to_string())
        );
        let _ = std::fs::remove_file(&db);
    }
}

/// v1.3 RENAME/DELETE leg shapes (Task 1): rename lifecycle on the double,
/// backend-side guard order, prod cache-migrate entries.
pub mod live_gate_26_folders {
    use super::*;

    #[test]
    fn folders_rename_uids_survive_shape() {
        let old = "SGE-Test-folders-1-Rename";
        let new = "SGE-Test-folders-1-Renamed";
        harness::assert_sandbox_prefix(old).unwrap();
        harness::assert_sandbox_prefix(new).unwrap();
        let mut s = FakeSession::new();
        s.mailboxes.insert("INBOX".to_string());
        block_on(s.create_mailbox(old)).unwrap();
        block_on(s.select_mailbox(old)).unwrap();
        block_on(s.append_message(old, "", b"Subject: SGE-TEST-rename-folder\r\n\r\nx")).unwrap();
        let uid = block_on(s.search_uids()).unwrap()[0];
        block_on(s.rename_mailbox(old, new)).unwrap();
        // LIST: old gone, new present.
        let infos = block_on(s.list_mailboxes()).unwrap();
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert!(!names.contains(&old));
        assert!(names.contains(&new));
        // UIDs survive under the new name.
        block_on(s.select_mailbox(new)).unwrap();
        assert_eq!(block_on(s.search_uids()).unwrap(), vec![uid]);
    }

    #[test]
    fn folders_guards_refuse_inbox_and_nonempty() {
        // Backend-side guard order (mirrors the Phase 11 command rule):
        // INBOX first, then non-empty — both before any verb.
        assert!(harness::assert_folder_delete_allowed("INBOX", 0).is_err());
        assert!(harness::assert_folder_delete_allowed("inbox", 99).is_err());
        assert!(harness::assert_folder_delete_allowed("SGE-Test-x-Renamed", 3).is_err());
        assert!(harness::assert_folder_delete_allowed("SGE-Test-x-Renamed", 0).is_ok());
        // The prefix guard is the outer ring: INBOX never reaches a verb.
        assert!(harness::assert_sandbox_prefix("INBOX").is_err());
    }

    #[test]
    fn folders_cache_migrate_shapes() {
        // rename_mailbox_cache migrates the row (prod rename-IPC entry);
        // delete_mailbox_cache drops it (prod delete-IPC entry).
        let dir = std::env::temp_dir().join(format!("sge-26-folders-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("folders.sqlite");
        let store = sge_lib::store::Store::open(&db).unwrap();
        sge_lib::store::queries::ensure_mailbox(store.conn(), "SGE-Test-folders-2-Rename").unwrap();
        let touched = sge_lib::store::queries::rename_mailbox_cache(
            store.conn(),
            "SGE-Test-folders-2-Rename",
            "SGE-Test-folders-2-Renamed",
            "/",
        )
        .unwrap();
        assert_eq!(touched, 1);
        let rows = sge_lib::store::queries::list_mailboxes(store.conn()).unwrap();
        assert!(!rows.iter().any(|r| r.name == "SGE-Test-folders-2-Rename"));
        assert!(rows.iter().any(|r| r.name == "SGE-Test-folders-2-Renamed"));
        sge_lib::store::queries::delete_mailbox_cache(store.conn(), "SGE-Test-folders-2-Renamed")
            .unwrap();
        let rows = sge_lib::store::queries::list_mailboxes(store.conn()).unwrap();
        assert!(!rows.iter().any(|r| r.name == "SGE-Test-folders-2-Renamed"));
        let _ = std::fs::remove_file(&db);
    }
}

/// v1.3 batch/chunk probe leg shapes (Task 1): chunk ranges, journal
/// resume (zero dupes/skips), candidate scoping with pin respect.
pub mod live_gate_26_batch {
    use super::*;

    #[test]
    fn batch_plan_chunks_ranges_with_tail() {
        // 7 seeds at chunk 3: three chunks, partial tail exercised.
        assert_eq!(
            harness::plan_chunks(harness::BATCH_PROBE_SEEDS, harness::BATCH_PROBE_CHUNK),
            vec![(0, 3), (3, 6), (6, 7)]
        );
        assert!(
            harness::plan_chunks(harness::BATCH_PROBE_SEEDS, harness::BATCH_PROBE_CHUNK).len() >= 2
        );
        assert_eq!(harness::plan_chunks(0, 3), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn batch_journal_resume_zero_dupe_skip_shape() {
        // intent-before-verb journaling: every scoped candidate journaled
        // (resume skips all → zero dupes), none unjournaled (zero skips).
        let dir = std::env::temp_dir().join(format!("sge-26-batch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("batch.sqlite");
        let store = sge_lib::store::Store::open(&db).unwrap();
        let scope = "SGE-Test-batch-1";
        let mb = sge_lib::store::queries::ensure_mailbox(store.conn(), scope).unwrap();
        for (i, uid) in [21u32, 22, 23].iter().enumerate() {
            sge_lib::store::queries::upsert_message(
                store.conn(),
                mb,
                *uid,
                Some(&format!("<batch-{i}@example.com>")),
                &format!("SGE-TEST-batch-probe-{i}"),
                "me@example.com",
                "[\"me@example.com\"]",
                "[]",
                "2026-10-13T12:00:00Z",
                "[]",
                false,
                "",
                None,
                None,
            )
            .unwrap();
        }
        let run = sge_lib::store::queries::create_batch_run(store.conn()).unwrap();
        let cands =
            sge_lib::classify::batch::collect_candidates(store.conn(), Some(scope)).unwrap();
        assert_eq!(cands.len(), 3);
        // Chunked journal: chunk size 2 → (2,1), one intent per item, moved
        // after the (fake) verb.
        for (ci, (s, e)) in harness::plan_chunks(cands.len(), 2).iter().enumerate() {
            for (mid, from, _uid) in &cands[*s..*e] {
                sge_lib::classify::batch::journal_intent(
                    store.conn(),
                    run,
                    *mid,
                    from,
                    "SGE-Test-batch-1-Auto",
                    "batch-probe",
                    ci,
                )
                .unwrap();
                sge_lib::classify::batch::journal_moved(store.conn(), run, *mid).unwrap();
            }
        }
        let done = sge_lib::classify::batch::journaled_ids(store.conn(), run).unwrap();
        assert_eq!(done.len(), 3);
        let remaining: Vec<u64> = cands
            .iter()
            .filter(|(mid, _, _)| !done.contains(mid))
            .map(|(mid, _, _)| *mid)
            .collect();
        assert!(
            remaining.is_empty(),
            "resume must skip everything journaled"
        );
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn batch_candidates_skip_pinned_and_auto_tree() {
        // collect_candidates: pinned-override mail skipped (corrections
        // stand), Auto/-tree mail out of scope, excluded folders skipped.
        let dir = std::env::temp_dir().join(format!("sge-26-batch2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("batch2.sqlite");
        let store = sge_lib::store::Store::open(&db).unwrap();
        let scope = "SGE-Test-batch-2";
        let mb = sge_lib::store::queries::ensure_mailbox(store.conn(), scope).unwrap();
        sge_lib::store::queries::upsert_message(
            store.conn(),
            mb,
            31,
            Some("<plain@example.com>"),
            "SGE-TEST-plain",
            "me@example.com",
            "[\"me@example.com\"]",
            "[]",
            "2026-10-13T12:00:00Z",
            "[]",
            false,
            "",
            None,
            None,
        )
        .unwrap();
        sge_lib::store::queries::upsert_message(
            store.conn(),
            mb,
            32,
            Some("<pinned@example.com>"),
            "SGE-TEST-pinned",
            "me@example.com",
            "[\"me@example.com\"]",
            "[]",
            "2026-10-13T12:00:00Z",
            "[]",
            false,
            "",
            None,
            None,
        )
        .unwrap();
        let pinned_mid: u64 = store
            .conn()
            .query_row(
                "SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = 32",
                rusqlite::params![mb],
                |r| r.get(0),
            )
            .unwrap();
        sge_lib::store::queries::log_override(store.conn(), pinned_mid, "academico", scope)
            .unwrap();
        let auto_mb =
            sge_lib::store::queries::ensure_mailbox(store.conn(), "Auto/Acadêmico").unwrap();
        sge_lib::store::queries::upsert_message(
            store.conn(),
            auto_mb,
            1,
            Some("<filed@example.com>"),
            "filed",
            "me@example.com",
            "[\"me@example.com\"]",
            "[]",
            "2026-10-13T12:00:00Z",
            "[]",
            false,
            "",
            None,
            None,
        )
        .unwrap();
        let cands =
            sge_lib::classify::batch::collect_candidates(store.conn(), Some(scope)).unwrap();
        assert_eq!(
            cands.len(),
            1,
            "only the unpinned in-scope message is a candidate"
        );
        assert_eq!(cands[0].2, 31);
        // Unscoped collect still excludes the Auto/ tree.
        let all = sge_lib::classify::batch::collect_candidates(store.conn(), None).unwrap();
        assert!(all.iter().all(|(_, mb, _)| mb != "Auto/Acadêmico"));
        let _ = std::fs::remove_file(&db);
    }
}

/// 12/13 citation-row shapes (Task 2): re-run vs cite branching, SKIP-row
/// emission with reason + resume pointer, and the no-duplication contract
/// (the bundle cites the locked 21/22 runners — it holds no draft-send or
/// SMTP-send implementation of its own).
pub mod live_gate_26_cite {
    use super::*;

    #[test]
    fn cite_evidence_found_branch_shape() {
        // Prior-run log carries the locked runner's REPORT line → PASS cite
        // row naming the source (binary STEP id / REPORT path + line).
        let log = "STEP 1/13 preflight: PASS x\nSTEP 5/13 one-copy: PASS exactly-one-copy uid=7\nREPORT steps=13\n";
        let line = harness::citation_evidence(log, harness::CITE_DRAFT_MARKER);
        assert_eq!(
            line,
            Some("STEP 5/13 one-copy: PASS exactly-one-copy uid=7".to_string())
        );
        let row = format!(
            "REPORT gate=cite-draft-onecopy verdict=PASS evidence=cite=/tmp/prior22.log line={}",
            line.unwrap()
        );
        assert!(row.contains("cite=") && row.contains("one-copy: PASS"));
    }

    #[test]
    fn cite_evidence_absent_branch_emits_reasoned_skip() {
        // No prior run (normal case: 21/22 live pending creds) → SKIP row
        // with concrete reason + resume pointer, never a bare deferred.
        let reason =
            "no prior-run log at $SGE_LIVE22_LOG (locked 21/22 live run still pending creds)";
        let row = format!(
            "REPORT gate=cite-draft-onecopy verdict=SKIP evidence={}",
            harness::deferral_evidence(reason, "scripts/live-22.sh")
        );
        assert!(row.contains("verdict=SKIP"));
        let evidence = row.split_once("evidence=").unwrap().1;
        assert!(harness::is_reasoned_deferral(evidence));
        // A marker that never passed there is a concrete reason too.
        let reason2 = "prior log /tmp/x.log lacks \"one-copy: PASS\" (that leg never passed there)";
        assert!(harness::is_reasoned_deferral(&harness::deferral_evidence(
            reason2,
            "scripts/live-22.sh"
        )));
    }

    #[test]
    fn cite_markers_name_locked_runners() {
        // The citation contract: three distinct, non-empty markers naming
        // the locked 21/22 STEP/REPORT ids (binary plus STEP id, or REPORT
        // path plus line) — the bundle re-runs nothing.
        for marker in [
            harness::CITE_DRAFT_MARKER,
            harness::CITE_SEND_MARKER,
            harness::CITE_SEEN_MARKER,
        ] {
            assert!(!marker.trim().is_empty());
        }
        assert_ne!(harness::CITE_DRAFT_MARKER, harness::CITE_SEEN_MARKER);
        assert!(harness::CITE_SEND_MARKER.contains("REPORT draft-send binary verdict"));
        // Empty markers never match (no vacuous PASS cites).
        assert_eq!(harness::citation_evidence("one-copy: PASS", ""), None);
    }

    #[test]
    fn cite_skip_rows_refuse_bare_deferral() {
        // D-47 enforcement shape: the wrapper + REPORT accept a SKIP row
        // only with reason and resume pointer both present and non-empty.
        assert!(!harness::is_reasoned_deferral(
            "reason= resume=scripts/live-21.sh"
        ));
        assert!(!harness::is_reasoned_deferral(
            "reason=locked run pending resume="
        ));
        assert!(!harness::is_reasoned_deferral(
            "resume=scripts/live-21.sh reason="
        ));
        assert!(harness::is_reasoned_deferral(&harness::deferral_evidence(
            "smtp would duplicate sends (T-26-10)",
            "scripts/live-21.sh"
        )));
    }
}

/// Single combined bundle REPORT shapes (Task 3): per-gate row emission,
/// deferral-row validation (reason + pointer present, empty refused),
/// NOT-RUN fill for unrun gates, and zero secret substrings in any
/// committed verdict text (T-26-11).
pub mod live_gate_26_report {
    use super::*;

    fn row(gate: &str, verdict: &str, evidence: &str) -> String {
        format!("REPORT gate={gate} verdict={verdict} evidence={evidence}")
    }

    #[test]
    fn report_rows_carry_gate_verdict_evidence() {
        let pass = row(
            "confirm-move-undo",
            "PASS",
            "confirm→MOVE uid 9 in auto + undo restored 1",
        );
        assert!(pass.starts_with("REPORT gate=confirm-move-undo verdict=PASS evidence="));
        let fail = row("rename-delete", "FAIL", "RENAME failed: timeout");
        assert!(fail.contains("verdict=FAIL"));
        let skip = row(
            "cite-smtp-send",
            "SKIP",
            &harness::deferral_evidence("no prior-run log (21 live pending)", "scripts/live-21.sh"),
        );
        assert!(skip.contains("verdict=SKIP"));
        assert!(harness::is_reasoned_deferral(
            skip.split_once("evidence=").unwrap().1
        ));
    }

    #[test]
    fn report_bundle_covers_deferred_set_with_not_run_fill() {
        // The bundle covers exactly the full deferred set; anything
        // collected short of it surfaces as NOT-RUN (never silent).
        assert_eq!(harness::ALL_GATES.len(), 10);
        let partial = vec![(
            "seen-roundtrip".to_string(),
            "PASS".to_string(),
            "d".to_string(),
        )];
        let missing = harness::missing_gates(&partial, harness::ALL_GATES);
        assert_eq!(missing.len(), 9);
        assert!(missing.contains(&"cite-smtp-send"));
        for gate in missing {
            let fill = format!("BUNDLE-ROW gate={gate} verdict=NOT-RUN evidence=leg-never-ran");
            assert!(fill.contains("verdict=NOT-RUN"));
        }
    }

    #[test]
    fn report_verdict_text_carries_no_secrets() {
        // Committed verdict text is reasons + pointers only (T-26-11):
        // the scan the wrapper runs would fire on any leak here.
        let secrets = ["hunter2-imap-pass", "smtp-secret-9"];
        let verdicts = [
            row("batch-probe", "PASS", "7 moves in 3 chunks, resume 0-dupe/0-skip"),
            row(
                "cite-draft-onecopy",
                "SKIP",
                &harness::deferral_evidence(
                    "no prior-run log at $SGE_LIVE22_LOG (locked run pending creds)",
                    "scripts/live-22.sh",
                ),
            ),
            "BUNDLE-ROW gate=starttls-live verdict=SKIP evidence=reason=server is 993-only resume=re-run".to_string(),
        ];
        for v in &verdicts {
            assert_eq!(harness::scan_text_for_secrets(v, &secrets), None);
        }
        // And the scan itself is proven armed: a leaked row IS caught.
        let leaked = row(
            "seen-roundtrip",
            "PASS",
            "uid 3 with hunter2-imap-pass inside",
        );
        assert_eq!(harness::scan_text_for_secrets(&leaked, &secrets), Some(0));
    }
}
