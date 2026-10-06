//! Single-session IMAP ownership (Phase 6, Plan 06-01).
//!
//! [`SessionManager`] owns the one authenticated [`BoxedSession`] and hands
//! out mailbox-scoped [`MailboxLease`]s: exclusive, folder-selected access
//! with single-flight serialization. The first lease connects and SELECTs;
//! after a transparent reconnect the mailbox is re-SELECTed before the
//! caller proceeds, so every flag write lands on the intended mailbox.
//!
//! Locking discipline: the inner state is an async mutex — it may be held
//! across `.await` (unlike the sync Store mutex, which must never cross an
//! await point). At most one lease exists at a time, so concurrent
//! `set_seen` and sync passes serialize instead of interleaving STOREs.
//!
//! Password discipline: the owned [`AccountConfig`] (whose `Debug` redacts
//! the password) is only cloned into `connect_sync`. No log line in this
//! module formats the config, the password, or credentials.

use super::{
    choose_expunge_path, choose_move_path, chunk_uid_set, AccountConfig, ExpungePath,
    MovePath, SyncError, SyncSession,
};
use crate::imap::session::connect_sync;
use std::collections::HashSet;

/// Owns one authenticated IMAP session for the active account.
///
/// Created per account from the same credential-loading shape as
/// `start_sync` (in-memory `active_account` first, keyring fallback —
/// see `commands::sync`); the command layer caches one manager per
/// account key and replaces it when the account changes.
pub struct SessionManager {
    config: AccountConfig,
    state: async_std::sync::Mutex<ManagerState>,
}

#[derive(Default)]
struct ManagerState {
    session: Option<Box<dyn SyncSession>>,
    selected_mailbox: Option<String>,
    /// `CAPABILITY` atoms queried once per fresh connection (the set is
    /// stable per session). Filled in [`SessionManager::lease_for`],
    /// invalidated by `reconnect()` — Plan 10-02 capability cache.
    cached_capabilities: Option<Vec<String>>,
}

impl SessionManager {
    /// Take ownership of the account config; connects lazily on first lease.
    pub fn new(config: AccountConfig) -> Self {
        Self {
            config,
            state: async_std::sync::Mutex::new(ManagerState::default()),
        }
    }

    /// Identity of the owned account (host/port/username — never secrets)
    /// so the command layer can cache one manager per account.
    pub fn account_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.config.host, self.config.port, self.config.username
        )
    }

    /// Exclusive, mailbox-selected access to the owned session.
    ///
    /// Connects on first use, (re-)SELECTs `mailbox` whenever the session is
    /// fresh or a different folder is selected, and serializes concurrent
    /// callers through the state mutex (FOLD-03: per-folder leases).
    pub async fn lease_for(&self, mailbox: &str) -> Result<MailboxLease<'_>, SyncError> {
        let mut guard = self.state.lock().await;
        if guard.session.is_none() {
            let session = connect_sync(&self.config).await.map_err(|e| {
                SyncError::Protocol(format!("SessionManager connect: {e}"))
            })?;
            guard.session = Some(Box::new(session));
            guard.selected_mailbox = None;
            guard.cached_capabilities = None;
        }
        let needs_select = guard.selected_mailbox.as_deref() != Some(mailbox);
        if needs_select {
            let session = guard.session.as_mut().expect("connected above");
            let summary = session.select_mailbox(mailbox).await.map_err(|e| {
                SyncError::Protocol(format!("SessionManager SELECT {mailbox}: {e}"))
            })?;
            eprintln!(
                "[SGE imap] lease selected {mailbox}: uid_validity={} exists={}",
                summary.uid_validity, summary.exists
            );
            guard.selected_mailbox = Some(mailbox.to_string());
        }
        // Capability cache: the atom set is stable per connection, so one
        // `CAPABILITY` per fresh session serves every gated op until the
        // next reconnect. Failing closed here surfaces a broken session
        // immediately instead of misrouting MOVE/expunge fallbacks.
        if guard.cached_capabilities.is_none() {
            let session = guard.session.as_mut().expect("connected above");
            let caps = session.capabilities().await.map_err(|e| {
                SyncError::Protocol(format!("SessionManager CAPABILITY: {e}"))
            })?;
            guard.cached_capabilities = Some(caps);
        }
        Ok(MailboxLease { guard })
    }

    /// Exclusive, INBOX-selected access to the owned session.
    ///
    /// Back-compat delegate of [`lease_for`](Self::lease_for) for INBOX-only
    /// (Phase 6) callers.
    pub async fn lease(&self) -> Result<MailboxLease<'_>, SyncError> {
        self.lease_for("INBOX").await
    }

    /// Drop the owned session; the next lease reconnects transparently.
    /// Used after a failed write before the single retry.
    async fn reconnect(&self) -> Result<(), SyncError> {
        let mut guard = self.state.lock().await;
        let session = connect_sync(&self.config).await.map_err(|e| {
            SyncError::Protocol(format!("SessionManager reconnect: {e}"))
        })?;
        guard.session = Some(Box::new(session));
        guard.selected_mailbox = None;
        // Capabilities belong to the old connection — the next lease
        // re-queries them (Plan 10-02 invalidation rule).
        guard.cached_capabilities = None;
        Ok(())
    }

    /// Cached `CAPABILITY` atoms for the owned connection (Plan 10-02).
    ///
    /// Served from the per-connection cache filled by [`lease_for`](Self::lease_for);
    /// the `INBOX` lease is the stable default (same as `list_mailboxes`).
    pub async fn capabilities_cached(&self) -> Result<Vec<String>, SyncError> {
        let mut lease = self.lease_for("INBOX").await?;
        if let Some(caps) = lease.cached_capabilities() {
            return Ok(caps);
        }
        // Defensive only: `lease_for` fills the cache, so this runs solely
        // when a future lease path skips the fill — never a second lease
        // (no re-entrant `lease_for` while holding one).
        lease.session().capabilities().await
    }

    /// UID STORE `\Seen` through the owned session with one transparent
    /// reconnect + retry: if the write fails the session may be stale, so
    /// reconnect, re-SELECT (via a fresh lease), and retry exactly once.
    ///
    /// The write lands on `mailbox`: the lease SELECTs it first, so a
    /// `set_seen` for Sent never touches INBOX (FOLD-03).
    pub async fn set_seen_in(
        &self,
        mailbox: &str,
        uid: u32,
        seen: bool,
    ) -> Result<(), SyncError> {
        let mut lease = self.lease_for(mailbox).await?;
        match lease.session().set_seen(uid, seen).await {
            Ok(()) => Ok(()),
            Err(first) => {
                eprintln!("[SGE imap] set_seen {mailbox} uid {uid} failed ({first}) — reconnecting once");
                drop(lease);
                self.reconnect().await?;
                let mut lease = self.lease_for(mailbox).await?;
                lease.session().set_seen(uid, seen).await
            }
        }
    }

    /// UID STORE `\Seen` on INBOX — back-compat delegate for Phase 6 callers.
    pub async fn set_seen(&self, uid: u32, seen: bool) -> Result<(), SyncError> {
        self.set_seen_in("INBOX", uid, seen).await
    }

    /// UID STORE `\Deleted` through the owned session with one transparent
    /// reconnect + retry (Plan 10-02, DEL slice). Same shape as
    /// [`set_seen_in`](Self::set_seen_in): the lease SELECTs `mailbox`
    /// first, so the flag lands on the intended folder.
    pub async fn mark_deleted_in(
        &self,
        mailbox: &str,
        uid: u32,
        deleted: bool,
    ) -> Result<(), SyncError> {
        let mut lease = self.lease_for(mailbox).await?;
        match lease.session().store_deleted(uid, deleted).await {
            Ok(()) => Ok(()),
            Err(first) => {
                eprintln!("[SGE imap] store_deleted {mailbox} uid {uid} failed ({first}) — reconnecting once");
                drop(lease);
                self.reconnect().await?;
                let mut lease = self.lease_for(mailbox).await?;
                lease.session().store_deleted(uid, deleted).await
            }
        }
    }

    /// Scoped `UID EXPUNGE <set>` (RFC 4315, requires UIDPLUS) through the
    /// owned session with one transparent reconnect + retry (Plan 10-02,
    /// DEL-02 slice). Returns the expunged UIDs as reported by the server
    /// (drained to completion, never trusted for local cache — the next
    /// SEARCH reconciles). The lease SELECTs `mailbox` first so the expunge
    /// cannot drift to the wrong folder.
    pub async fn uid_expunge_in(
        &self,
        mailbox: &str,
        uid_set: &str,
    ) -> Result<Vec<u32>, SyncError> {
        let set = uid_set.to_string();
        let mut lease = self.lease_for(mailbox).await?;
        match lease.session().uid_expunge(&set).await {
            Ok(uids) => Ok(uids),
            Err(first) => {
                eprintln!("[SGE imap] UID EXPUNGE {mailbox} set {set} failed ({first}) — reconnecting once");
                drop(lease);
                self.reconnect().await?;
                let mut lease = self.lease_for(mailbox).await?;
                lease.session().uid_expunge(&set).await
            }
        }
    }

    /// `CREATE <wire>` through the owned session with one transparent
    /// reconnect + retry (Plan 11-01 folder CREATE; same shape as
    /// [`create_trash`](Self::create_trash)). `CREATE` needs no particular
    /// folder selected, so the stable `INBOX` lease suffices. Takes the RAW
    /// wire name (already modified-UTF-7 encoded by the caller) — never
    /// encodes inside. A [`SyncError::Refused`] is deterministic and is
    /// never retried (same rule as [`move_message_in`](Self::move_message_in)).
    pub async fn create_mailbox_in(&self, wire: &str) -> Result<(), SyncError> {
        let wire_owned = wire.to_string();
        let mut lease = self.lease_for("INBOX").await?;
        match lease.session().create_mailbox(&wire_owned).await {
            Ok(()) => Ok(()),
            Err(refused @ SyncError::Refused(_)) => Err(refused),
            Err(first) => {
                eprintln!("[SGE imap] CREATE {wire_owned} failed ({first}) — reconnecting once");
                drop(lease);
                self.reconnect().await?;
                let mut lease = self.lease_for("INBOX").await?;
                lease.session().create_mailbox(&wire_owned).await
            }
        }
    }
    /// `CREATE Trash` through the owned session with one transparent
    /// reconnect + retry (Plan 10-02 Trash fallback). `CREATE` needs no
    /// particular folder selected, so the stable `INBOX` lease suffices.
    /// Raw ASCII name — no modified-UTF-7 encoding needed.
    pub async fn create_trash(&self) -> Result<(), SyncError> {
        let mut lease = self.lease_for("INBOX").await?;
        match lease.session().create_mailbox("Trash").await {
            Ok(()) => Ok(()),
            Err(first) => {
                eprintln!("[SGE imap] CREATE Trash failed ({first}) — reconnecting once");
                drop(lease);
                self.reconnect().await?;
                let mut lease = self.lease_for("INBOX").await?;
                lease.session().create_mailbox("Trash").await
            }
        }
    }

    /// `RENAME <old> -> <new>` through the owned session with one
    /// transparent reconnect + retry (Plan 11-02). The lease SELECTs `old`
    /// first (keeps SELECT state sane); a [`SyncError::Refused`] is
    /// deterministic and never retried. Ends with a `mailbox_status(new)`
    /// UIDVALIDITY check against the pre-rename value (read by the caller
    /// of this method): a bump surfaces [`SyncError::State`] — the command
    /// layer treats it like an epoch change, never a silent accept.
    ///
    /// Locking: the rename lease is dropped before the post-rename STATUS
    /// (same async mutex — holding it would deadlock).
    pub async fn rename_mailbox_in(&self, old: &str, new: &str) -> Result<(), SyncError> {
        let pre = self.mailbox_status(old).await?.uid_validity;
        {
            let mut lease = self.lease_for(old).await?;
            match lease.session().rename_mailbox(old, new).await {
                Ok(()) => {}
                Err(refused @ SyncError::Refused(_)) => return Err(refused),
                Err(first) => {
                    eprintln!("[SGE imap] RENAME {old} -> {new} failed ({first}) — reconnecting once");
                    drop(lease);
                    self.reconnect().await?;
                    let mut lease = self.lease_for(old).await?;
                    lease.session().rename_mailbox(old, new).await?;
                }
            }
        }
        let post = self.mailbox_status(new).await?.uid_validity;
        if post != pre {
            return Err(SyncError::State(format!(
                "RENAME {old} -> {new}: UIDVALIDITY changed {pre} -> {post} — treated like an epoch change"
            )));
        }
        Ok(())
    }

    /// `DELETE <name>` through the owned session with one transparent
    /// reconnect + retry (Plan 11-02). The lease SELECTs `name` first (keeps
    /// SELECT state sane); a [`SyncError::Refused`] is deterministic and
    /// never retried.
    pub async fn delete_mailbox_in(&self, name: &str) -> Result<(), SyncError> {
        let mut lease = self.lease_for(name).await?;
        match lease.session().delete_mailbox(name).await {
            Ok(()) => Ok(()),
            Err(refused @ SyncError::Refused(_)) => Err(refused),
            Err(first) => {
                eprintln!("[SGE imap] DELETE {name} failed ({first}) — reconnecting once");
                drop(lease);
                self.reconnect().await?;
                let mut lease = self.lease_for(name).await?;
                lease.session().delete_mailbox(name).await
            }
        }
    }
    /// Move `uid_set` (comma-joined `"1,2,3"`) from `src` to `dest` (raw
    /// wire names) under ONE held lease, with reconnect-retry and
    /// capability-gated fallback orchestration (Plan 10-02, MOVE-01 slice).
    ///
    /// MOVE advertised → one-verb `UID MOVE` per ~200-UID chunk. Otherwise
    /// COPY → STORE `+Deleted` (per UID) → scoped removal: `UID EXPUNGE`
    /// with UIDPLUS, else the unmark-others dance behind a bare `EXPUNGE`
    /// (loud refusal when the unmark cannot be verified — never a blind
    /// expunge). On first failure the lease drops, the session reconnects
    /// (capabilities re-read), and the whole sequence retries exactly once.
    pub async fn move_message_in(
        &self,
        src: &str,
        uid_set: &str,
        dest: &str,
    ) -> Result<MoveOutcome, SyncError> {
        match self.move_once(src, uid_set, dest).await {
            Ok(outcome) => Ok(outcome),
            // A loud refusal (unverifiable unmark dance) is deterministic:
            // retrying would re-COPY (duplicates in dest) only to refuse
            // again, so it bypasses the reconnect-retry.
            Err(refused @ SyncError::Refused(_)) => Err(refused),
            Err(first) => {
                eprintln!(
                    "[SGE imap] move {src} -> {dest} set {uid_set} failed ({first}) — reconnecting once"
                );
                self.reconnect().await?;
                self.move_once(src, uid_set, dest).await
            }
        }
    }

    /// One attempt of the move. Holds a SINGLE `lease_for(src)` guard for
    /// the whole COPY/STORE/EXPUNGE sequence so no intermediate SELECT can
    /// drift the expunge to the wrong folder. Never calls `self.lease_for`
    /// re-entrantly while holding the lease (async mutex → deadlock) —
    /// verbs run on `lease.session()` directly.
    async fn move_once(
        &self,
        src: &str,
        uid_set: &str,
        dest: &str,
    ) -> Result<MoveOutcome, SyncError> {
        let uids = parse_uid_set(uid_set)?;
        let mut lease = self.lease_for(src).await?;
        let caps = match lease.cached_capabilities() {
            Some(caps) => caps,
            // Defensive only: `lease_for` fills the cache, so this runs
            // solely when a future lease path skips the fill.
            None => lease.session().capabilities().await?,
        };
        run_move_sequence(lease.session(), &uids, dest, &caps).await
    }

    /// `LIST "" "*"` through the owned session (FOLD-01 folder discovery).
    pub async fn list_mailboxes(&self) -> Result<Vec<super::MailboxInfo>, SyncError> {
        let mut lease = self.lease_for("INBOX").await?;
        lease.session().list_mailboxes().await
    }

    /// `STATUS <mailbox> (UIDVALIDITY UIDNEXT UNSEEN MESSAGES)` through the
    /// owned session (FOLD-02 triage signals + Plan 11-02 message count).
    /// Does not disturb the lease's SELECTed folder — STATUS works on any
    /// mailbox.
    pub async fn mailbox_status(
        &self,
        mailbox: &str,
    ) -> Result<super::MailboxStatus, SyncError> {
        let mut lease = self.lease_for("INBOX").await?;
        lease.session().mailbox_status(mailbox).await
    }
}

/// Outcome of [`SessionManager::move_message_in`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveOutcome {
    /// True when the server lacked MOVE and the COPY + STORE + EXPUNGE
    /// fallback sequence ran instead of one-verb `UID MOVE`.
    pub used_fallback: bool,
}

/// Capability-gated move on an already-selected session, without a manager
/// lease (Plan 10-03 pre-sweep replay entry).
///
/// The sync worker borrows the pass session (already SELECTed to the source
/// mailbox) and replays queued delete/move ops through the same
/// [`run_move_sequence`] body the manager uses — one implementation, two
/// callers. Issues no `select_mailbox` itself; dest-side Seen resolution
/// (which SELECTs) is the caller's job to bracket with a re-SELECT.
pub async fn run_move_on_session(
    session: &mut dyn SyncSession,
    uids: &[u32],
    dest: &str,
    caps: &[String],
) -> Result<MoveOutcome, SyncError> {
    run_move_sequence(session, uids, dest, caps).await
}

/// Parse a comma-joined UID set (`"1,2,3"`) into UIDs. Fail-closed on any
/// malformed segment — a bad set must never reach the wire.
fn parse_uid_set(uid_set: &str) -> Result<Vec<u32>, SyncError> {
    uid_set
        .split(',')
        .map(|s| {
            s.trim().parse::<u32>().map_err(|_| {
                SyncError::State(format!(
                    "invalid UID set {uid_set:?} — expected comma-joined UIDs"
                ))
            })
        })
        .collect()
}

/// True when a stored flags value carries `\Deleted`.
///
/// Flags travel as the JSON array `format_flags` writes
/// (`["\\Seen","\\Deleted"]`); parse it exactly, falling back to a
/// substring probe for foreign shapes rather than missing a mark. A clean
/// parse with no exact atom is trusted (a custom keyword merely
/// containing "deleted" must not trigger an unmark/restore cycle).
fn flags_carry_deleted(flags: &str) -> bool {
    if let Ok(names) = serde_json::from_str::<Vec<String>>(flags) {
        return names.iter().any(|n| n == "\\Deleted");
    }
    flags.contains("Deleted")
}

/// Capability-gated move body: runs on the caller's already-leased session
/// (SELECT stable) and issues NO `select_mailbox` itself — the single-lease
/// rule. Chunked at [`chunk_uid_set`] width so multi-message ops stay
/// poll-responsive.
async fn run_move_sequence(
    session: &mut dyn SyncSession,
    uids: &[u32],
    dest: &str,
    caps: &[String],
) -> Result<MoveOutcome, SyncError> {
    if uids.is_empty() {
        return Ok(MoveOutcome {
            used_fallback: false,
        });
    }
    if choose_move_path(caps) == MovePath::UidMove {
        for chunk in chunk_uid_set(uids) {
            session.uid_move_to(&chunk, dest).await?;
        }
        return Ok(MoveOutcome {
            used_fallback: false,
        });
    }
    // Fallback without MOVE: COPY, then mark, then scoped removal.
    for chunk in chunk_uid_set(uids) {
        session.uid_copy_to(&chunk, dest).await?;
    }
    for uid in uids {
        session.store_deleted(*uid, true).await?;
    }
    if choose_expunge_path(caps) == ExpungePath::UidExpunge {
        for chunk in chunk_uid_set(uids) {
            session.uid_expunge(&chunk).await?;
        }
    } else {
        unmark_dance_expunge(session, uids).await?;
    }
    Ok(MoveOutcome {
        used_fallback: true,
    })
}

/// UIDPLUS-absent expunge: protect other clients' `\Deleted` marks across a
/// bare `EXPUNGE` that would otherwise nuke every marked message in the
/// folder.
///
/// 1. Sweep flags (read-only, no SELECT) for live `\Deleted` UIDs outside
///    our set (foreign marks). 2. `-FLAGS` them. 3. Re-read and VERIFY the
///    unmark landed — any surviving mark aborts with a loud,
///    plain-language refusal (nothing expunged yet). 4. Bare `EXPUNGE`
///    (removes exactly our marked set, barring a concurrent foreign mark
///    in the gap — residual risk inherent to servers without UIDPLUS).
/// 5. Restore the foreign marks.
async fn unmark_dance_expunge(
    session: &mut dyn SyncSession,
    ours: &[u32],
) -> Result<(), SyncError> {
    let ours_set: HashSet<u32> = ours.iter().copied().collect();
    let live = session.search_uids().await?;
    let mut foreign: Vec<u32> = Vec::new();
    for chunk in chunk_uid_set(&live) {
        for header in session.fetch_envelopes(&chunk).await? {
            if !ours_set.contains(&header.uid) && flags_carry_deleted(&header.flags) {
                foreign.push(header.uid);
            }
        }
    }
    for uid in &foreign {
        session.store_deleted(*uid, false).await?;
    }
    if !foreign.is_empty() {
        let check = foreign
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        for header in session.fetch_envelopes(&check).await? {
            if flags_carry_deleted(&header.flags) {
                return Err(SyncError::Refused(
                    "servidor não suporta UID EXPUNGE e a proteção de mensagens \
                     de outros clientes não pôde ser verificada — nenhuma mensagem \
                     foi apagada"
                        .to_string(),
                ));
            }
        }
    }
    session.expunge().await?;
    for uid in &foreign {
        session.store_deleted(*uid, true).await?;
    }
    Ok(())
}

/// Exclusive, mailbox-selected access to the manager's session.
///
/// Holds the single-flight lock for its lifetime: while a lease exists no
/// other caller can interleave IMAP commands on the owned session.
pub struct MailboxLease<'a> {
    guard: async_std::sync::MutexGuard<'a, ManagerState>,
}

impl MailboxLease<'_> {
    /// The live, mailbox-selected session. Never `None`: `lease_for()`
    /// connects and SELECTs before handing out the guard.
    ///
    /// Returned as a trait object so manager internals (and the sync
    /// worker's borrowed entry) drive verbs without a second connection —
    /// and so tests can inject a fake session without network.
    pub fn session(&mut self) -> &mut dyn SyncSession {
        self.guard
            .session
            .as_mut()
            .map(|boxed| boxed.as_mut())
            .expect("lease guarantees a live session")
    }

    /// Clone of the connection's cached `CAPABILITY` atoms, if the lease
    /// fill already ran. Read through the held lease — never a second
    /// `lease_for` while holding one (async mutex → deadlock).
    pub fn cached_capabilities(&self) -> Option<Vec<String>> {
        self.guard.cached_capabilities.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::super::headers::MessageHeader;
    use super::*;
    use crate::imap::PinBox;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    /// Stateful fake session: records every verb, tracks `\Deleted` flag
    /// state per UID, and actually removes expunged UIDs — so fallback
    /// sequences, the unmark dance, and multi-client survival are
    /// exercisable with zero network.
    struct FakeSession {
        select_calls: Vec<String>,
        caps_calls: usize,
        caps: Vec<String>,
        all_uids: Vec<u32>,
        deleted: HashMap<u32, bool>,
        deleted_calls: Vec<(u32, bool)>,
        copied_calls: Vec<(String, String)>,
        moved_calls: Vec<(String, String)>,
        expunged_sets: Vec<String>,
        /// UIDs actually removed by (uid-)expunge, in call order.
        removed: Vec<u32>,
        plain_expunge_calls: usize,
        created_mailboxes: Vec<String>,
        set_seen_calls: Vec<(u32, bool)>,
        /// When true, `create_mailbox` fails with a `Protocol` error
        /// (drives the reconnect-retry path of `create_mailbox_in`).
        fail_create: bool,
        /// When true, `create_mailbox` fails with `Refused` (validation
        /// refusal — must never be retried).
        fail_create_refused: bool,
        /// Recorded `(old, new)` pairs from `rename_mailbox` (Plan 11-02).
        renamed_calls: Vec<(String, String)>,
        /// Mailbox names passed to `delete_mailbox` (Plan 11-02).
        deleted_mailboxes: Vec<String>,
        /// When true, `rename_mailbox` / `delete_mailbox` fail with a
        /// `Protocol` error (drives the reconnect-retry path).
        fail_rename: bool,
        fail_delete: bool,
        /// When true, the RENAME/DELETE verbs fail with `Refused`
        /// (deterministic — must never be retried).
        fail_rename_refused: bool,
        fail_delete_refused: bool,
        /// Canned STATUS MESSAGES datum (Plan 11-02 non-empty guard).
        status_messages: u32,
        /// When true, every `mailbox_status` call bumps the returned
        /// UIDVALIDITY by one first — drives the post-RENAME epoch check.
        bump_status_validity: bool,
        status_calls: usize,
        fail_store_deleted: bool,
        fail_expunge: bool,
        fail_copy: bool,
        fail_move: bool,
        /// UIDs whose `\Deleted` survives a `-FLAGS` write (simulates a
        /// server that won't honor the unmark — drives the loud-refusal
        /// path of the UIDPLUS-absent dance).
        sticky_deleted: HashSet<u32>,
    }

    impl FakeSession {
        fn live_uids(&self) -> Vec<u32> {
            self.all_uids
                .iter()
                .copied()
                .filter(|u| !self.removed.contains(u))
                .collect()
        }

        fn flags_json(&self, uid: u32) -> String {
            if *self.deleted.get(&uid).unwrap_or(&false) {
                // JSON encoding of the canonical `["\Deleted"]` flag list.
                "[\"\\Deleted\"]".to_string()
            } else {
                "[]".to_string()
            }
        }
    }

    /// Shareable handle around [`FakeSession`]: the manager owns one clone
    /// (boxed as the session), tests keep another for assertions. Every
    /// trait method locks briefly and never holds the lock across `.await`.
    #[derive(Clone)]
    struct FakeHandle(Arc<Mutex<FakeSession>>);

    impl FakeHandle {
        fn new(caps: &[&str], uids: &[u32]) -> Self {
            Self(Arc::new(Mutex::new(FakeSession {
                select_calls: Vec::new(),
                caps_calls: 0,
                caps: caps.iter().map(|s| s.to_string()).collect(),
                all_uids: uids.to_vec(),
                deleted: HashMap::new(),
                deleted_calls: Vec::new(),
                copied_calls: Vec::new(),
                moved_calls: Vec::new(),
                expunged_sets: Vec::new(),
                removed: Vec::new(),
                plain_expunge_calls: 0,
                created_mailboxes: Vec::new(),
                set_seen_calls: Vec::new(),
                fail_create: false,
                fail_create_refused: false,
                renamed_calls: Vec::new(),
                deleted_mailboxes: Vec::new(),
                fail_rename: false,
                fail_delete: false,
                fail_rename_refused: false,
                fail_delete_refused: false,
                status_messages: 0,
                bump_status_validity: false,
                status_calls: 0,
                fail_store_deleted: false,
                fail_expunge: false,
                fail_copy: false,
                fail_move: false,
                sticky_deleted: HashSet::new(),
            })))
        }
    }

    fn parse_set(set: &str) -> HashSet<u32> {
        set.split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .collect()
    }

    impl SyncSession for FakeHandle {
        fn select_mailbox(
            &mut self,
            name: &str,
        ) -> PinBox<'_, Result<super::super::MailboxSummary, SyncError>> {
            let summary = {
                let mut f = self.0.lock().unwrap();
                f.select_calls.push(name.to_string());
                super::super::MailboxSummary {
                    selected_mailbox: name.to_string(),
                    uid_validity: 100,
                    uid_next: None,
                    exists: f.live_uids().len() as u32,
                }
            };
            Box::pin(async move { Ok(summary) })
        }

        fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            let uids = self.0.lock().unwrap().live_uids();
            Box::pin(async move { Ok(uids) })
        }

        fn fetch_envelopes<'a>(
            &'a mut self,
            range: &'a str,
        ) -> PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
            let out = {
                let f = self.0.lock().unwrap();
                let wanted = parse_set(range);
                let live: HashSet<u32> = f.live_uids().into_iter().collect();
                wanted
                    .into_iter()
                    .filter(|u| live.contains(u))
                    .map(|uid| MessageHeader {
                        uid,
                        message_id: Some(format!("<msg{uid}@example.com>")),
                        subject: format!("Subject {uid}"),
                        from_addr: "alice@example.com".to_string(),
                        to_addrs: String::new(),
                        cc_addrs: String::new(),
                        date_utc: "2024-10-03T12:00:00Z".to_string(),
                        flags: f.flags_json(uid),
                        has_attachments: false,
                        preview: format!("Subject {uid}"),
                    })
                    .collect::<Vec<_>>()
            };
            Box::pin(async move { Ok(out) })
        }

        fn fetch_body(&mut self, _uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>> {
            self.0.lock().unwrap().set_seen_calls.push((uid, seen));
            Box::pin(async move { Ok(()) })
        }

        fn store_deleted(
            &mut self,
            uid: u32,
            deleted: bool,
        ) -> PinBox<'_, Result<(), SyncError>> {
            let fail = {
                let mut f = self.0.lock().unwrap();
                f.deleted_calls.push((uid, deleted));
                if f.fail_store_deleted {
                    true
                } else {
                    // Sticky UIDs ignore the unmark write (refusal fixture).
                    if !(f.sticky_deleted.contains(&uid) && !deleted) {
                        f.deleted.insert(uid, deleted);
                    }
                    false
                }
            };
            Box::pin(async move {
                if fail {
                    Err(SyncError::Protocol("fake store_deleted failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            let (gone, fail) = {
                let mut f = self.0.lock().unwrap();
                f.plain_expunge_calls += 1;
                if f.fail_expunge {
                    (Vec::new(), true)
                } else {
                    let gone: Vec<u32> = f
                        .live_uids()
                        .into_iter()
                        .filter(|u| *f.deleted.get(u).unwrap_or(&false))
                        .collect();
                    f.removed.extend(gone.iter().copied());
                    (gone, false)
                }
            };
            Box::pin(async move {
                if fail {
                    Err(SyncError::Protocol("fake expunge failure".to_string()))
                } else {
                    Ok(gone)
                }
            })
        }

        fn uid_expunge(&mut self, uid_set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            let (gone, fail) = {
                let mut f = self.0.lock().unwrap();
                f.expunged_sets.push(uid_set.to_string());
                if f.fail_expunge {
                    (Vec::new(), true)
                } else {
                    let wanted = parse_set(uid_set);
                    // Scoped: only marked UIDs inside the set are removed —
                    // foreign `\Deleted` outside the set always survives.
                    let gone: Vec<u32> = f
                        .live_uids()
                        .into_iter()
                        .filter(|u| wanted.contains(u) && *f.deleted.get(u).unwrap_or(&false))
                        .collect();
                    f.removed.extend(gone.iter().copied());
                    (gone, false)
                }
            };
            Box::pin(async move {
                if fail {
                    Err(SyncError::Protocol("fake uid_expunge failure".to_string()))
                } else {
                    Ok(gone)
                }
            })
        }

        fn uid_copy_to(
            &mut self,
            uid_set: &str,
            dest: &str,
        ) -> PinBox<'_, Result<(), SyncError>> {
            let fail = {
                let mut f = self.0.lock().unwrap();
                f.copied_calls
                    .push((uid_set.to_string(), dest.to_string()));
                f.fail_copy
            };
            Box::pin(async move {
                if fail {
                    Err(SyncError::Protocol("fake uid_copy failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn uid_move_to(
            &mut self,
            uid_set: &str,
            dest: &str,
        ) -> PinBox<'_, Result<(), SyncError>> {
            let fail = {
                let mut f = self.0.lock().unwrap();
                f.moved_calls
                    .push((uid_set.to_string(), dest.to_string()));
                f.fail_move
            };
            Box::pin(async move {
                if fail {
                    Err(SyncError::Protocol("fake uid_move failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn capabilities(&mut self) -> PinBox<'_, Result<Vec<String>, SyncError>> {
            let caps = {
                let mut f = self.0.lock().unwrap();
                f.caps_calls += 1;
                f.caps.clone()
            };
            Box::pin(async move { Ok(caps) })
        }

        fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
            let (fail_protocol, fail_refused) = {
                let mut f = self.0.lock().unwrap();
                f.created_mailboxes.push(name.to_string());
                (f.fail_create, f.fail_create_refused)
            };
            Box::pin(async move {
                if fail_refused {
                    Err(SyncError::Refused("fake create refused".to_string()))
                } else if fail_protocol {
                    Err(SyncError::Protocol("fake create failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn list_mailboxes(
            &mut self,
        ) -> PinBox<'_, Result<Vec<super::super::MailboxInfo>, SyncError>> {
            Box::pin(async move { Ok(vec![]) })
        }

        fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>> {
            let (fail_protocol, fail_refused) = {
                let mut f = self.0.lock().unwrap();
                f.renamed_calls.push((old.to_string(), new.to_string()));
                (f.fail_rename, f.fail_rename_refused)
            };
            Box::pin(async move {
                if fail_refused {
                    Err(SyncError::Refused("fake rename refused".to_string()))
                } else if fail_protocol {
                    Err(SyncError::Protocol("fake rename failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
            let (fail_protocol, fail_refused) = {
                let mut f = self.0.lock().unwrap();
                f.deleted_mailboxes.push(name.to_string());
                (f.fail_delete, f.fail_delete_refused)
            };
            Box::pin(async move {
                if fail_refused {
                    Err(SyncError::Refused("fake delete refused".to_string()))
                } else if fail_protocol {
                    Err(SyncError::Protocol("fake delete failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn mailbox_status(
            &mut self,
            _name: &str,
        ) -> PinBox<'_, Result<super::super::MailboxStatus, SyncError>> {
            let status = {
                let mut f = self.0.lock().unwrap();
                f.status_calls += 1;
                let validity = if f.bump_status_validity { 100 + f.status_calls as u32 } else { 100 };
                super::super::MailboxStatus {
                    uid_validity: validity,
                    uid_next: None,
                    unseen: 0,
                    messages: f.status_messages,
                }
            };
            Box::pin(async move { Ok(status) })
        }

        fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }
    }

    #[cfg(test)]
    impl SessionManager {
        /// Test-only constructor with a pre-connected session: `lease_for`
        /// never dials the network, so destructive-lease tests run offline.
        fn for_test_session(session: Box<dyn SyncSession>) -> Self {
            Self {
                config: AccountConfig {
                    host: "test.invalid".to_string(),
                    port: 993,
                    security: super::super::SecurityMode::ImplicitTls,
                    username: "test".to_string(),
                    password: zeroize::Zeroizing::new("test".to_string()),
                    allow_untrusted: false,
                    plain_local_confirmed: false,
                },
                state: async_std::sync::Mutex::new(ManagerState {
                    session: Some(session),
                    selected_mailbox: None,
                    cached_capabilities: None,
                }),
            }
        }
    }

    fn run<F>(f: F)
    where
        F: std::future::Future<Output = ()>,
    {
        async_std::task::block_on(f);
    }

    #[test]
    fn capabilities_queried_once_per_connection() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS", "MOVE"], &[1, 2]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            // Two gated ops on different folders: CAPABILITY must issue
            // exactly once — the second lease reads the cache.
            manager.mark_deleted_in("INBOX", 1, true).await.unwrap();
            let caps = manager.capabilities_cached().await.unwrap();
            assert!(caps.iter().any(|c| c == "MOVE"));
            manager.uid_expunge_in("Archive", "1").await.unwrap();
            let f = probe.lock().unwrap();
            assert_eq!(f.caps_calls, 1, "CAPABILITY re-queried: {:?}", f.caps_calls);
            assert_eq!(f.select_calls, vec!["INBOX".to_string(), "Archive".to_string()]);
        });
    }

    #[test]
    fn mark_deleted_selects_folder_and_records_uid() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[7]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            manager.mark_deleted_in("Sent", 7, true).await.unwrap();
            // A follow-up lease on the same folder reuses the selection —
            // no re-SELECT, no extra CAPABILITY.
            manager.mark_deleted_in("Sent", 7, false).await.unwrap();
            let f = probe.lock().unwrap();
            assert_eq!(f.select_calls, vec!["Sent".to_string()]);
            assert_eq!(f.deleted_calls, vec![(7, true), (7, false)]);
            assert_eq!(f.caps_calls, 1);
        });
    }

    #[test]
    fn uid_expunge_and_create_trash_record_verbs() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS"], &[1, 2]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            let gone = manager.uid_expunge_in("INBOX", "1,2").await.unwrap();
            assert!(gone.is_empty(), "nothing was marked, nothing removed");
            manager.create_trash().await.unwrap();
            let f = probe.lock().unwrap();
            assert_eq!(f.expunged_sets, vec!["1,2".to_string()]);
            assert_eq!(f.created_mailboxes, vec!["Trash".to_string()]);
            // CREATE reuses the INBOX selection from the expunge lease.
            assert_eq!(f.select_calls, vec!["INBOX".to_string()]);
        });
    }

    #[test]
    fn create_mailbox_in_passes_wire_name_byte_identical() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            // Raw ASCII hierarchy passes through untouched.
            manager.create_mailbox_in("Pai/Filho").await.unwrap();
            // Non-ASCII leaf arrives already encoded by the caller —
            // the manager never encodes inside (T-11-01 wire-name rule).
            let wire = super::super::mutf7::encode_modified_utf7("Lixeira & Cia");
            manager.create_mailbox_in(&wire).await.unwrap();
            let f = probe.lock().unwrap();
            assert_eq!(f.created_mailboxes, vec!["Pai/Filho".to_string(), wire]);
            // CREATE reuses the stable INBOX lease — one SELECT total.
            assert_eq!(f.select_calls, vec!["INBOX".to_string()]);
        });
    }

    #[test]
    fn create_mailbox_in_never_retries_refused() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            {
                probe.lock().unwrap().fail_create_refused = true;
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            let err = manager
                .create_mailbox_in("Pai")
                .await
                .expect_err("refused CREATE must surface");
            assert!(
                matches!(err, super::super::SyncError::Refused(_)),
                "expected Refused, got: {err}"
            );
            // No new lease attempt: exactly one verb call, no retry.
            let f = probe.lock().unwrap();
            assert_eq!(f.created_mailboxes.len(), 1);
        });
    }

    #[test]
    fn create_mailbox_in_retry_goes_through_reconnect() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            {
                probe.lock().unwrap().fail_create = true;
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            // Offline the reconnect cannot succeed (test.invalid dials
            // nothing): the retry path surfaces the reconnect error, and
            // crucially the verb fired exactly once — the second CREATE
            // only runs after a successful reconnect.
            let err = manager
                .create_mailbox_in("Pai")
                .await
                .expect_err("offline retry must surface the reconnect error");
            let msg = err.to_string().to_lowercase();
            assert!(
                msg.contains("reconnect") || msg.contains("connect"),
                "expected a reconnect/connect error, got: {err}"
            );
            let f = probe.lock().unwrap();
            assert_eq!(f.created_mailboxes, vec!["Pai".to_string()]);
        });
    }

    #[test]
    fn rename_and_delete_pass_wire_names_byte_identical() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            manager.rename_mailbox_in("Pai", "Novo").await.unwrap();
            manager.delete_mailbox_in("Velha").await.unwrap();
            let f = probe.lock().unwrap();
            assert_eq!(
                f.renamed_calls,
                vec![("Pai".to_string(), "Novo".to_string())]
            );
            assert_eq!(f.deleted_mailboxes, vec!["Velha".to_string()]);
            // RENAME leases the old name, DELETE the target name.
            assert_eq!(f.select_calls, vec!["Pai".to_string(), "Velha".to_string()]);
        });
    }

    #[test]
    fn rename_and_delete_never_retry_refused() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            {
                let mut f = probe.lock().unwrap();
                f.fail_rename_refused = true;
                f.fail_delete_refused = true;
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            let err = manager
                .rename_mailbox_in("Pai", "Novo")
                .await
                .expect_err("refused RENAME must surface");
            assert!(matches!(err, super::super::SyncError::Refused(_)));
            let err = manager
                .delete_mailbox_in("Velha")
                .await
                .expect_err("refused DELETE must surface");
            assert!(matches!(err, super::super::SyncError::Refused(_)));
            // No new lease attempts: exactly one verb call each, no retry.
            let f = probe.lock().unwrap();
            assert_eq!(f.renamed_calls.len(), 1);
            assert_eq!(f.deleted_mailboxes.len(), 1);
        });
    }

    #[test]
    fn rename_and_delete_retry_goes_through_reconnect() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            {
                let mut f = probe.lock().unwrap();
                f.fail_rename = true;
                f.fail_delete = true;
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            // Offline the reconnect cannot succeed: the retry path surfaces
            // the reconnect error with exactly one verb call each.
            for err in [
                manager.rename_mailbox_in("Pai", "Novo").await.expect_err("offline"),
                manager.delete_mailbox_in("Velha").await.expect_err("offline"),
            ] {
                let msg = err.to_string().to_lowercase();
                assert!(
                    msg.contains("reconnect") || msg.contains("connect"),
                    "expected a reconnect/connect error, got: {err}"
                );
            }
            let f = probe.lock().unwrap();
            assert_eq!(f.renamed_calls.len(), 1);
            assert_eq!(f.deleted_mailboxes.len(), 1);
        });
    }

    #[test]
    fn rename_rejects_uidvalidity_bump_as_state_error() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            {
                probe.lock().unwrap().bump_status_validity = true;
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            // Pre-rename STATUS returns 101, post-rename 102 — a bump the
            // caller must never silently accept (epoch rule).
            let err = manager
                .rename_mailbox_in("Pai", "Novo")
                .await
                .expect_err("UIDVALIDITY bump must surface");
            let msg = err.to_string();
            assert!(
                matches!(err, super::super::SyncError::State(_)) && msg.contains("UIDVALIDITY"),
                "expected a State UIDVALIDITY error, got: {err}"
            );
            let f = probe.lock().unwrap();
            assert_eq!(f.renamed_calls.len(), 1, "bump detected after the verb, not instead of it");
        });
    }

    #[test]
    fn mailbox_status_carries_canned_message_count() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1"], &[]);
            let probe = fake.0.clone();
            {
                probe.lock().unwrap().status_messages = 7;
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            // The MESSAGES datum flows from the session stub through the
            // manager untouched — the delete guard reads it here.
            let status = manager.mailbox_status("INBOX").await.unwrap();
            assert_eq!(status.messages, 7);
        });
    }

    #[test]
    fn move_fallback_prefers_uid_move() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS", "MOVE"], &[1, 2]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            let outcome = manager.move_message_in("INBOX", "1,2", "Trash").await.unwrap();
            assert_eq!(outcome, MoveOutcome { used_fallback: false });
            let f = probe.lock().unwrap();
            assert_eq!(f.moved_calls, vec![("1,2".to_string(), "Trash".to_string())]);
            assert!(f.copied_calls.is_empty());
            assert!(f.deleted_calls.is_empty());
            assert!(f.expunged_sets.is_empty());
            assert_eq!(f.plain_expunge_calls, 0);
            // One SELECT for the whole op.
            assert_eq!(f.select_calls, vec!["INBOX".to_string()]);
        });
    }

    #[test]
    fn move_fallback_copy_store_uid_expunge() {
        run(async {
            // UIDPLUS without MOVE: COPY + STORE + scoped UID EXPUNGE.
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS"], &[1, 2, 9]);
            let probe = fake.0.clone();
            {
                // UID 9 carries another client's `\Deleted` — it must survive
                // our scoped expunge.
                probe.lock().unwrap().deleted.insert(9, true);
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            let outcome = manager.move_message_in("INBOX", "1,2", "Archive").await.unwrap();
            assert_eq!(outcome, MoveOutcome { used_fallback: true });
            let f = probe.lock().unwrap();
            assert_eq!(f.moved_calls.len(), 0);
            assert_eq!(f.copied_calls, vec![("1,2".to_string(), "Archive".to_string())]);
            assert_eq!(f.deleted_calls, vec![(1, true), (2, true)]);
            assert_eq!(f.expunged_sets, vec!["1,2".to_string()]);
            assert_eq!(f.plain_expunge_calls, 0, "no bare EXPUNGE with UIDPLUS");
            assert_eq!(f.select_calls, vec!["INBOX".to_string()]);
            assert_eq!(f.removed, vec![1, 2], "only our UIDs removed");
            assert_eq!(f.deleted.get(&9), Some(&true), "foreign mark survives");
        });
    }

    #[test]
    fn move_fallback_neither_runs_unmark_dance() {
        run(async {
            // Neither MOVE nor UIDPLUS: COPY + STORE + unmark dance + EXPUNGE.
            let fake = FakeHandle::new(&["IMAP4rev1"], &[1, 2, 4]);
            let probe = fake.0.clone();
            {
                probe.lock().unwrap().deleted.insert(4, true);
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            let outcome = manager.move_message_in("INBOX", "1,2", "Trash").await.unwrap();
            assert_eq!(outcome, MoveOutcome { used_fallback: true });
            let f = probe.lock().unwrap();
            assert_eq!(f.copied_calls, vec![("1,2".to_string(), "Trash".to_string())]);
            // Ours marked; foreign 4 unmarked then restored.
            assert!(f.deleted_calls.contains(&(1, true)));
            assert!(f.deleted_calls.contains(&(2, true)));
            assert!(f.deleted_calls.contains(&(4, false)), "foreign unmarked: {:?}", f.deleted_calls);
            assert!(f.deleted_calls.contains(&(4, true)), "foreign restored: {:?}", f.deleted_calls);
            assert_eq!(f.plain_expunge_calls, 1);
            assert!(f.expunged_sets.is_empty(), "no UID EXPUNGE without UIDPLUS");
            assert_eq!(f.select_calls, vec!["INBOX".to_string()], "single SELECT held");
            assert_eq!(f.removed, vec![1, 2], "foreign UID 4 survives the dance");
            assert_eq!(f.deleted.get(&4), Some(&true), "foreign mark restored");
        });
    }

    #[test]
    fn move_fallback_move_without_uidplus_skips_dance() {
        run(async {
            // MOVE without UIDPLUS still moves in one verb — no expunge path.
            let fake = FakeHandle::new(&["IMAP4rev1", "MOVE"], &[1, 2]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            let outcome = manager.move_message_in("INBOX", "1,2", "Trash").await.unwrap();
            assert_eq!(outcome, MoveOutcome { used_fallback: false });
            let f = probe.lock().unwrap();
            assert_eq!(f.moved_calls.len(), 1);
            assert_eq!(f.plain_expunge_calls, 0);
            assert!(f.expunged_sets.is_empty());
        });
    }

    #[test]
    fn move_fallback_chunks_large_sets() {
        run(async {
            let uids: Vec<u32> = (1..=450).collect();
            let set = uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
            // MOVE path: 200 + 200 + 50.
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS", "MOVE"], &uids);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            manager.move_message_in("INBOX", &set, "Trash").await.unwrap();
            {
                let f = probe.lock().unwrap();
                assert_eq!(f.moved_calls.len(), 3);
                assert_eq!(f.moved_calls[0].0.split(',').count(), 200);
                assert_eq!(f.moved_calls[2].0.split(',').count(), 50);
            }
            // Fallback COPY path chunks identically.
            let fake2 = FakeHandle::new(&["IMAP4rev1", "UIDPLUS"], &uids);
            let probe2 = fake2.0.clone();
            let manager2 = SessionManager::for_test_session(Box::new(fake2));
            manager2.move_message_in("INBOX", &set, "Trash").await.unwrap();
            let f2 = probe2.lock().unwrap();
            assert_eq!(f2.copied_calls.len(), 3);
            assert_eq!(f2.expunged_sets.len(), 3);
            assert_eq!(f2.deleted_calls.len(), 450);
        });
    }

    #[test]
    fn move_fallback_rejects_malformed_uid_set() {
        run(async {
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS", "MOVE"], &[1]);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            assert!(manager.move_message_in("INBOX", "", "Trash").await.is_err());
            assert!(manager.move_message_in("INBOX", "1,,2", "Trash").await.is_err());
            // Fail-closed: nothing reached the wire.
            let f = probe.lock().unwrap();
            assert!(f.moved_calls.is_empty());
            assert!(f.copied_calls.is_empty());
        });
    }

    #[test]
    fn lease_hold_single_select_across_chunked_fallback() {
        run(async {
            // 450 UIDs through the COPY + STORE + UID EXPUNGE fallback:
            // 3 COPY + 450 STORE + 3 EXPUNGE verbs, yet exactly ONE SELECT
            // — the lease is held across the whole sequence, so a second
            // SELECT can never drift the expunge to the wrong folder.
            let uids: Vec<u32> = (1..=450).collect();
            let set = uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS"], &uids);
            let probe = fake.0.clone();
            let manager = SessionManager::for_test_session(Box::new(fake));
            let outcome = manager.move_message_in("INBOX", &set, "Archive").await.unwrap();
            assert_eq!(outcome, MoveOutcome { used_fallback: true });
            let f = probe.lock().unwrap();
            assert_eq!(f.copied_calls.len(), 3);
            assert_eq!(f.deleted_calls.len(), 450);
            assert_eq!(f.expunged_sets.len(), 3);
            assert_eq!(
                f.select_calls,
                vec!["INBOX".to_string()],
                "intermediate SELECT inside fallback: {:?}",
                f.select_calls
            );
        });
    }

    #[test]
    fn move_never_deadlocks_on_reentrant_lease() {
        run(async {
            // Any `self.lease_for()` call inside `move_message_in` while the
            // sequence lease is held would wedge the async mutex forever
            // (non-reentrant). The timeout turns that deadlock into a loud
            // failure instead of a hung suite.
            let fake = FakeHandle::new(&["IMAP4rev1"], &[1, 2]);
            let manager = SessionManager::for_test_session(Box::new(fake));
            let outcome = async_std::future::timeout(
                std::time::Duration::from_secs(5),
                manager.move_message_in("INBOX", "1,2", "Trash"),
            )
            .await
            .expect("move_message_in deadlocked — re-entrant lease_for?");
            assert!(outcome.is_ok());
        });
    }

    #[test]
    fn unmark_dance_refuses_when_unverifiable() {
        run(async {
            // UID 4 ignores the `-FLAGS` unmark (sticky): the verify re-read
            // still sees `\Deleted`, so the dance must refuse LOUDLY —
            // no bare EXPUNGE, nothing removed.
            let fake = FakeHandle::new(&["IMAP4rev1"], &[1, 2, 4]);
            let probe = fake.0.clone();
            {
                let mut f = probe.lock().unwrap();
                f.deleted.insert(4, true);
                f.sticky_deleted.insert(4);
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            let err = manager
                .move_message_in("INBOX", "1,2", "Trash")
                .await
                .expect_err("unverifiable dance must refuse");
            let msg = err.to_string();
            assert!(
                msg.contains("UID EXPUNGE"),
                "plain-language refusal, got: {msg}"
            );
            let f = probe.lock().unwrap();
            assert_eq!(f.plain_expunge_calls, 0, "blind expunge forbidden");
            assert!(f.removed.is_empty(), "nothing removed on refusal");
            assert!(f.expunged_sets.is_empty());
            // COPY + mark legs ran before the refusal point (retry replays
            // the whole sequence — 10-03 replay owns idempotence).
            assert_eq!(f.copied_calls.len(), 1);
        });
    }

    #[test]
    fn foreign_deleted_survive_scoped_uid_expunge() {
        run(async {
            // UIDPLUS path with two foreign marks (9, 10): the scoped
            // `UID EXPUNGE "1,2"` removes exactly our set; foreign marks
            // are never unmarked, never touched.
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS"], &[1, 2, 9, 10]);
            let probe = fake.0.clone();
            {
                let mut f = probe.lock().unwrap();
                f.deleted.insert(9, true);
                f.deleted.insert(10, true);
            }
            let manager = SessionManager::for_test_session(Box::new(fake));
            manager.move_message_in("INBOX", "1,2", "Trash").await.unwrap();
            let f = probe.lock().unwrap();
            assert_eq!(f.removed, vec![1, 2]);
            assert_eq!(f.deleted.get(&9), Some(&true));
            assert_eq!(f.deleted.get(&10), Some(&true));
            // No unmark/restore traffic at all on the scoped path.
            assert_eq!(f.deleted_calls, vec![(1, true), (2, true)]);
            assert_eq!(f.plain_expunge_calls, 0);
        });
    }

    #[test]
    fn lease_and_destructive_op_serialize_on_one_session() {
        run(async {
            // Plan 10-02 single-flight contention: a sync-style lease held
            // across a pass serializes a concurrent destructive op — no
            // second connection, one shared session, one SELECT. (The
            // `start_sync`-under-lease path holds exactly such a lease.)
            let fake = FakeHandle::new(&["IMAP4rev1", "UIDPLUS"], &[1, 2]);
            let probe = fake.0.clone();
            let manager = Arc::new(SessionManager::for_test_session(Box::new(fake)));
            let holder_mgr = manager.clone();
            let holder = async move {
                let _lease = holder_mgr.lease_for("INBOX").await.unwrap();
                async_std::task::sleep(std::time::Duration::from_millis(300)).await;
            };
            let writer_mgr = manager.clone();
            let writer = async move {
                writer_mgr.mark_deleted_in("INBOX", 1, true).await.unwrap();
            };
            let start = std::time::Instant::now();
            futures::join!(holder, writer);
            let elapsed = start.elapsed();
            assert!(
                elapsed >= std::time::Duration::from_millis(200),
                "destructive op did not wait for the held lease: {elapsed:?}"
            );
            let f = probe.lock().unwrap();
            // One session throughout: a single SELECT, a single CAPABILITY.
            assert_eq!(f.select_calls, vec!["INBOX".to_string()]);
            assert_eq!(f.caps_calls, 1);
            assert_eq!(f.deleted_calls, vec![(1, true)]);
        });
    }
}
