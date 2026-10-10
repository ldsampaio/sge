// SGE connection IPC contract (Phase 1).
//
// `connect_account` is the permanent architecture contract for opening an
// IMAP INBOX session. Its signature MUST NOT change in later plans; the body
// delegates to `imap::probe` (filled in Plan 01-02, extended by later phases).

pub mod creds;
pub mod drafts;
pub mod imap;
pub mod send_queue;
pub mod sidecar;
pub mod smtp;
pub mod classify;
pub mod store;
pub mod sync;
pub mod commands;

use std::sync::{Arc, Mutex};

use creds::{CredentialStore, KeyringStore, SavedCredentials, ServerConfig};
use serde::Serialize;
use tauri::Manager;
use thiserror::Error;

use store::Store;

/// Shared app state: the SQLite store, accessible to all Tauri commands,
/// plus the in-memory active account (set by `connect_account`).
///
/// `active_account` lets `start_sync` / `fetch_message` / `save_attachment`
/// reconnect without requiring keyring persistence (remember-me unchecked).
/// It is memory-only: never written to disk.
///
/// `session_manager` caches the single-session [`imap::manager::SessionManager`]
/// for the active account so flag writes reuse one authenticated session;
/// it is replaced when the account changes.
///
/// `sync_gate` is the Phase 8 single-flight guard: at most one `start_sync`
/// pass runs at a time, so a poll tick firing mid-sync skips instead of
/// overlapping.
///
/// `sync_cancel` is the cooperative cancel flag: `cancel_sync` sets it,
/// `start_sync` clears it when a pass begins, and the worker checks it
/// between sweep batches.
///
/// `trash_cache` maps account key (`host:port:username`, same key as
/// `session_manager`) to the resolved Trash wire name from
/// [`imap::trash::detect_trash`]. Memory-only, re-detected on LIST refresh;
/// consumed by the delete/move commands (Phase 10, Plan 10-03).
///
/// `app_data` is the `<data>/sge` dir owning `sge.db` and `attachments/`;
/// the expunge paths clean `<app_data>/attachments/<uidv>/<uid>/` best-effort.
///
/// `sidecar` is the Phase 15 Laya classifier supervisor (spawn/health/kill).
/// `None` until `setup()` spawns it; `Down`/`Stopped` when the binary is
/// missing or crashes repeatedly — sync and UI never block on it.
///
/// `sidecar_child` owns the live child process handle; killed on window
/// destroy so no orphan survives app quit.
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub active_account: Mutex<Option<ActiveAccount>>,
    pub session_manager: Mutex<Option<Arc<imap::manager::SessionManager>>>,
    pub sync_gate: Arc<sync::SyncGate>,
    pub sync_cancel: Arc<std::sync::atomic::AtomicBool>,
    pub trash_cache: Mutex<std::collections::HashMap<String, String>>,
    pub app_data: std::path::PathBuf,
    pub sidecar: Mutex<Option<Arc<sidecar::SidecarSupervisor>>>,
    pub sidecar_child:
        Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
    /// Phase 17 single-flight drain guard (shared by hook + commands).
    pub classify_gate: Arc<classify::worker::ClassifyGate>,
    /// Phase 20 cooperative batch-cancel flag (checked per chunk).
    pub batch_cancel: Arc<std::sync::atomic::AtomicBool>,
}

/// In-memory credentials + server config for the connected session.
#[derive(Debug, Clone)]
pub struct ActiveAccount {
    pub host: String,
    pub port: u16,
    pub security: String,
    pub username: String,
    pub password: String,
}

/// Typed argument-validation errors for the connect path. Transport, auth,
/// and TLS failures come from [`imap::ImapError`] and already name the
/// failing part for the login UX.
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("invalid host: server address must not be empty")]
    InvalidHost,
    #[error("invalid port: must be in range 1-65535")]
    InvalidPort,
}

/// Mailbox summary returned after a successful INBOX SELECT.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectSummary {
    pub selected_mailbox: String,
    pub uid_validity: u32,
    pub exists: u32,
}

/// Open an IMAP session and SELECT INBOX, returning the mailbox summary.
///
/// Arguments come from the login form over IPC; host/port are validated here
/// in Rust and never trusted from the frontend. Password is held only for
/// this call: never logged, never persisted (keyring persistence lands in
/// Plan 01-03 behind remember-me consent).
///
/// Runs on a dedicated blocking thread (`spawn_blocking` + async-std
/// executor), never on a Tokio runtime thread.
#[tauri::command]
async fn connect_account(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    security: String,
    username: String,
    password: String,
) -> Result<ConnectSummary, String> {
    // WR-03: normalize before anything touches the network — the trimmed,
    // port-stripped value is what reaches dial/DNS/TLS SNI, never the raw
    // frontend string.
    let host = imap::normalize_host(&host).map_err(|e| e.to_string())?;
    if port == 0 {
        return Err(ConnectError::InvalidPort.to_string());
    }
    let mode = imap::SecurityMode::parse(&security).map_err(|e| e.to_string())?;
    // Localhost unencrypted mode needs no extra click; remote unencrypted mode is always
    // refused inside the session module.
    let loopback = imap::is_loopback(&host);
    // Clones for the in-memory session (cfg moves the originals).
    let mem_host = host.clone();
    let mem_username = username.clone();
    let mem_password = password.clone();
    let mem_security = security.clone();
    let cfg = imap::AccountConfig {
        host,
        port,
        security: mode,
        username,
        // WR-10: zeroized on drop once the spawned probe finishes.
        password: zeroize::Zeroizing::new(password),
        // No cert-exception path through this command: untrusted certs hard
        // fail. A warned one-time override can arrive via a separate,
        // explicitly confirmed command in a later plan.
        allow_untrusted: false,
        plain_local_confirmed: loopback,
    };
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(imap::probe::run_probe(&cfg))
    })
    .await
    .map_err(|e| format!("internal error: connection task failed ({e})"))?
    .map_err(|e| e.to_string())?;
    // Remember the session in memory so start_sync can reconnect even
    // when remember-me (keyring) is unchecked.
    {
        let mut guard = state.active_account.lock().unwrap();
        *guard = Some(ActiveAccount {
            host: mem_host,
            port,
            security: mem_security,
            username: mem_username,
            password: mem_password,
        });
    }
    Ok(ConnectSummary {
        selected_mailbox: outcome.summary.selected_mailbox,
        uid_validity: outcome.summary.uid_validity,
        exists: outcome.summary.exists,
    })
}

/// Save server config (host/port/security) to the OS keyring so
/// `start_sync` can reconnect without re-prompting the user.
#[tauri::command]
async fn save_server_config(host: String, port: u16, security: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        creds::KeyringStore::new().save_server_config(&creds::ServerConfig {
            host,
            port,
            security,
        })
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?
    .map_err(|e| e.to_string())
}

/// Save username + password to the OS keyring (remember-me consent).
/// Keyring I/O blocks: runs on a blocking thread, never on async runtime
/// threads. Keyring-locked/headless failures surface as friendly errors so
/// the UI can fall back to a memory-only session with an explanatory note.
///
/// IN-04: the username is trim-checked but the password is only empty-checked
/// — password whitespace is significant (IMAP passwords may contain spaces)
/// and must never be stripped.
#[tauri::command]
async fn save_credentials(username: String, password: String) -> Result<(), String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("username and password must not be empty".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        KeyringStore::new().save(&SavedCredentials { username, password })
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?
    .map_err(|e| e.to_string())
}

/// Load remembered credentials, if any. `Ok(None)` means nothing was saved.
#[tauri::command]
async fn load_credentials() -> Result<Option<SavedCredentials>, String> {
    tauri::async_runtime::spawn_blocking(|| KeyringStore::new().load())
        .await
        .map_err(|e| format!("internal error: keyring task failed ({e})"))?
        .map_err(|e| e.to_string())
}

/// Delete remembered credentials. Missing entries are a no-op success.
#[tauri::command]
async fn clear_credentials() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| KeyringStore::new().clear())
        .await
        .map_err(|e| format!("internal error: keyring task failed ({e})"))?
        .map_err(|e| e.to_string())
}

/// Load saved server configuration (host/port/security) from the keyring.
///
/// Returns `Ok(None)` when no entry exists or the keyring is unavailable —
/// never surfaces a plaintext fallback (WR-02).
#[tauri::command]
async fn load_server_config() -> Result<Option<ServerConfig>, String> {
    let stored = tauri::async_runtime::spawn_blocking(|| {
        KeyringStore::new().load_server_config()
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?;

    match stored {
        Ok(opt) => Ok(opt),
        Err(creds::CredsError::StoreUnavailable { .. }) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Classifier sidecar status for the UI and later phases (Phase 15+).
///
/// Returns `{ running, port, cold_start_ms, state }`. Never contains key
/// material or absolute paths. `stopped` when the sidecar never spawned
/// (e.g. binary missing) — sync and UI treat that as "unclassified", not
/// as an error.
#[tauri::command]
async fn sidecar_status(
    state: tauri::State<'_, AppState>,
) -> Result<sidecar::SidecarStatusPayload, String> {
    let guard = state.sidecar.lock().unwrap();
    match guard.as_ref() {
        Some(sup) => Ok(sidecar::SidecarStatusPayload::from_supervisor(sup)),
        None => Ok(sidecar::SidecarStatusPayload {
            running: false,
            port: 0,
            cold_start_ms: None,
            state: "stopped".to_string(),
        }),
    }
}

/// Background supervision loop: probe until healthy, restart on crash with
/// backoff, give up after [`sidecar::MAX_RESTARTS`] (status `Down`).
/// Never blocks window creation — runs on a detached async task.
async fn supervise_sidecar(
    app: tauri::AppHandle,
    supervisor: Arc<sidecar::SidecarSupervisor>,
) {
    // Initial settle: give the frozen Python runtime a moment before the
    // first probe (cold-start seconds are measured spawn → first 200).
    let mut attempts: u32 = 0;
    loop {
        match supervisor.probe().await {
            Ok(_) => {
                supervisor.record_healthy();
                return;
            }
            Err(_) => {
                attempts += 1;
                if attempts > 6 {
                    break;
                }
                async_std::task::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
    // Probe never went green in the grace window — check whether the child
    // is even alive; if not, record one crash and leave restart policy to
    // the supervisor state machine (a fresh spawn happens next launch).
    //
    // NOTE: full kill-and-respawn inside this loop needs the shell scope on
    // the AppHandle; the minimal spike keeps one spawn per boot and surfaces
    // Down honestly. Crash-restart-across-boots is covered by setup() spawn.
    let _ = app;
    supervisor.record_crash();
}

/// Weights dir under the Tauri resource dir (production bundles).
fn resource_weights_dir(app: &tauri::App) -> std::path::PathBuf {
    app.path()
        .resource_dir()
        .map(|d| d.join("weights"))
        .unwrap_or_else(|_| std::path::PathBuf::from("weights"))
}

/// Build the weights-related child env for a candidate weights dir.
///
/// Advertises `HF_HOME` (offline snapshot cache) when the HF marker exists
/// and `LAYA_EXTRA_MODELS` (checkpoint re-point at the materialized
/// standalone snapshot) when the checkpoints dir exists. Empty when neither
/// marker is present — the sidecar then starts without preloadable weights
/// and reports Down honestly instead of crash-looping.
fn weights_extra_env(weights: &std::path::Path) -> Vec<(String, String)> {
    let mut extra = Vec::new();
    if weights.join("hub").is_dir() {
        extra.push((
            "HF_HOME".to_string(),
            weights.to_string_lossy().into_owned(),
        ));
    }
    let ckpt = weights.join("checkpoints").join("multilingual");
    if ckpt.is_dir() {
        extra.push((
            "LAYA_EXTRA_MODELS".to_string(),
            format!(
                "{{\"multilingual\": \"{}\"}}",
                ckpt.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
            ),
        ));
    }
    extra
}

/// Spawn the Laya sidecar (best-effort, never fatal): generate per-boot key
/// + ephemeral loopback port, spawn via the shell plugin, store supervisor
/// + child, detach the health-probe loop.
///
/// `laya-serve` is configured env-only (it takes no CLI port/host args).
/// Weights resolve from the bundled resource dir (`HF_HOME` → resource
/// `weights/hf-cache`, offline); in dev the resource dir may lack weights
/// and the sidecar reports Down honestly until `build-sidecar.sh` populates
/// them.
fn spawn_sidecar_best_effort(app: &tauri::App) -> Option<Arc<sidecar::SidecarSupervisor>> {
    use tauri_plugin_shell::ShellExt;
    let port = sidecar::pick_ephemeral_port().ok()?;
    let key = sidecar::generate_api_key().ok()?;
    let config = sidecar::SidecarConfig::new("127.0.0.1", port, key.to_string()).ok()?;
    // Weights cache: <resource-dir>/weights (shipped HF_HOME layout) or
    // the repo sidecar/weights dir in dev. Only advertised when the HF
    // cache marker exists, so a half-downloaded dir is never used.
    //
    // Checkpoint re-point: the Router resolves "multilingual" to the BUNDLE
    // repo (convaiinnovations/laya subfolder), which we do NOT ship. The
    // build script materializes the pinned standalone snapshot at
    // weights/checkpoints/multilingual/, and LAYA_EXTRA_MODELS re-points
    // the name at that local dir (documented laya mechanism — a matching
    // name re-points the checkpoint instead of adding one).
    let mut extra = weights_extra_env(&resource_weights_dir(app));
    if extra.is_empty() {
        // Dev fallback: `cargo run` / `tauri dev` binaries live in
        // src-tauri/target/{debug,release}/ — the repo sidecar/weights dir
        // is found by walking up from the current exe. Production bundles
        // use resource_dir above.
        if let Ok(exe) = std::env::current_exe() {
            let mut dir = exe.as_path();
            for _ in 0..5 {
                if let Some(parent) = dir.parent() {
                    dir = parent;
                    let cand = dir.join("sidecar").join("weights");
                    extra = weights_extra_env(&cand);
                    if !extra.is_empty() {
                        break;
                    }
                }
            }
        }
    }
    let supervisor = Arc::new(sidecar::SidecarSupervisor::with_extra_env(config, extra));
    let (_, child) = app
        .shell()
        .sidecar("sge-laya")
        .ok()?
        .envs(supervisor.child_env())
        .spawn()
        .ok()?;
    if let Some(state) = app.try_state::<AppState>() {
        *state.sidecar_child.lock().unwrap() = Some(child);
    }
    Some(supervisor)
}
fn default_db_path() -> Option<std::path::PathBuf> {
    app_data_dir().map(|d| d.join("sge.db"))
}

/// App-data dir owning `sge.db` + `attachments/` (`<data>/sge`).
fn app_data_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(base.join("sge"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store_instance = default_db_path()
        .and_then(|path| crate::store::Store::open(&path).ok())
        .unwrap_or_else(|| {
            crate::store::Store::open_in_memory()
                .expect("in-memory store should always succeed")
        });
    let store = Arc::new(Mutex::new(store_instance));
    let app_data = app_data_dir().unwrap_or_else(|| std::env::temp_dir().join("sge"));
    let state = AppState {
        store,
        active_account: Mutex::new(None),
        session_manager: Mutex::new(None),
        sync_gate: Arc::new(sync::SyncGate::default()),
        sync_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        trash_cache: Mutex::new(std::collections::HashMap::new()),
        app_data,
        sidecar: Mutex::new(None),
        sidecar_child: Mutex::new(None),
        classify_gate: Arc::new(classify::worker::ClassifyGate::default()),
        batch_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .setup(|app| {
            // Phase 15: spawn the classifier sidecar backgrounded — window
            // creation never waits on it. Best-effort: a missing binary
            // leaves `sidecar: None` and `sidecar_status` reports stopped.
            if let Some(supervisor) = spawn_sidecar_best_effort(app) {
                if let Some(state) = app.try_state::<AppState>() {
                    *state.sidecar.lock().unwrap() = Some(supervisor.clone());
                }
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(supervise_sidecar(handle, supervisor));
            }
            // Phase 17: crash safety — rows stuck `processing` from a killed
            // run go back to `pending` for the next drain.
            if let Some(state) = app.try_state::<AppState>() {
                let guard = state.store.lock().unwrap();
                let reaped = crate::classify::worker::reap_processing(&guard);
                if reaped > 0 {
                    eprintln!("[SGE classify] reaped {reaped} stuck processing rows");
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Kill-on-exit: no orphan sidecar survives app quit.
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    if let Some(child) = state.sidecar_child.lock().unwrap().take() {
                        let _ = child.kill();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            connect_account,
            save_server_config,
            save_credentials,
            load_credentials,
            clear_credentials,
            commands::sync::start_sync,
            commands::sync::sync_status,
            commands::sync::set_seen,
            commands::sync::delete_message,
            commands::sync::create_folder,
            commands::sync::rename_folder,
            commands::sync::delete_folder,
            commands::sync::move_message,
            commands::sync::expunge_messages,
            commands::sync::undo_queued_op,
            commands::sync::save_draft,
            commands::sync::get_draft,
            commands::sync::discard_draft,
            commands::sync::cancel_sync,
            commands::sync::list_messages,
            commands::sync::list_mailboxes,
            commands::sync::search_messages,
            commands::sync::fetch_message,
            commands::sync::save_attachment,
            commands::sync::queue_send,
            commands::sync::retry_send,
            commands::sync::send_status,
            commands::classify::classify_message,
            commands::classify::classify_status,
            commands::classify::confirm_suggestion,
            commands::classify::override_label,
            commands::classify::dismiss_suggestion,
            commands::classify::set_confidence_threshold,
            commands::classify::review_list,
            commands::classify::suggestion_detail,
            commands::classify::suggestion_for_uid,
            commands::classify::mailbox_labels,
            commands::classify::list_taxonomy,
            commands::classify::add_category,
            commands::classify::rename_category,
            commands::classify::merge_categories,
            commands::classify::delete_category,
            commands::classify::update_category_keywords,
            commands::classify::import_taxonomy,
            commands::classify::export_taxonomy,
            commands::classify::batch_classify,
            commands::classify::cancel_batch,
            commands::classify::batch_report,
            commands::classify::undo_batch,
            sidecar_status,
            load_server_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
