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

use super::{AccountConfig, BoxedSession, SyncError, SyncSession};
use crate::imap::session::connect_sync;

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
    session: Option<BoxedSession>,
    selected_mailbox: Option<String>,
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
            guard.session = Some(session);
            guard.selected_mailbox = None;
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
        guard.session = Some(session);
        guard.selected_mailbox = None;
        Ok(())
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

    /// `LIST "" "*"` through the owned session (FOLD-01 folder discovery).
    pub async fn list_mailboxes(&self) -> Result<Vec<super::MailboxInfo>, SyncError> {
        let mut lease = self.lease_for("INBOX").await?;
        lease.session().list_mailboxes().await
    }

    /// `STATUS <mailbox> (UIDVALIDITY UIDNEXT UNSEEN)` through the owned
    /// session (FOLD-02 triage signals). Does not disturb the lease's
    /// SELECTed folder — STATUS works on any mailbox.
    pub async fn mailbox_status(
        &self,
        mailbox: &str,
    ) -> Result<super::MailboxStatus, SyncError> {
        let mut lease = self.lease_for("INBOX").await?;
        lease.session().mailbox_status(mailbox).await
    }
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
    pub fn session(&mut self) -> &mut BoxedSession {
        self.guard
            .session
            .as_mut()
            .expect("lease guarantees a live session")
    }
}
