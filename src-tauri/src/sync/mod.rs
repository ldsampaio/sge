//! Sync engine: header sweep → delta detect → body fetch → local store.
//!
//! Phase 2 lives here.  The engine is transport-agnostic — it drives
//! an IMAP session through the [`SyncSession`] trait (real
//! `BoxedSession` in production, `MockSession` in tests).

use std::sync::Arc;
use serde::Serialize;

/// Progress event emitted at each stage of a sync pass.
#[derive(Debug, Clone, Serialize)]
pub enum SyncEvent {
    /// A 200-UID header sweep batch began.
    BatchStarted { range: String, server_total: u32 },

    /// One message was inserted or updated.
    MessageSynced { uid: u32, flag: SyncFlag },

    /// A sweep batch finished.
    BatchCompleted { new: usize, updated: usize, deleted: usize },

    /// The full pass completed.
    SyncCompleted { summary: SyncSummary },

    /// A non-fatal error occurred (e.g. single-message fetch failed).
    SyncError { detail: String },
}

/// Distinguishes new vs. updated for a single message event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SyncFlag {
    New,
    Updated,
}

/// Aggregate result of one sync pass.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SyncSummary {
    /// Messages not previously seen.
    pub new: usize,
    /// Messages already seen, updated.
    pub updated: usize,
    /// Messages unchanged (envelope matched server bytes).
    pub unchanged: usize,
    /// Messages deleted from the server (wiped from local store).
    pub deleted: usize,
    /// `true` when UIDVALIDITY changed and a full wipe+resync occurred.
    pub uid_validity_bump: bool,
}

impl SyncSummary {
    /// Total messages now in the local store for this mailbox.
    pub fn total(&self) -> usize {
        self.new + self.updated + self.unchanged
    }
}

/// Thread-safe callback for streaming sync progress to the frontend
/// (via Tauri `emit` or a test assertion sink).
pub type SyncCallback = Arc<dyn Fn(SyncEvent) + Send + Sync>;

pub mod worker;
