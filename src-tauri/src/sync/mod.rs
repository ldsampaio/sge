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
    BatchCompleted { new: usize, updated: usize, deleted: usize, moved: usize, expunged: usize },

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
    /// Delete/move outbox ops acknowledged this pass (Plan 10-03 pre-sweep
    /// replay). Both `delete` (move-to-Trash) and `move` move server-side,
    /// so every acked `imap_outbox` op counts here.
    pub moved: usize,
    /// Locally-initiated permanent deletes acked this pass. The expunge
    /// command path deletes rows directly (no sync pass), so this stays 0
    /// until a pass-attributed expunge exists — reserved, not dead.
    pub expunged: usize,
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

/// Advisory IDLE wakeup (Phase 29, IDLE-01).
///
/// The [`IdleWatcher`](crate::imap::idle::IdleWatcher) owns a dedicated
/// read-only connection and must NEVER call sync directly: it signals
/// through `request_sync`, and whoever drains (`dispatch`) still goes
/// through [`SyncGate::try_begin`] — skip-if-busy, coalesced. A burst of
/// EXISTS/RECENT untagged responses collapses to one pending wakeup
/// (`dirty` flag); a dispatch that finds the gate busy leaves the flag
/// set for the next idle moment instead of queueing work.
pub struct SyncWake {
    gate: Arc<SyncGate>,
    dirty: std::sync::atomic::AtomicBool,
    requests: std::sync::atomic::AtomicU64,
    dispatched: std::sync::atomic::AtomicU64,
}

impl SyncWake {
    pub fn new(gate: Arc<SyncGate>) -> Self {
        Self {
            gate,
            dirty: std::sync::atomic::AtomicBool::new(false),
            requests: std::sync::atomic::AtomicU64::new(0),
            dispatched: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Advisory wakeup from the IDLE loop. Cheap, infallible, coalescing:
    /// any number of calls before the next dispatch equal one pending sync.
    pub fn request_sync(&self) {
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// A wakeup is pending (set by `request_sync`, cleared by a
    /// successful `dispatch`).
    pub fn pending(&self) -> bool {
        self.dirty.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Try to turn a pending wakeup into a pass. Returns `true` only when
    /// the flag was set AND the gate opened — the caller then runs exactly
    /// one sync pass and must let the returned guard drop at pass end.
    /// A busy gate returns `false` with the flag INTACT (coalesced retry
    /// later, never a queue, never an overlap).
    pub fn dispatch(&self) -> Option<SyncGuard<'_>> {
        if !self.dirty.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let guard = self.gate.try_begin()?;
        self.dirty.store(false, std::sync::atomic::Ordering::SeqCst);
        self.dispatched
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(guard)
    }

    /// Total `request_sync` calls (diagnostics only — bursts are expected).
    pub fn request_count(&self) -> u64 {
        self.requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Passes actually opened through the gate.
    pub fn dispatch_count(&self) -> u64 {
        self.dispatched.load(std::sync::atomic::Ordering::SeqCst)
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

    /// EXISTS bursts coalesce: N rapid `request_sync` calls open exactly
    /// one pass through the gate.
    #[test]
    fn wake_burst_coalesces_to_single_dispatch() {
        let gate = Arc::new(SyncGate::default());
        let wake = SyncWake::new(gate);
        for _ in 0..5 {
            wake.request_sync();
        }
        assert!(wake.pending());
        assert_eq!(wake.request_count(), 5);
        let _pass = wake.dispatch().expect("one pass opens");
        assert!(!wake.pending(), "dispatch clears the flag");
        assert!(wake.dispatch().is_none(), "no second pass without new mail");
        assert_eq!(wake.dispatch_count(), 1);
    }

    /// A busy gate keeps the wakeup pending — no queue, no overlap, the
    /// next idle drain retries.
    #[test]
    fn wake_busy_gate_keeps_dirty_flag() {
        let gate = Arc::new(SyncGate::default());
        let wake = SyncWake::new(gate.clone());
        wake.request_sync();
        let _held = gate.try_begin().expect("hold the gate (a sync is running)");
        assert!(wake.dispatch().is_none(), "busy gate must not open");
        assert!(wake.pending(), "wakeup stays pending for later");
        drop(_held);
        assert!(wake.dispatch().is_some(), "freed gate dispatches");
        assert!(!wake.pending());
    }

    /// No wakeup, no pass: idle loops with no mail never touch the gate.
    #[test]
    fn wake_quiet_idle_never_dispatches() {
        let gate = Arc::new(SyncGate::default());
        let wake = SyncWake::new(gate);
        assert!(!wake.pending());
        assert!(wake.dispatch().is_none());
        assert_eq!(wake.dispatch_count(), 0);
    }
}

pub mod worker;
pub mod bodies;
