//! Laya sidecar supervision — Phase 15 (packaging spike, hello-world only).
//!
//! The classifier runs as a Tauri `externalBin` child process speaking
//! loopback-only HTTP. This module owns: ephemeral loopback port selection,
//! per-boot API key generation, spawn/health/restart/kill lifecycle, and a
//! `sidecar_status` IPC surface for later phases.
//!
//! Security boundaries (all enforced here, not in the UI):
//! - The sidecar NEVER binds off-loopback: [`SidecarConfig::new`] refuses
//!   non-loopback hosts, and spawn always passes `LAYA_HOST=127.0.0.1`.
//! - The per-boot key is [`zeroize::Zeroizing`]-wrapped and NEVER appears in
//!   `Display`/`Debug`/error strings or logs — only in the `Authorization`
//!   header and the child env (both memory-only, same machine).
//! - No classification calls exist yet: only `GET /health` is probed.
//!   `POST /v1/systemone` arrives in Phase 17.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Loopback-only sidecar endpoint config.
///
/// `api_key` is per-boot: generated fresh at every app launch, held in
/// memory only, never logged. Loopback binding is the real access boundary;
/// the key is per-boot namespacing so a stale prober from a previous boot
/// cannot be mistaken for the live sidecar.
pub struct SidecarConfig {
    host: String,
    port: u16,
    api_key: zeroize::Zeroizing<String>,
}

impl SidecarConfig {
    /// Build a config. Refuses non-loopback hosts — the sidecar must never
    /// be reachable off-machine, even if a caller passes a LAN IP or `0.0.0.0`.
    pub fn new(host: &str, port: u16, api_key: String) -> Result<Self, SidecarError> {
        if !crate::imap::is_loopback(host) {
            return Err(SidecarError::NonLoopbackHost(host.to_string()));
        }
        if port == 0 {
            return Err(SidecarError::InvalidPort);
        }
        Ok(Self {
            host: host.to_string(),
            port,
            api_key: zeroize::Zeroizing::new(api_key),
        })
    }

    /// Health-check URL, e.g. `http://127.0.0.1:43121/health`.
    pub fn health_url(&self) -> String {
        format!("http://{}:{}/health", self.host, self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Redacted summary for logs/status: port only, never the key.
    pub fn describe(&self) -> String {
        format!("sidecar at {}:{} (key: <redacted>)", self.host, self.port)
    }
}

/// Pick a free ephemeral port on loopback by binding `:0` and reading back
/// the assigned port. The socket is closed immediately; a later bind race is
/// handled by supervisor restart (port conflict → respawn with a fresh port).
pub fn pick_ephemeral_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Generate a 256-bit per-boot API key from OS entropy (`/dev/urandom`,
/// Linux-only — the app ships Linux-only). Hex-encoded, 64 chars.
pub fn generate_api_key() -> std::io::Result<zeroize::Zeroizing<String>> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut hex = String::with_capacity(64);
    for b in bytes {
        hex.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    Ok(zeroize::Zeroizing::new(hex))
}

// ---------------------------------------------------------------------------
// Errors (plain language, never contain key material)
// ---------------------------------------------------------------------------

/// Typed sidecar failures. Every variant renders as a plain-language string
/// safe to show in the UI and logs. The key is never interpolated.
#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("classifier sidecar is not running (spawn it by launching the app normally)")]
    NotRunning,
    #[error("classifier refused a non-loopback host — the sidecar only serves this machine")]
    NonLoopbackHost(String),
    #[error("invalid sidecar port (must be 1-65535)")]
    InvalidPort,
    #[error("classifier sidecar failed to start (binary missing or crashed on boot)")]
    SpawnFailed(String),
    #[error("classifier did not answer the health check in time (still warming up or overloaded)")]
    HealthTimeout,
    #[error("classifier answered the health check with an error (status {0})")]
    HealthFailed(u16),
    #[error("classifier keeps crashing and was stopped; mail stays unclassified until the app restarts")]
    RestartExhausted,
    #[error("sidecar I/O fault: {0}")]
    Io(String),
}

impl From<std::io::Error> for SidecarError {
    fn from(e: std::io::Error) -> Self {
        SidecarError::Io(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// Maximum consecutive crash-restarts before giving up for this boot.
pub const MAX_RESTARTS: u32 = 3;
/// Base backoff between restarts (multiplied by attempt number).
pub const RESTART_BACKOFF: Duration = Duration::from_secs(2);
/// Health-probe timeout — cold starts are slow, but a single probe must not
/// hang the caller; the background loop retries.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Supervisor status snapshot (cloneable, key-free).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state")]
pub enum SidecarStatus {
    /// Never started (or explicitly stopped).
    Stopped,
    /// Child spawned, first `/health` 200 not yet observed.
    Starting { attempts: u32 },
    /// `/health` green. `cold_start_ms` measures spawn → first 200.
    Running { cold_start_ms: u64 },
    /// Crashed more than MAX_RESTARTS times; waiting for next app launch.
    Down,
}

/// Supervises one sidecar child process.
///
/// Pure-logic parts (`record_crash`, `status`, config) are handle-free and
/// unit-tested. Actually spawning/killing needs a Tauri `AppHandle` and goes
/// through `spawn_sidecar` / `shutdown` (integration-tested in dev, not in
/// `cargo test` — no Tauri runtime in unit tests).
pub struct SidecarSupervisor {
    config: SidecarConfig,
    inner: Mutex<SupervisorInner>,
    started_at: Instant,
    /// Extra child env (resource-dir-derived: HF_HOME, HF_HUB_OFFLINE).
    /// Set once at spawn; never contains the API key.
    extra_env: Vec<(String, String)>,
}

struct SupervisorInner {
    status: SidecarStatus,
    crashes: u32,
}

impl SidecarSupervisor {
    pub fn new(config: SidecarConfig) -> Arc<Self> {
        Arc::new(Self::with_extra_env(config, Vec::new()))
    }

    /// Constructor with extra child env (weights-cache paths resolved from
    /// the Tauri resource dir at spawn time). The API key must never appear
    /// in `extra` — it travels via [`SidecarSupervisor::child_env`] only.
    pub fn with_extra_env(config: SidecarConfig, extra: Vec<(String, String)>) -> Self {
        Self {
            config,
            inner: Mutex::new(SupervisorInner {
                status: SidecarStatus::Stopped,
                crashes: 0,
            }),
            started_at: Instant::now(),
            extra_env: extra,
        }
    }

    /// Current status snapshot.
    pub fn status(&self) -> SidecarStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Loopback port (safe to log — contains no key material).
    pub fn port(&self) -> u16 {
        self.config.port()
    }

    /// Mark a successful health probe. First success transitions
    /// Starting → Running and records cold-start latency.
    pub fn record_healthy(&self) {
        let mut inner = self.inner.lock().unwrap();
        if !matches!(inner.status, SidecarStatus::Running { .. }) {
            inner.status = SidecarStatus::Running {
                cold_start_ms: self.started_at.elapsed().as_millis() as u64,
            };
        }
    }

    /// Record a crash/failure. Returns the post-crash status so the caller
    /// knows whether to restart (`Starting`) or give up (`Down`).
    ///
    /// Pure logic — unit-tested without spawning anything.
    pub fn record_crash(&self) -> SidecarStatus {
        let mut inner = self.inner.lock().unwrap();
        inner.crashes += 1;
        if inner.crashes > MAX_RESTARTS {
            inner.status = SidecarStatus::Down;
        } else {
            inner.status = SidecarStatus::Starting {
                attempts: inner.crashes,
            };
        }
        inner.status.clone()
    }

    /// Backoff before restart attempt `n` (1-based): base × n.
    pub fn backoff_for(attempt: u32) -> Duration {
        RESTART_BACKOFF * attempt.max(1)
    }

    /// Env block for the child. `laya-serve` is env-only (no CLI args):
    /// loopback bind + port + per-boot key + multilingual-only preload.
    /// The key travels here (child env, same machine) and in the
    /// `Authorization` header — never in argv (visible via /proc) or logs.
    ///
    /// `LAYA_MODELS=multilingual` + `LAYA_DEFAULT_MODEL=multilingual`:
    /// pt-BR mail must never route to the English checkpoint, including the
    /// no-language-evidence fallback (per laya README deployment guidance).
    /// Weights resolve from `HF_HOME` (resource-dir cache, offline) — see
    /// `extra_env`, set from the Tauri resource dir at spawn.
    pub fn child_env(&self) -> Vec<(String, String)> {
        let mut env = vec![
            ("LAYA_HOST".to_string(), "127.0.0.1".to_string()),
            ("LAYA_PORT".to_string(), self.config.port().to_string()),
            (
                "LAYA_API_KEY".to_string(),
                self.config.api_key.as_str().to_string(),
            ),
            ("LAYA_MAX_LOADED".to_string(), "1".to_string()),
            ("LAYA_MODELS".to_string(), "multilingual".to_string()),
            ("LAYA_DEFAULT_MODEL".to_string(), "multilingual".to_string()),
            ("LAYA_PRELOAD".to_string(), "1".to_string()),
            ("HF_HUB_OFFLINE".to_string(), "1".to_string()),
        ];
        env.extend(self.extra_env.clone());
        env
    }

    /// Probe `GET /health` once with a short timeout. Returns the probe
    /// latency on 2xx, or a plain-language [`SidecarError`].
    pub async fn probe(&self) -> Result<Duration, SidecarError> {
        let client = reqwest::Client::builder()
            .timeout(HEALTH_TIMEOUT)
            .build()
            .map_err(|e| SidecarError::Io(e.to_string()))?;
        let t0 = Instant::now();
        let res = client
            .get(self.config.health_url())
            .header(
                "Authorization",
                format!("Bearer {}", self.config.api_key.as_str()),
            )
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SidecarError::HealthTimeout
                } else {
                    SidecarError::Io("health endpoint unreachable".to_string())
                }
            })?;
        if res.status().is_success() {
            Ok(t0.elapsed())
        } else {
            Err(SidecarError::HealthFailed(res.status().as_u16()))
        }
    }
}

/// IPC-facing status payload: running flag + port + cold-start. Contains no
/// key material and no absolute paths (binary name only, elsewhere).
#[derive(Debug, Clone, Serialize)]
pub struct SidecarStatusPayload {
    pub running: bool,
    pub port: u16,
    pub cold_start_ms: Option<u64>,
    pub state: String,
}

impl SidecarStatusPayload {
    pub fn from_supervisor(s: &SidecarSupervisor) -> Self {
        let st = s.status();
        let (running, cold_start_ms, state) = match &st {
            SidecarStatus::Running { cold_start_ms } => {
                (true, Some(*cold_start_ms), "running".to_string())
            }
            SidecarStatus::Starting { .. } => (false, None, "starting".to_string()),
            SidecarStatus::Down => (false, None, "down".to_string()),
            SidecarStatus::Stopped => (false, None, "stopped".to_string()),
        };
        Self {
            running,
            port: s.config.port(),
            cold_start_ms,
            state,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SidecarConfig {
        SidecarConfig::new("127.0.0.1", 43121, "test-key-abc".to_string()).unwrap()
    }

    #[test]
    fn rejects_non_loopback_host() {
        for host in ["0.0.0.0", "192.168.1.10", "example.com", ""] {
            // NOTE: no `unwrap_err` — SidecarConfig has no Debug by design
            // (key-leak guard), so match explicitly instead.
            match SidecarConfig::new(host, 43121, "k".to_string()) {
                Err(SidecarError::NonLoopbackHost(_)) => {}
                other => panic!("host {host:?} must be refused, got {}", other.is_ok()),
            }
        }
        // Loopback forms accepted.
        for host in ["127.0.0.1", "localhost", "::1"] {
            assert!(
                SidecarConfig::new(host, 43121, "k".to_string()).is_ok(),
                "host {host:?} must be accepted"
            );
        }
    }

    #[test]
    fn rejects_zero_port() {
        match SidecarConfig::new("127.0.0.1", 0, "k".to_string()) {
            Err(SidecarError::InvalidPort) => {}
            _ => panic!("port 0 must be refused"),
        }
    }

    #[test]
    fn health_url_is_loopback() {
        assert_eq!(test_config().health_url(), "http://127.0.0.1:43121/health");
    }

    #[test]
    fn describe_and_errors_never_leak_key() {
        let secret = "super-secret-key-12345";
        let cfg = SidecarConfig::new("127.0.0.1", 43121, secret.to_string()).unwrap();
        assert!(!cfg.describe().contains(secret));
        // NOTE: SidecarConfig deliberately has NO Debug derive — a {:?}
        // format must fail to compile, so the key cannot leak via Debug.
        // (If someone adds `#[derive(Debug)]` later, this test still guards
        // the error strings below.)
        let shown = format!("{}", SidecarError::HealthFailed(500));
        assert!(!shown.contains(secret));
        // The key must not appear even when errors wrap context that held it.
        let spawned = format!("{}", SidecarError::SpawnFailed("boot fault".to_string()));
        assert!(!spawned.contains(secret));
    }

    #[test]
    fn api_keys_unique_and_sized() {
        let a = generate_api_key().unwrap();
        let b = generate_api_key().unwrap();
        assert_ne!(a.as_str(), b.as_str());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn ephemeral_port_is_bindable_loopback() {
        let port = pick_ephemeral_port().unwrap();
        assert!(port > 0);
        // Must be re-bindable on loopback right away (proves it came from
        // the loopback range, not a wildcard pick).
        let _listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .expect("picked port must be bindable on loopback");
    }

    #[test]
    fn crash_policy_gives_up_after_max_restarts() {
        let sup = SidecarSupervisor::new(test_config());
        assert_eq!(sup.status(), SidecarStatus::Stopped);
        for attempt in 1..=MAX_RESTARTS {
            let st = sup.record_crash();
            assert_eq!(st, SidecarStatus::Starting { attempts: attempt });
        }
        assert_eq!(sup.record_crash(), SidecarStatus::Down);
        // Stays down — no silent resurrection without a fresh boot.
        assert_eq!(sup.record_crash(), SidecarStatus::Down);
    }

    #[test]
    fn healthy_transitions_to_running_once() {
        let sup = SidecarSupervisor::new(test_config());
        sup.record_healthy();
        let ms = match sup.status() {
            SidecarStatus::Running { cold_start_ms } => cold_start_ms,
            other => panic!("expected Running, got {other:?}"),
        };
        assert!(ms < 60_000, "cold-start clock must be sane in tests");
    }

    #[test]
    fn backoff_grows_linearly() {
        assert_eq!(SidecarSupervisor::backoff_for(1), Duration::from_secs(2));
        assert_eq!(SidecarSupervisor::backoff_for(3), Duration::from_secs(6));
    }

    #[test]
    fn child_env_pins_loopback_and_preload() {
        let env = SidecarSupervisor::new(test_config()).child_env();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).unwrap().1.clone();
        assert_eq!(get("LAYA_HOST"), "127.0.0.1");
        assert_eq!(get("LAYA_MAX_LOADED"), "1");
        assert_eq!(get("LAYA_PORT"), "43121");
        assert_eq!(get("LAYA_API_KEY"), "test-key-abc");
        // pt-BR routing: multilingual only, including the fallback default.
        assert_eq!(get("LAYA_MODELS"), "multilingual");
        assert_eq!(get("LAYA_DEFAULT_MODEL"), "multilingual");
        assert_eq!(get("HF_HUB_OFFLINE"), "1");
        // Key travels in env by design here — but never in Display/logs
        // (covered by describe_and_errors_never_leak_key).
    }

    #[test]
    fn extra_env_carries_weights_cache_without_key() {
        let sup = SidecarSupervisor::with_extra_env(
            test_config(),
            vec![("HF_HOME".to_string(), "/r/weights/hf-cache".to_string())],
        );
        let env = sup.child_env();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).unwrap().1.clone();
        assert_eq!(get("HF_HOME"), "/r/weights/hf-cache");
        assert_eq!(get("LAYA_API_KEY"), "test-key-abc");
    }

    #[test]
    fn status_payload_has_no_key_material() {
        let sup = SidecarSupervisor::new(SidecarConfig::new(
            "127.0.0.1",
            43121,
            "payload-secret-999".to_string(),
        )
        .unwrap());
        sup.record_healthy();
        let payload = SidecarStatusPayload::from_supervisor(&sup);
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("payload-secret-999"));
        assert!(payload.running);
        assert_eq!(payload.port, 43121);
    }
}
