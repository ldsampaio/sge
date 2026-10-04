// SGE connection IPC contract (Phase 1).
//
// `connect_account` is the permanent architecture contract for opening an
// IMAP INBOX session. Its signature MUST NOT change in later plans; the body
// delegates to `imap::probe` (filled in Plan 01-02, extended by later phases).

pub mod creds;
pub mod imap;
pub mod store;
pub mod sync;
pub mod commands;

use std::sync::{Arc, Mutex};

use creds::{CredentialStore, KeyringStore, SavedCredentials, ServerConfig};
use serde::Serialize;
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
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub active_account: Mutex<Option<ActiveAccount>>,
    pub session_manager: Mutex<Option<Arc<imap::manager::SessionManager>>>,
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

fn default_db_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(base.join("sge").join("sge.db"))
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
    let state = AppState {
        store,
        active_account: Mutex::new(None),
        session_manager: Mutex::new(None),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            connect_account,
            save_server_config,
            save_credentials,
            load_credentials,
            clear_credentials,
            commands::sync::start_sync,
            commands::sync::sync_status,
            commands::sync::set_seen,
            commands::sync::cancel_sync,
            commands::sync::list_messages,
            commands::sync::search_messages,
            commands::sync::fetch_message,
            commands::sync::save_attachment,
            load_server_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
