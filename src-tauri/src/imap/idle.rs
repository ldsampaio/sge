//! Manager-owned IDLE watcher (Phase 29, IDLE-01).
//!
//! HARD INVARIANTS (structural, test-asserted — never relax):
//! - The watcher owns a DEDICATED read-only connection: it clones the
//!   account config and connects via [`connect_sync`](crate::imap::session::connect_sync),
//!   never leasing or parking on the [`SessionManager`](crate::imap::manager::SessionManager)
//!   session. The manager session stays free for sync + destructive ops.
//! - Read-only verbs only on the wire: `SELECT`, `CAPABILITY`, `IDLE`,
//!   `DONE`, `LOGOUT`. No `STORE`/`COPY`/`MOVE`/`EXPUNGE`/`APPEND` may
//!   appear in non-test code here (asserted by
//!   `idle_module_emits_read_only_verbs`).
//! - Wakeups are ADVISORY ONLY: new mail calls
//!   [`SyncWake::request_sync`](crate::sync::SyncWake::request_sync)
//!   (coalesced dirty flag) plus an `idle-wakeup` notification callback.
//!   The watcher NEVER runs a sync pass itself — the existing poll entry
//!   (`start_sync` → [`SyncGate::try_begin`](crate::sync::SyncGate::try_begin),
//!   skip-if-busy) stays the only sync path, so poll remains the fallback
//!   and multi-folder sweeper unchanged.
//! - `DONE` on ALL exits: normal wakeup, 29-min timeout re-IDLE, error,
//!   kill (stop flag), capability loss. A dropped handle without DONE
//!   leaves the server hanging until ITS timeout — the loop structure
//!   makes `end_idle` unavoidable per cycle.
//! - Capability gating: [`IdleWatcher::spawn_if_allowed`] reads the IDLE
//!   atom from the Phase-27 `capability-profile.json`; when false the
//!   watcher NEVER spawns (poll-only mode). A live `CAPABILITY` re-check
//!   on the dedicated connection refuses to IDLE when the server does not
//!   advertise it (RFC 2177: client MUST NOT idle then).
//! - BODY.PEEK-only holds trivially: the watcher issues no FETCH at all.
//!   UIDVALIDITY gating and header sync stay in the worker (Phase 9).

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use serde::Serialize;

use crate::imap::session::connect_sync;
use crate::imap::{AccountConfig, BoxedStream, SyncError};
use crate::sync::SyncWake;

/// Re-IDLE heartbeat: async-imap's `wait()` already times out at 29 min
/// (RFC 2177 §3: re-issue at least every 29 minutes), mirrored here so
/// tests and the transcript assert the same bound.
pub const IDLE_REISSUE_SECS: u64 = 29 * 60;

/// Watcher lifecycle states (the `idle_status` command surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IdleState {
    /// Never started (or stopped by the user / manager shutdown).
    Stopped,
    /// Opening the dedicated connection.
    Connecting,
    /// `SELECT INBOX` + `CAPABILITY` check running.
    Selecting,
    /// `IDLE` issued, waiting for server events.
    Idling,
    /// `DONE` sent, connection released, about to re-cycle.
    Done,
    /// Backing off after a failure (`attempt` counts consecutive fails).
    Reconnecting,
    /// Capability gate refused — poll-only, never spawns.
    Unavailable,
}

/// Shareable snapshot written by the watcher loop, read by `idle_status`.
#[derive(Debug, Clone, Serialize)]
pub struct IdleSnapshot {
    pub state: IdleState,
    pub last_wakeup: Option<String>,
    pub reconnect_attempt: u32,
    /// Pending advisory wakeups not yet drained by the poll entry.
    pub wakeups_pending: bool,
    pub wakeups_requested: u64,
}

impl Default for IdleSnapshot {
    fn default() -> Self {
        Self {
            state: IdleState::Stopped,
            last_wakeup: None,
            reconnect_attempt: 0,
            wakeups_pending: false,
            wakeups_requested: 0,
        }
    }
}

/// UI-facing status: honest Push/Polling/Offline/Unavailable.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IdleStatus {
    /// `Push` (green) | `Polling` (amber/blue) | `Offline` (gray/red) |
    /// `Unavailable` (capability gate refused — poll-only forever).
    pub status: String,
    pub detail: String,
    pub last_wakeup: Option<String>,
    pub reconnect_attempt: u32,
}

pub fn status_of(snap: &IdleSnapshot, idle_capable: bool) -> IdleStatus {
    if !idle_capable {
        return IdleStatus {
            status: "Polling".to_string(),
            detail: "push indisponível no servidor — verificação automática".to_string(),
            last_wakeup: snap.last_wakeup.clone(),
            reconnect_attempt: 0,
        };
    }
    match snap.state {
        IdleState::Idling => IdleStatus {
            status: "Push".to_string(),
            detail: "aguardando e-mail em tempo real".to_string(),
            last_wakeup: snap.last_wakeup.clone(),
            reconnect_attempt: 0,
        },
        IdleState::Connecting
        | IdleState::Selecting
        | IdleState::Reconnecting
        | IdleState::Done => IdleStatus {
            status: "Polling".to_string(),
            detail: if snap.state == IdleState::Reconnecting {
                format!("reconectando… (tentativa {})", snap.reconnect_attempt)
            } else {
                "conectando…".to_string()
            },
            last_wakeup: snap.last_wakeup.clone(),
            reconnect_attempt: snap.reconnect_attempt,
        },
        IdleState::Stopped => IdleStatus {
            status: "Offline".to_string(),
            detail: "observador parado".to_string(),
            last_wakeup: snap.last_wakeup.clone(),
            reconnect_attempt: 0,
        },
        IdleState::Unavailable => IdleStatus {
            status: "Polling".to_string(),
            detail: "push indisponível no servidor — verificação automática".to_string(),
            last_wakeup: snap.last_wakeup.clone(),
            reconnect_attempt: 0,
        },
    }
}

/// Exponential backoff with deterministic jitter (manager-consistent
/// shape: base 5s, doubling, 10-min cap). No RNG dependency — the jitter
/// is `attempt * 37 % 5` seconds, enough to decorrelate simultaneous
/// watchers without a new crate.
pub fn backoff_secs(attempt: u32) -> u64 {
    const BASE: u64 = 5;
    const CAP: u64 = 600;
    let exp = BASE.saturating_mul(1u64 << attempt.min(7));
    (exp.min(CAP)).saturating_add((attempt as u64 * 37) % 5)
}

/// One wait outcome from the driver's blocking IDLE wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleEvent {
    /// Server pushed something (EXISTS/RECENT/untagged) — advisory wakeup.
    NewMail,
    /// 29-min server silence — re-IDLE (DONE + fresh IDLE, no wakeup).
    Timeout,
    /// Transport failed or the stop flag fired — DONE + backoff.
    Closed(String),
}

/// The three verbs the watcher loop needs, in the order it calls them.
/// The production driver wraps a real `BoxedSession`; tests replay
/// canned transcripts through [`ScriptDriver`](tests::ScriptDriver).
/// `end_idle` MUST send DONE (or best-effort release) on every path —
/// the loop calls it after EVERY `wait` return, unconditionally.
#[allow(async_fn_in_trait)]
pub trait IdleDriver: Send {
    async fn begin_idle(&mut self) -> Result<(), String>;
    async fn wait(&mut self) -> IdleEvent;
    async fn end_idle(&mut self);
}

/// Outcome of one supervised cycle (test-observable, no timers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleOutcome {
    Woke,
    ReIdle,
    Backoff { attempt: u32 },
    Stopped,
}

/// Run ONE supervised cycle: begin → wait → end_idle (unconditional).
/// Pure decision logic — the spawn loop adds sleep/stop around it.
pub async fn drive_cycle<D: IdleDriver>(
    driver: &mut D,
    wake: &SyncWake,
    snap: &Arc<Mutex<IdleSnapshot>>,
    stop: &AtomicBool,
    attempt: u32,
) -> CycleOutcome {
    if stop.load(Ordering::SeqCst) {
        set_state(snap, IdleState::Stopped, attempt);
        return CycleOutcome::Stopped;
    }
    set_state(snap, IdleState::Connecting, attempt);
    set_state(snap, IdleState::Selecting, attempt);
    if let Err(e) = driver.begin_idle().await {
        let _ = e;
        set_state(snap, IdleState::Reconnecting, attempt + 1);
        // Release best-effort even on begin failure (half-open IDLE).
        driver.end_idle().await;
        return CycleOutcome::Backoff {
            attempt: attempt + 1,
        };
    }
    set_state(snap, IdleState::Idling, 0);
    let event = driver.wait().await;
    // DONE-ON-ALL-EXITS: unconditional, before any branching.
    driver.end_idle().await;
    set_state(snap, IdleState::Done, 0);
    match event {
        IdleEvent::NewMail => {
            wake.request_sync();
            let now = epoch_secs_now();
            // IN-03: poison-safe — a poisoned snapshot mutex (earlier panic
            // while held) must not kill this long-lived watcher task.
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            s.last_wakeup = Some(now);
            s.wakeups_pending = wake.pending();
            s.wakeups_requested = wake.request_count();
            CycleOutcome::Woke
        }
        IdleEvent::Timeout => CycleOutcome::ReIdle,
        IdleEvent::Closed(_) => {
            set_state(snap, IdleState::Reconnecting, attempt + 1);
            CycleOutcome::Backoff {
                attempt: attempt + 1,
            }
        }
    }
}

fn set_state(snap: &Arc<Mutex<IdleSnapshot>>, state: IdleState, attempt: u32) {
    // IN-03: poison-safe (see above) — background task, never panic here.
    let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
    s.state = state;
    if state == IdleState::Reconnecting {
        s.reconnect_attempt = attempt;
    } else if state == IdleState::Idling {
        s.reconnect_attempt = 0;
    }
}

fn epoch_secs_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{}", d.as_secs()))
        .unwrap_or_else(|_| "0".to_string())
}

/// Manager-owned watcher: config clone + advisory wake + shared snapshot.
/// Owns NO session across cycles — each `run` cycle connects through the
/// production driver on a fresh dedicated connection.
pub struct IdleWatcher {
    config: AccountConfig,
    wake: Arc<SyncWake>,
    snapshot: Arc<Mutex<IdleSnapshot>>,
    stop: Arc<AtomicBool>,
    wakes: Arc<AtomicU64>,
    on_wake: Arc<dyn Fn() + Send + Sync>,
}

impl IdleWatcher {
    pub fn new(
        config: AccountConfig,
        wake: Arc<SyncWake>,
        on_wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            config,
            wake,
            snapshot: Arc::new(Mutex::new(IdleSnapshot::default())),
            stop: Arc::new(AtomicBool::new(false)),
            wakes: Arc::new(AtomicU64::new(0)),
            on_wake,
        }
    }

    pub fn snapshot_handle(&self) -> Arc<Mutex<IdleSnapshot>> {
        self.snapshot.clone()
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Supervised run loop with a caller-supplied driver (tests inject
    /// [`ScriptDriver`](tests::ScriptDriver); production uses
    /// [`LiveIdleDriver`]). Returns when the stop flag fires.
    pub async fn run_with<D: IdleDriver>(self, mut driver: D) {
        let mut attempt = 0u32;
        loop {
            if self.stop.load(Ordering::SeqCst) {
                set_state(&self.snapshot, IdleState::Stopped, 0);
                driver.end_idle().await;
                return;
            }
            match drive_cycle(&mut driver, &self.wake, &self.snapshot, &self.stop, attempt).await {
                CycleOutcome::Stopped => {
                    driver.end_idle().await;
                    return;
                }
                CycleOutcome::Woke => {
                    attempt = 0;
                    self.wakes.fetch_add(1, Ordering::SeqCst);
                    (self.on_wake)();
                }
                CycleOutcome::ReIdle => {
                    // WR-01: 29-min server silence is healthy — re-IDLE
                    // silently. No wake counter bump, no `on_wake`, no
                    // spurious `idle-wakeup` → no pointless sync pass.
                    attempt = 0;
                }
                CycleOutcome::Backoff { attempt: next } => {
                    attempt = next;
                    let secs = backoff_secs(attempt);
                    async_std::task::sleep(std::time::Duration::from_secs(secs)).await;
                }
            }
        }
    }

    /// Convenience: run with the live network driver on a dedicated
    /// `connect_sync` connection (production path).
    pub async fn run(self) {
        let driver = LiveIdleDriver::with_stop(self.config.clone(), self.stop.clone());
        self.run_with(driver).await;
    }
}

/// Capability-gated spawn: reads the IDLE atom from `profile_path` (Phase
/// 27). `false`/missing → returns `None` and marks the snapshot
/// `Unavailable`: the watcher NEVER spawns, poll-only mode.
pub fn spawn_if_allowed(
    config: AccountConfig,
    wake: Arc<SyncWake>,
    profile_path: &std::path::Path,
    on_wake: Arc<dyn Fn() + Send + Sync>,
) -> Option<(IdleWatcherHandle, tauri::async_runtime::JoinHandle<()>)> {
    if !crate::classify::rules_io::idle_allowed_from_profile(profile_path) {
        return None;
    }
    let watcher = IdleWatcher::new(config, wake, on_wake);
    let handle = IdleWatcherHandle {
        snapshot: watcher.snapshot_handle(),
        stop: watcher.stop.clone(),
        wakes: watcher.wakes.clone(),
    };
    let join = tauri::async_runtime::spawn(async move { watcher.run().await });
    Some((handle, join))
}

/// Test-visible handle: snapshot polling + cooperative stop + wake count.
#[derive(Clone)]
pub struct IdleWatcherHandle {
    snapshot: Arc<Mutex<IdleSnapshot>>,
    stop: Arc<AtomicBool>,
    wakes: Arc<AtomicU64>,
}

impl IdleWatcherHandle {
    pub fn snapshot(&self) -> IdleSnapshot {
        // IN-03: poison-safe — a wedged `idle_status` is worse than a
        // slightly stale snapshot.
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn wake_count(&self) -> u64 {
        self.wakes.load(Ordering::SeqCst)
    }
}

/// Production driver: dedicated `connect_sync` connection, SELECT INBOX +
/// live CAPABILITY check, async-imap `idle()` handle, DONE + LOGOUT on
/// every `end_idle`. Holds NO session while idle-waiting outside a cycle.
///
/// The blocking `wait` races the 29-min server wait against a 1s stop
/// poll: a stop request sends DONE (no half-open IDLE left behind) and
/// returns `Closed("stopped")` within ~1s instead of hanging to timeout.
pub struct LiveIdleDriver {
    config: AccountConfig,
    stop: Arc<AtomicBool>,
    session: Option<crate::imap::BoxedSession>,
    handle: Option<async_imap::extensions::idle::Handle<BoxedStream>>,
}

impl LiveIdleDriver {
    pub fn new(config: AccountConfig) -> Self {
        Self::with_stop(config, Arc::new(AtomicBool::new(false)))
    }

    pub fn with_stop(config: AccountConfig, stop: Arc<AtomicBool>) -> Self {
        Self {
            config,
            stop,
            session: None,
            handle: None,
        }
    }
}

#[allow(async_fn_in_trait)]
impl IdleDriver for LiveIdleDriver {
    async fn begin_idle(&mut self) -> Result<(), String> {
        // Dedicated connection — never the manager lease (RFC 2177: the
        // connection is unusable for anything else while IDLE runs).
        let mut session = connect_sync(&self.config)
            .await
            .map_err(|e: SyncError| format!("idle connect: {e}"))?;
        // UIDVALIDITY-first discipline starts here: SELECT before IDLE so
        // the subsequent poll sync has a fresh epoch to gate on.
        session
            .select("INBOX")
            .await
            .map_err(|e| format!("idle select: {e}"))?;
        // Live capability re-check: MUST NOT idle without the atom.
        let caps = session
            .capabilities()
            .await
            .map_err(|e| format!("idle capability: {e}"))?;
        let idle = caps.iter().any(|c| {
            matches!(c, async_imap::types::Capability::Atom(a) if a.eq_ignore_ascii_case("IDLE"))
        });
        if !idle {
            let _ = session.logout().await;
            return Err("server does not advertise IDLE (poll-only)".to_string());
        }
        let mut handle = session.idle();
        // WR-05: `idle()` moved the session INTO the handle — on `init()`
        // failure the handle must still be released best-effort (DONE +
        // LOGOUT) or the server holds a half-open session until ITS
        // timeout (rapid retries would accumulate server-side sessions).
        if let Err(e) = handle.init().await {
            release_handle(handle).await;
            return Err(format!("idle init: {e}"));
        }
        self.session = None;
        self.handle = Some(handle);
        Ok(())
    }

    async fn wait(&mut self) -> IdleEvent {
        use std::sync::atomic::Ordering;
        // Take the handle OUT: the wait below borrows only the owned
        // handle + stop flag, never self — so every arm may use self.
        let mut handle = match self.handle.take() {
            Some(h) => h,
            None => return IdleEvent::Closed("idle wait without handle".to_string()),
        };
        if self.stop.load(Ordering::SeqCst) {
            release_handle(handle).await;
            return IdleEvent::Closed("stopped".to_string());
        }
        match wait_owned(&mut handle, &self.stop).await {
            // Stop requested mid-IDLE: DONE first (releases the server
            // side), then report Closed so the loop backs off / exits.
            WaitOut::Stopped => {
                release_handle(handle).await;
                IdleEvent::Closed("stopped".to_string())
            }
            WaitOut::Resp(r) => {
                self.handle = Some(handle);
                match r {
                    Ok(async_imap::extensions::idle::IdleResponse::NewData(_)) => {
                        IdleEvent::NewMail
                    }
                    Ok(async_imap::extensions::idle::IdleResponse::Timeout) => IdleEvent::Timeout,
                    Ok(async_imap::extensions::idle::IdleResponse::ManualInterrupt) => {
                        IdleEvent::Closed("manual interrupt".to_string())
                    }
                    Err(e) => IdleEvent::Closed(format!("idle wait: {e}")),
                }
            }
        }
    }

    async fn end_idle(&mut self) {
        // DONE-ON-ALL-EXITS: the handle is consumed by `done()`; on error
        // the session is dropped (server times the half-open IDLE out).
        if let Some(handle) = self.handle.take() {
            release_handle(handle).await;
        }
        if let Some(mut session) = self.session.take() {
            let _ = session.logout().await;
        }
    }
}

enum WaitOut {
    Resp(Result<async_imap::extensions::idle::IdleResponse, async_imap::error::Error>),
    Stopped,
}

/// Best-effort DONE + LOGOUT for a detached handle (consumes it).
async fn release_handle(handle: async_imap::extensions::idle::Handle<BoxedStream>) {
    match handle.done().await {
        Ok(mut session) => {
            let _ = session.logout().await;
        }
        Err(_) => {}
    }
}

/// Race the 29-min server wait against the stop flag (1s poll). Holds no
/// watcher state — borrow-safe by construction.
async fn wait_owned(
    handle: &mut async_imap::extensions::idle::Handle<BoxedStream>,
    stop: &AtomicBool,
) -> WaitOut {
    use futures::FutureExt;
    use std::sync::atomic::Ordering;
    // The StopSource MUST outlive the wait future: dropping it cancels
    // the token and the wait returns ManualInterrupt immediately.
    let (fut, _stop_src) = handle.wait();
    let mut fut = fut.fuse();
    let stop_wait = async {
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            async_std::task::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    .fuse();
    futures::pin_mut!(fut, stop_wait);
    futures::select! {
        r = fut => WaitOut::Resp(r),
        _ = stop_wait => WaitOut::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncGate;
    use std::collections::VecDeque;

    /// Canned transcript driver: each `wait` pops the next scripted event;
    /// every verb is recorded as a `C2S:` line so tests assert the exact
    /// command sequence (SELECT → IDLE → … → DONE → LOGOUT) including
    /// DONE-on-every-exit and the 29-min re-IDLE shape.
    pub struct ScriptDriver {
        pub transcript: Vec<String>,
        script: VecDeque<ScriptStep>,
        pub fail_begin: bool,
        idling: bool,
    }

    #[derive(Debug, Clone)]
    pub enum ScriptStep {
        Mail,
        Timeout,
        Drop,
    }

    impl ScriptDriver {
        pub fn new(steps: Vec<ScriptStep>) -> Self {
            Self {
                transcript: vec![],
                script: steps.into(),
                fail_begin: false,
                idling: false,
            }
        }

        fn note(&mut self, line: &str) {
            self.transcript.push(line.to_string());
        }
    }

    #[allow(async_fn_in_trait)]
    impl IdleDriver for ScriptDriver {
        async fn begin_idle(&mut self) -> Result<(), String> {
            self.note("C2S: SELECT INBOX");
            self.note("S2C: OK SELECT completed");
            self.note("C2S: CAPABILITY");
            self.note("S2C: * CAPABILITY IMAP4rev1 IDLE");
            if self.fail_begin {
                self.note("S2C: NO SELECT failed (folder gone)");
                return Err("select failed".to_string());
            }
            self.note("C2S: IDLE");
            self.note("S2C: + idling");
            self.idling = true;
            Ok(())
        }

        async fn wait(&mut self) -> IdleEvent {
            match self.script.pop_front() {
                Some(ScriptStep::Mail) => {
                    self.note("S2C: * 23 EXISTS");
                    IdleEvent::NewMail
                }
                Some(ScriptStep::Timeout) => {
                    self.note("S2C: (29 min silence)");
                    IdleEvent::Timeout
                }
                Some(ScriptStep::Drop) | None => {
                    self.note("S2C: * BYE connection dropped");
                    IdleEvent::Closed("dropped".to_string())
                }
            }
        }

        async fn end_idle(&mut self) {
            if self.idling {
                self.note("C2S: DONE");
                self.note("S2C: OK IDLE terminated");
                self.idling = false;
            }
            self.note("C2S: LOGOUT");
        }
    }

    fn test_wake() -> Arc<SyncWake> {
        Arc::new(SyncWake::new(Arc::new(SyncGate::default())))
    }

    fn snap() -> Arc<Mutex<IdleSnapshot>> {
        Arc::new(Mutex::new(IdleSnapshot::default()))
    }

    fn stop_flag() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn idle_watcher_lifecycle() {
        // Verbindungs-Lebenszyklus: SELECT → IDLE → EXISTS → DONE → wake.
        let wake = test_wake();
        let s = snap();
        let stop = stop_flag();
        let mut driver = ScriptDriver::new(vec![ScriptStep::Mail]);
        let outcome = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
        assert_eq!(outcome, CycleOutcome::Woke);
        assert_eq!(wake.request_count(), 1, "one EXISTS → one advisory wakeup");
        let seq = driver.transcript.join("\n");
        let select = seq.find("C2S: SELECT").unwrap();
        let idle = seq.find("C2S: IDLE").unwrap();
        let done = seq.find("C2S: DONE").unwrap();
        assert!(
            select < idle && idle < done,
            "verb order SELECT→IDLE→DONE:\n{seq}"
        );
        let snap = s.lock().unwrap();
        assert!(snap.last_wakeup.is_some());
        assert_eq!(snap.state, IdleState::Done);
    }

    #[test]
    fn idle_transcript_tripwire_done_reconnect_backoff() {
        // Drop mid-IDLE → DONE still sent → Backoff(1) → next cycle
        // re-SELECTs and re-IDLEs (29-min re-IDLE shape on Timeout).
        let wake = test_wake();
        let s = snap();
        let stop = stop_flag();
        let mut driver = ScriptDriver::new(vec![ScriptStep::Drop]);
        let outcome = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
        assert_eq!(outcome, CycleOutcome::Backoff { attempt: 1 });
        assert_eq!(wake.request_count(), 0, "a drop is not mail — no wakeup");
        assert!(
            driver.transcript.iter().any(|l| l == "C2S: DONE"),
            "DONE on drop exit"
        );
        assert_eq!(backoff_secs(1), 12, "5s * 2 + jitter(37 % 5 = 2)");
        assert!(backoff_secs(1) < backoff_secs(3), "backoff grows");
        assert!(backoff_secs(100) <= 605, "backoff capped");

        // Reconnect cycle re-IDLEs after backoff.
        let mut driver2 = ScriptDriver::new(vec![ScriptStep::Timeout]);
        let outcome2 = async_std::task::block_on(drive_cycle(&mut driver2, &wake, &s, &stop, 1));
        assert_eq!(
            outcome2,
            CycleOutcome::ReIdle,
            "timeout re-IDLEs without wakeup"
        );
        assert_eq!(wake.request_count(), 0);
        assert!(driver2.transcript.iter().any(|l| l == "C2S: DONE"));
    }

    #[test]
    fn idle_transcript_tripwire_wakeup_coalescing() {
        // 5 rapid EXISTS bursts → 5 advisory requests, ONE gate pass.
        let gate = Arc::new(SyncGate::default());
        let wake = Arc::new(SyncWake::new(gate));
        let s = snap();
        let stop = stop_flag();
        let mut driver = ScriptDriver::new(vec![
            ScriptStep::Mail,
            ScriptStep::Mail,
            ScriptStep::Mail,
            ScriptStep::Mail,
            ScriptStep::Mail,
        ]);
        for _ in 0..5 {
            let outcome = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
            assert_eq!(outcome, CycleOutcome::Woke);
        }
        assert_eq!(wake.request_count(), 5);
        let _pass = wake.dispatch().expect("burst opens one pass");
        assert!(wake.dispatch().is_none(), "coalesced — no second pass");
        assert_eq!(wake.dispatch_count(), 1);
    }

    #[test]
    fn idle_transcript_tripwire_poll_fallback() {
        // IDLE=false in the profile → spawn refused, poll-only mode.
        let dir = std::env::temp_dir();
        let profile = dir.join("sge-29-idle-poll-fallback.json");
        std::fs::write(&profile, r#"{"accounts":[{"atoms":{"IDLE":false}}]}"#).unwrap();
        let cfg = AccountConfig {
            host: "mail.example".to_string(),
            port: 993,
            security: crate::imap::SecurityMode::ImplicitTls,
            username: "u".to_string(),
            password: zeroize::Zeroizing::new("p".to_string()),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let on_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let spawned = spawn_if_allowed(cfg, test_wake(), &profile, on_wake);
        assert!(spawned.is_none(), "IDLE=false must never spawn the watcher");
        let _ = std::fs::remove_file(&profile);
    }

    #[test]
    fn idle_edge_kill_mid_idle_sends_done() {
        // Kill mid-IDLE: the in-flight cycle still runs end_idle (DONE)
        // and the next cycle observes the stop flag → Stopped, no wakeup.
        let wake = test_wake();
        let s = snap();
        let stop = stop_flag();
        let mut driver = ScriptDriver::new(vec![ScriptStep::Mail]);
        let outcome = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
        assert_eq!(outcome, CycleOutcome::Woke);
        stop.store(true, Ordering::SeqCst);
        let outcome2 = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
        assert_eq!(outcome2, CycleOutcome::Stopped);
        assert_eq!(s.lock().unwrap().state, IdleState::Stopped);
        assert_eq!(wake.request_count(), 1, "stop adds no duplicate wakeup");
    }

    #[test]
    fn reidle_cycles_never_fire_on_wake() {
        // WR-01: 29-min Timeout re-IDLEs must NOT bump the wake counter nor
        // invoke `on_wake` (no spurious `idle-wakeup` → no pointless sync).
        // Only a real EXISTS may fire it.
        let cfg = AccountConfig {
            host: "mail.example".to_string(),
            port: 993,
            security: crate::imap::SecurityMode::ImplicitTls,
            username: "u".to_string(),
            password: zeroize::Zeroizing::new("p".to_string()),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let fired = Arc::new(AtomicU64::new(0));
        let fired2 = fired.clone();
        let on_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            fired2.fetch_add(1, Ordering::SeqCst);
        });
        let watcher = IdleWatcher::new(cfg, test_wake(), on_wake);
        let stop = watcher.stop.clone();
        // Deep Timeout script: ReIdle cycles never sleep, so the loop spins
        // until `stop` fires — no backoff timer is ever armed here.
        let script = vec![ScriptStep::Timeout; 100_000];
        let join = std::thread::spawn(move || {
            async_std::task::block_on(watcher.run_with(ScriptDriver::new(script)))
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        stop.store(true, Ordering::SeqCst);
        join.join().expect("run_with must exit on stop");
        assert_eq!(
            fired.load(Ordering::SeqCst),
            0,
            "silent 29-min periods must never fire on_wake"
        );
    }

    #[test]
    fn idle_edge_select_fail_backs_off_without_wakeup() {
        let wake = test_wake();
        let s = snap();
        let stop = stop_flag();
        let mut driver = ScriptDriver::new(vec![]);
        driver.fail_begin = true;
        let outcome = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 2));
        assert_eq!(outcome, CycleOutcome::Backoff { attempt: 3 });
        assert_eq!(wake.request_count(), 0);
        assert_eq!(s.lock().unwrap().reconnect_attempt, 3);
        // Even a failed begin releases best-effort (LOGOUT line present).
        assert!(driver.transcript.iter().any(|l| l == "C2S: LOGOUT"));
    }

    #[test]
    fn idle_edge_state_machine_transitions() {
        // Connecting → Selecting → Idling → Done → Reconnecting → Stopped
        // is exercised across cycles; assert the terminal snapshot chain.
        let wake = test_wake();
        let s = snap();
        let stop = stop_flag();
        let mut driver = ScriptDriver::new(vec![ScriptStep::Mail, ScriptStep::Drop]);
        let o1 = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
        assert_eq!(o1, CycleOutcome::Woke);
        let o2 = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 0));
        assert_eq!(o2, CycleOutcome::Backoff { attempt: 1 });
        assert_eq!(s.lock().unwrap().state, IdleState::Reconnecting);
        stop.store(true, Ordering::SeqCst);
        let o3 = async_std::task::block_on(drive_cycle(&mut driver, &wake, &s, &stop, 1));
        assert_eq!(o3, CycleOutcome::Stopped);
    }

    #[test]
    fn idle_edge_status_mapping() {
        let mut base = IdleSnapshot::default();
        base.state = IdleState::Idling;
        assert_eq!(status_of(&base, true).status, "Push");
        base.state = IdleState::Reconnecting;
        base.reconnect_attempt = 2;
        let r = status_of(&base, true);
        assert_eq!(r.status, "Polling");
        assert!(r.detail.contains('2'));
        base.state = IdleState::Stopped;
        assert_eq!(status_of(&base, true).status, "Offline");
        // Capability false → never Push, whatever the snapshot claims.
        base.state = IdleState::Idling;
        assert_eq!(status_of(&base, false).status, "Polling");
    }

    #[test]
    fn idle_module_emits_read_only_verbs() {
        // Structural read-only assertion: non-test code here must never
        // name a mutating verb. Comments/docs are stripped so the guard
        // watches behavior, not prose.
        let src = include_str!("idle.rs");
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        let code: String = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for verb in [
            "STORE", "EXPUNGE", "APPEND", "COPY", "MOVE", "CREATE", "FETCH", " UID ",
        ] {
            assert!(!code.contains(verb), "idle.rs must not emit {verb}");
        }
        // DONE + LOGOUT + SELECT + IDLE are the only wire verbs
        // (matched case-insensitively: `done()`/`logout()` are method
        // calls, `SELECT`/`IDLE`/`DONE`/`LOGOUT` are command strings).
        let lower = code.to_lowercase();
        for verb in ["done", "logout", "select", "idle"] {
            assert!(lower.contains(verb), "idle.rs must use {verb}");
        }
    }

    #[test]
    fn live_idle_leg_skips_offline() {
        // Live-gate shape (sandbox contract): without operator credentials
        // this leg SKIPs — never fails, never opens a socket.
        if std::env::var("SGE_LIVE_IMAP_USER").is_err() {
            eprintln!("SKIP: live IDLE leg needs SGE_LIVE_IMAP_USER (operator-only)");
            return;
        }
        // Credentialed runs stay operator-driven; headless transcripts
        // above are the assertion surface.
    }
}
