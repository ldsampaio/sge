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
    /// Envelope FETCH calls issued this pass (Phase 9 convergence signal).
    pub fetched: usize,
    /// Gap re-fetch batches issued this pass (Phase 9 range-diff).
    pub gap_refetches: usize,
    /// `true` when the sweep was skipped: server UID set already matched
    /// local with no epoch bump and no pending outbox ops — two consecutive
    /// `converged` passes issue zero message FETCHes (Phase 9).
    pub converged: bool,
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

/// Single-flight guard for sync passes (Phase 8).
///
/// One sync runs at a time on the single session: a poll tick or manual
/// refresh that arrives mid-sync (or mid flag-STORE replay) gets `None`
/// from [`try_begin`](SyncGate::try_begin) and must skip instead of
/// overlapping. The [`SyncGuard`] releases on drop, so even a panicking
/// or early-returning pass cannot wedge the gate shut.
pub struct SyncGate {
    in_flight: std::sync::atomic::AtomicBool,
}

impl Default for SyncGate {
    fn default() -> Self {
        Self {
            in_flight: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl SyncGate {
    /// Begin a pass. Returns `None` when another pass is in flight — the
    /// caller must skip (never queue or overlap).
    pub fn try_begin(&self) -> Option<SyncGuard<'_>> {
        if self
            .in_flight
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            Some(SyncGuard { gate: self })
        } else {
            None
        }
    }
}

/// RAII release for [`SyncGate`]: drop ends the pass.
pub struct SyncGuard<'a> {
    gate: &'a SyncGate,
}

impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        self.gate
            .in_flight
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second begin while a pass is in flight returns `None` (skip, never overlap).
    #[test]
    fn poll_guard_second_call_returns_busy() {
        let gate = SyncGate::default();
        let _first = gate.try_begin().expect("first pass begins");
        assert!(
            gate.try_begin().is_none(),
            "poll tick mid-sync must skip, not overlap"
        );
    }

    /// Dropping the guard (pass end, including early return) re-opens the
    /// gate — the next poll tick proceeds. This is the timer-path contract:
    /// ticks never overlap, and a finished pass never wedges future ticks.
    #[test]
    fn poll_timer_next_tick_proceeds_after_release() {
        let gate = SyncGate::default();
        {
            let _pass = gate.try_begin().expect("tick 1 begins");
            assert!(gate.try_begin().is_none(), "tick 2 mid-pass skips");
        } // `_pass` drops here — pass ends
        assert!(
            gate.try_begin().is_some(),
            "tick 3 after pass end must proceed"
        );
    }
}

pub mod worker;
pub mod bodies;
