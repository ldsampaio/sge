// SGE connection IPC contract (Phase 1, Plan 01-01).
//
// `connect_account` is the permanent architecture contract for opening an
// IMAP INBOX session. Its signature MUST NOT change in later plans:
// Plan 01-02 fills the body with the real `imap::session` implementation
// behind the same signature.

use serde::Serialize;
use thiserror::Error;

/// Typed backend errors for the connect path.
///
/// Each variant maps to a plain-language string that names the failing part
/// (host vs credentials vs TLS) for the login UX. Only the not-wired stub
/// exists in this plan; real variants land with the IMAP session in 01-02.
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("imap-not-wired")]
    NotWired,
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
#[tauri::command]
fn connect_account(
    host: String,
    port: u16,
    security: String,
    username: String,
    password: String,
) -> Result<ConnectSummary, String> {
    if host.trim().is_empty() {
        return Err(ConnectError::InvalidHost.to_string());
    }
    if port == 0 {
        return Err(ConnectError::InvalidPort.to_string());
    }
    // Tracer stub: real IMAP session wires in here in Plan 01-02.
    let _ = (security, username, password);
    Err(ConnectError::NotWired.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![connect_account])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
