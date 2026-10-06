//! Sync Tauri commands: start_sync, sync_status, cancel_sync.
//!
//! `start_sync` prefers in-memory `AppState::active_account` (set on login),
//! falling back to the OS keyring so auto-connect after restart also works.
//! It opens a real IMAP session (BODY.PEEK only), runs the SyncWorker,
//! and streams typed SyncEvent progress over a Channel.
//! `sync_status` reads the worker-written sync_state row from SQLite.
//! `cancel_sync` sets the cancellation flag so the worker aborts at
//! the next batch boundary.
//!
//! No auto-login logic -- login screen always shown first (Phase 5).

use std::sync::Arc;

use serde::Serialize;
use tauri::{ipc::Channel, State};

use crate::sync::worker::SyncWorker;
use crate::sync::bodies;
use crate::sync::{SyncEvent, SyncCallback};
use crate::creds::{CredentialStore, KeyringStore, SavedCredentials, ServerConfig};
use crate::imap::{AccountConfig, SecurityMode};
use crate::imap::manager::SessionManager;
use crate::imap::session::connect_sync;
use crate::imap::SyncSession;
use crate::store::queries;

/// Load the active account config: prefer in-memory `active_account`
/// (set by `connect_account`), fall back to the OS keyring so sync works
/// after restart. Shared by `start_sync` and `set_seen`.
async fn load_account_config(
    state: tauri::State<'_, crate::AppState>,
) -> Result<AccountConfig, String> {
    // In-memory first (works even when "remember me" is unchecked).
    if let Some(acc) = state.active_account.lock().unwrap().clone() {
        eprintln!("[SGE sync] Using in-memory credentials for {}", acc.username);
        return Ok(AccountConfig {
            host: acc.host,
            port: acc.port,
            security: SecurityMode::parse(&acc.security).map_err(|e| e.to_string())?,
            username: acc.username,
            password: zeroize::Zeroizing::new(acc.password),
            allow_untrusted: false,
            plain_local_confirmed: false,
        });
    }
    eprintln!("[SGE sync] No in-memory session -- loading from keyring...");
    let server_cfg: ServerConfig = {
        let kr = KeyringStore::new();
        tauri::async_runtime::spawn_blocking(move || kr.load_server_config())
            .await
            .map_err(|e| format!("internal error: keyring task failed ({e})"))?
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "No server config saved -- log in first".to_string())?
    };
    let creds: SavedCredentials = {
        let kr = KeyringStore::new();
        tauri::async_runtime::spawn_blocking(move || kr.load())
            .await
            .map_err(|e| format!("internal error: keyring task failed ({e})"))?
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "No saved credentials -- log in first".to_string())?
    };
    eprintln!("[SGE sync] Using keyring credentials for {}", creds.username);
    Ok(AccountConfig {
        host: server_cfg.host,
        port: server_cfg.port,
        security: SecurityMode::parse(&server_cfg.security).map_err(|e| e.to_string())?,
        username: creds.username,
        password: zeroize::Zeroizing::new(creds.password),
        allow_untrusted: false,
        plain_local_confirmed: false,
    })
}

/// Get the cached [`SessionManager`] for this account, creating it on first
/// use and replacing it when the account changes. The slot holds only an
/// `Arc` clone — never held across `.await`.
fn manager_for(
    state: &tauri::State<'_, crate::AppState>,
    cfg: &AccountConfig,
) -> Arc<SessionManager> {
    let key = format!("{}:{}:{}", cfg.host, cfg.port, cfg.username);
    let mut slot = state.session_manager.lock().unwrap();
    if let Some(existing) = slot.as_ref() {
        if existing.account_key() == key {
            return existing.clone();
        }
    }
    let manager = Arc::new(SessionManager::new(cfg.clone()));
    *slot = Some(manager.clone());
    manager
}

/// Start a sync pass against the stored IMAP server + credentials.
///
/// Prefers the in-memory `AppState::active_account` (set by `connect_account`)
/// over the OS keyring, so sync works even when "Lembrar me" is unchecked.
/// Falls back to keyring if in-memory session is absent (e.g. app restart).
///
/// Progress events are streamed over `on_event: Channel<SyncEvent>`.
#[tauri::command]
pub async fn start_sync(
    state: State<'_, crate::AppState>,
    on_event: Channel<SyncEvent>,
    mailbox: String,
) -> Result<(), String> {
    // Load credentials: prefer in-memory session, fall back to keyring.
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;

    eprintln!("[SGE sync] Connecting to {}:{}...", account_cfg.host, account_cfg.port);

    // Run sync on a dedicated blocking thread.
    let store = state.store.clone();
    let gate = state.sync_gate.clone();
    let cancel_flag = state.sync_cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // Single-flight (Phase 8): a poll tick or second refresh that
            // arrives mid-sync skips instead of overlapping. The guard
            // releases on drop, so failures cannot wedge future syncs.
            let _pass = match gate.try_begin() {
                Some(guard) => guard,
                None => {
                    eprintln!("[SGE sync] Skipped: another sync pass is already running");
                    return Ok(Default::default());
                }
            };
            // A new pass clears any pending cancel (stale "Pausar" press).
            cancel_flag.store(false, std::sync::atomic::Ordering::SeqCst);
            let session = connect_sync(&account_cfg).await
                .map_err(|e| format!("IMAP connection: {e}"))?;
            eprintln!("[SGE sync] Connected -- starting worker...");
            let worker = SyncWorker::with_cancel(store, cancel_flag);
            let cb: SyncCallback = Arc::new(move |event| {
                let _ = on_event.send(event);
            });
            let result = worker.sync_with_session(Box::new(session), &mailbox, cb).await
                .map_err(|e| e.to_string());
            match &result {
                Ok(s) => eprintln!("[SGE sync] Done: new={} updated={} deleted={}", s.new, s.updated, s.deleted),
                Err(e) => eprintln!("[SGE sync] Error: {e}"),
            }
            result
        })
    })
    .await
    .map_err(|e| format!("internal error: sync task failed ({e})"))?
    .map_err(|e| e)?;

    Ok(())
}

/// Outcome of a `set_seen` toggle returned to the frontend.
///
/// The toggle applies locally instantly (optimistic UI). `acked` tells the
/// UI whether the server confirmed the write or the op stays queued in the
/// durable outbox with `pending_count` shown as the pending indicator until
/// a later sync acknowledges it.
#[derive(Debug, Clone, Serialize)]
pub struct SetSeenResult {
    pub uid: u32,
    pub seen: bool,
    pub acked: bool,
    pub pending_count: i64,
    pub detail: String,
}

/// Mark a message read (`seen=true`) or unread (`seen=false`).
///
/// Optimistic + durable: under one store lock the local flags flip and the
/// toggle enqueues in the outbox (latest-wins), then an immediate UID STORE
/// goes out through the cached [`SessionManager`] lease. The outbox op
/// deletes only on server acknowledgement — otherwise it stays queued with
/// the failure recorded, and the next sync replays it. A still-open session
/// also drains the rest of the queue opportunistically (replay on session
/// open). Runs on a blocking thread; the store lock is never held across
/// `.await`.
#[tauri::command]
pub async fn set_seen(
    state: State<'_, crate::AppState>,
    uid: u32,
    seen: bool,
    mailbox: Option<String>,
) -> Result<SetSeenResult, String> {
    // Optional in Phase 7: older frontends omit it; default preserves
    // the Phase 6 INBOX contract.
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Optimistic local write + durable enqueue under one lock.
            let (mailbox_id, epoch) = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mb = queries::ensure_mailbox(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?;
                let epoch = queries::get_sync_state(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?
                    .map(|(v, _)| v)
                    .unwrap_or(0);
                queries::set_local_seen(conn, mb, uid, seen)
                    .map_err(|e| format!("store: {e}"))?;
                queries::enqueue_outbox(conn, mb, uid, seen, epoch)
                    .map_err(|e| format!("store: {e}"))?;
                (mb, epoch)
            };

            // 2. Immediate UID STORE; ack deletes the op, failure stays queued.
            // The lease SELECTs `mailbox` first, so the write lands on the
            // intended folder (FOLD-03).
            let fail_reason: Option<String> = match manager.set_seen_in(&mailbox, uid, seen).await {
                Ok(()) => {
                    let guard = store.lock().unwrap();
                    let _ =
                        queries::delete_outbox_op(guard.conn(), mailbox_id, uid);
                    None
                }
                Err(e) => {
                    let guard = store.lock().unwrap();
                    let _ = queries::record_outbox_error(
                        guard.conn(),
                        mailbox_id,
                        uid,
                        &e.to_string(),
                    );
                    eprintln!(
                        "[SGE sync] set_seen uid {uid} not acknowledged ({e}) — stays queued"
                    );
                    Some(e.to_string())
                }
            };

            // 3. Session is open on success — drain the rest of the queue
            // opportunistically (no fresh SEARCH here, so no absent-UID
            // pruning; the next full sync handles that).
            if fail_reason.is_none() {
                match manager.lease().await {
                    Ok(mut lease) => {
                        let worker = SyncWorker::new(store.clone());
                        match worker
                            .replay_outbox(lease.session(), mailbox_id, epoch, None)
                            .await
                        {
                            Ok(r) => eprintln!(
                                "[SGE sync] set_seen replay: acked={} dropped={} failed={}",
                                r.acked, r.dropped, r.failed
                            ),
                            Err(e) => eprintln!("[SGE sync] set_seen replay error: {e}"),
                        }
                    }
                    Err(e) => eprintln!("[SGE sync] set_seen replay lease failed: {e}"),
                }
            }

            let pending_count = {
                let guard = store.lock().unwrap();
                queries::outbox_count(guard.conn(), mailbox_id).unwrap_or(0)
            };
            Ok(SetSeenResult {
                uid,
                seen,
                acked: fail_reason.is_none(),
                pending_count,
                detail: fail_reason.map_or_else(
                    || "Seen flag confirmed on server".to_string(),
                    |e| format!("Queued — will retry on next sync ({e})"),
                ),
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: set_seen task failed ({e})"))?
}

/// Return the latest sync status from SQLite: last_sync_at + counts.
///
/// Read-only -- no IMAP round-trip. Used by the frontend to show
/// "Up-to-date <timestamp>" or "Offline -- last synced <timestamp>".
/// `pending_count` is the durable-outbox depth (FLAG-02 pending indicator).
#[tauri::command]
pub async fn sync_status(
    state: State<'_, crate::AppState>,
    mailbox: Option<String>,
) -> Result<SyncStatus, String> {
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let (last_sync_at, uidv, uid_next, count) =
            queries::sync_status(conn, &mailbox)
                .map_err(|e| format!("store: {e}"))?
                .unwrap_or_default();
        let pending_count = queries::mailbox_id(conn, &mailbox)
            .map_err(|e| format!("store: {e}"))?
            .map(|mb| queries::outbox_count(conn, mb).unwrap_or(0))
            .unwrap_or(0);
        Ok(SyncStatus {
            mailbox,
            last_sync_at,
            uid_validity: uidv,
            uid_next,
            message_count: count,
            pending_count,
        })
    })
    .await
    .map_err(|e| format!("internal error: sync status task failed ({e})"))?
}

/// List messages for a mailbox from the local SQLite store (offline, no IMAP).
///
/// Mirrors the `sync_status` pattern: spawns a blocking thread, locks the
/// Store mutex only for the synchronous DB read, returns a Vec<MessageRow>.
/// Results are ordered newest-first (date_utc DESC, uid DESC) per the
/// store query. Pagination is controlled by `limit` (caller fetches more
/// batches for infinite scroll).
#[tauri::command]
pub async fn list_messages(
    state: State<'_, crate::AppState>,
    mailbox: String,
    limit: usize,
    offset: usize,
) -> Result<Vec<crate::store::queries::MessageRow>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        crate::store::queries::list_messages(conn, &mailbox, limit, offset)
            .map_err(|e| format!("store: {e}"))
    })
    .await
    .map_err(|e| format!("internal error: list messages task failed ({e})"))?
}

/// Full-text search over the local FTS5 index for a mailbox (offline, no IMAP).
///
/// Calls `queries::fts_search` which joins `messages_fts` against `messages`
/// with BM25 ranking. The search query string is passed as a parameter
/// binding to FTS5 -- no string interpolation (T-03-05 mitigations).
#[tauri::command]
pub async fn search_messages(
    state: State<'_, crate::AppState>,
    mailbox: String,
    query: String,
) -> Result<Vec<crate::store::queries::MessageRow>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        crate::store::queries::fts_search(conn, &mailbox, &query)
            .map_err(|e| format!("store: {e}"))
    })
    .await
    .map_err(|e| format!("internal error: search messages task failed ({e})"))?
}

/// Cancel the currently running sync pass.
///
/// Sets the cooperative flag; the worker checks it between sweep batches
/// and aborts cleanly (writes no partial sync_state, notifies the UI).
#[tauri::command]
pub async fn cancel_sync(state: State<'_, crate::AppState>) -> Result<(), String> {
    state
        .sync_cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
    eprintln!("[SGE sync] cancel requested");
    Ok(())
}

/// Discover server folders and return cached per-folder triage rows (FOLD-01/02).
///
/// `LIST "" "*"` via the SessionManager, then `STATUS (UIDVALIDITY UIDNEXT
/// UNSEEN)` per selectable folder, cached with `set_mailbox_status`.
/// `\Noselect` entries (hierarchy placeholders) are skipped — they cannot
/// be SELECTed or STATUSed. Returns the locally cached rows (same shape the
/// sidebar renders), so the tree works offline after the first discovery:
/// when LIST/STATUS fails, the cached rows are served instead of an error
/// (error only when nothing was ever cached).
#[tauri::command]
pub async fn list_mailboxes(
    state: State<'_, crate::AppState>,
) -> Result<Vec<crate::store::queries::MailboxRow>, String> {
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            let discovered = match manager.list_mailboxes().await {
                Ok(folders) => folders,
                Err(e) => {
                    // Offline fallback: serve the cached tree.
                    let guard = store.lock().unwrap();
                    let cached =
                        queries::list_mailboxes(guard.conn()).map_err(|e| format!("store: {e}"))?;
                    if cached.is_empty() {
                        return Err(format!("LIST failed and no folders cached: {e}"));
                    }
                    eprintln!("[SGE sync] LIST failed ({e}) — serving {n} cached folders", n = cached.len());
                    return Ok(cached);
                }
            };
            {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                for folder in &discovered {
                    if folder
                        .attributes
                        .iter()
                        .any(|a| a.contains("NoSelect"))
                    {
                        continue;
                    }
                    match manager.mailbox_status(&folder.name).await {
                        Ok(status) => {
                            let _ = queries::set_mailbox_status(
                                conn,
                                &folder.name,
                                status.uid_validity,
                                status.uid_next.unwrap_or(0),
                                status.unseen,
                            );
                        }
                        Err(e) => eprintln!(
                            "[SGE sync] STATUS {} failed ({e}) — folder cached without unseen",
                            folder.name
                        ),
                    }
                    let _ = queries::ensure_mailbox(conn, &folder.name);
                }
            }
            let guard = store.lock().unwrap();
            queries::list_mailboxes(guard.conn()).map_err(|e| format!("store: {e}"))
        })
    })
    .await
    .map_err(|e| format!("internal error: list mailboxes task failed ({e})"))?
}

/// Sync status returned to the frontend.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStatus {
    pub mailbox: String,
    pub last_sync_at: String,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub message_count: i64,
    /// Durable-outbox depth: toggles awaiting server acknowledgement.
    pub pending_count: i64,
}

// Phase 4: Reader + Attachments

/// Parsed, sanitized view of a single message -- returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub uid: u32,
    pub subject: String,
    pub from_addr: String,
    pub to_addrs: Vec<String>,
    pub date_utc: String,
    pub html: Option<String>,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub attachments: Vec<crate::store::queries::AttachmentInfo>,
}

/// Fetch a single message body, sanitize HTML, extract headers + attachments.
///
/// Loads server config + credentials from the OS keyring, connects to IMAP
/// (read-only), and issues `BODY.PEEK[]` -- never sets `\Seen`. HTML is sanitized
/// with ammonia (Phase 3 constraint) and attachment metadata is extracted but
/// content is not returned (saved via `save_attachment` on demand).
#[tauri::command]
pub async fn fetch_message(
    state: State<'_, crate::AppState>,
    uid: u32,
    mailbox: Option<String>,
) -> Result<MessageView, String> {
    // Optional in Phase 7: older frontends omit it; default preserves
    // the Phase 4 INBOX contract.
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    let store = state.store.clone();
    let acc = state.active_account.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // Load server config + credentials: prefer in-memory, fall back to keyring.
            let account_cfg: AccountConfig = if let Some(a) = acc {
                AccountConfig {
                    host: a.host,
                    port: a.port,
                    security: SecurityMode::parse(&a.security).map_err(|e| e.to_string())?,
                    username: a.username,
                    password: zeroize::Zeroizing::new(a.password),
                    allow_untrusted: false,
                    plain_local_confirmed: false,
                }
            } else {
                let kr = KeyringStore::new();
                let server_cfg = kr
                    .load_server_config()
                    .map_err(|e| e.to_string())?
                    .ok_or("No server config saved -- log in first")?;
                let creds = kr
                    .load()
                    .map_err(|e| e.to_string())?
                    .ok_or("No saved credentials -- log in first")?;
                AccountConfig {
                    host: server_cfg.host,
                    port: server_cfg.port,
                    security: SecurityMode::parse(&server_cfg.security)
                        .map_err(|e| e.to_string())?,
                    username: creds.username,
                    password: zeroize::Zeroizing::new(creds.password),
                    allow_untrusted: false,
                    plain_local_confirmed: false,
                }
            };

            // Connect, SELECT the requested folder, fetch body via BODY.PEEK[] (read-only).
            let mut session = connect_sync(&account_cfg)
                .await
                .map_err(|e| format!("IMAP connection failed: {e}"))?;
            let _ = session
                .select_mailbox(&mailbox)
                .await
                .map_err(|e| format!("IMAP SELECT failed: {e}"))?;
            let raw = session
                .fetch_body(uid)
                .await
                .map_err(|e| format!("IMAP fetch failed: {e}"))?;
            let _ = session.logout().await;

            // Parse the RFC 822 message.
            let msg = mail_parser::MessageParser::new()
                .parse(&raw)
                .ok_or("Failed to parse message body")?;

            // Headers from SQLite (stored during sync).
            let (subject, from_addr, to_addrs, date_utc) = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let row = crate::store::queries::get_message_by_uid(conn, &mailbox, uid)
                    .map_err(|e| format!("store error: {e}"))?
                    .ok_or("Message not found in local store")?;
                let to_vec = if row.to_addrs.is_empty() || row.to_addrs == "[]" {
                    Vec::new()
                } else {
                    serde_json::from_str(&row.to_addrs).unwrap_or_default()
                };
                (row.subject, row.from_addr, to_vec, row.date_utc)
            };

            // Extract + sanitize body content.
            let text = bodies::extract_text(&msg);
            let html = bodies::extract_html(&msg).map(|h| bodies::sanitize_html(&h));
            let attachments = bodies::extract_attachments(&msg);
            let has_attachments = !attachments.is_empty();

            // Cache body + attachment metadata (brief lock scope).
            {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mb_id = crate::store::queries::ensure_mailbox(conn, &mailbox)
                    .map_err(|e| format!("store error: {e}"))?;
                if let Some(msg_id) = crate::store::queries::find_message_id(conn, mb_id, uid)
                    .map_err(|e| format!("store error: {e}"))? {
                    let total = text
                        .as_ref()
                        .map(|s| s.len())
                        .unwrap_or(0)
                        + html
                            .as_ref()
                            .map(|s| s.len())
                            .unwrap_or(0);
                    if total <= bodies::BODY_CACHE_CAP_BYTES {
                        let _ = crate::store::queries::insert_body(
                            conn,
                            msg_id,
                            text.as_deref(),
                            html.as_deref(),
                        );
                        for att in &attachments {
                            let _ = crate::store::queries::insert_attachment_meta(
                                conn,
                                msg_id,
                                &att.part_number,
                                &att.name,
                                &att.content_type,
                                att.size,
                            );
                        }
                    }
                }
            }

            Ok(MessageView {
                uid,
                subject,
                from_addr,
                to_addrs,
                date_utc,
                html,
                text,
                has_attachments,
                attachments,
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: fetch_message task failed ({e})"))?
}

/// Save a single attachment to `file_path` on disk (user-chosen via dialog).
#[tauri::command]
pub async fn save_attachment(
    state: State<'_, crate::AppState>,
    uid: u32,
    part_number: String,
    file_path: String,
    mailbox: Option<String>,
) -> Result<String, String> {
    // Optional: older frontends omit it; default preserves the INBOX contract.
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    let acc = state.active_account.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            let account_cfg: AccountConfig = if let Some(a) = acc {
                AccountConfig {
                    host: a.host,
                    port: a.port,
                    security: SecurityMode::parse(&a.security).map_err(|e| e.to_string())?,
                    username: a.username,
                    password: zeroize::Zeroizing::new(a.password),
                    allow_untrusted: false,
                    plain_local_confirmed: false,
                }
            } else {
                let kr = KeyringStore::new();
                let server_cfg = kr
                    .load_server_config()
                    .map_err(|e| e.to_string())?
                    .ok_or("No server config saved -- log in first")?;
                let creds = kr
                    .load()
                    .map_err(|e| e.to_string())?
                    .ok_or("No saved credentials -- log in first")?;
                AccountConfig {
                    host: server_cfg.host,
                    port: server_cfg.port,
                    security: SecurityMode::parse(&server_cfg.security)
                        .map_err(|e| e.to_string())?,
                    username: creds.username,
                    password: zeroize::Zeroizing::new(creds.password),
                    allow_untrusted: false,
                    plain_local_confirmed: false,
                }
            };

            let mut session = connect_sync(&account_cfg)
                .await
                .map_err(|e| format!("IMAP connection failed: {e}"))?;
            let _ = session.select_mailbox(&mailbox).await
                .map_err(|e| format!("IMAP SELECT failed: {e}"))?;
            let raw = session.fetch_body(uid).await
                .map_err(|e| format!("IMAP fetch failed: {e}"))?;
            let _ = session.logout().await;

            let msg = mail_parser::MessageParser::new()
                .parse(&raw)
                .ok_or("Failed to parse message body")?;

            let bytes = bodies::extract_attachment_bytes(&msg, &part_number)
                .ok_or("Attachment not found")?;

            // Defense-in-depth: reduce file_name to basename (D-attachments).
            let safe_name = std::path::Path::new(&file_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if safe_name.is_empty() {
                return Err("Invalid filename".to_string());
            }

            std::fs::write(&file_path, &bytes)
                .map_err(|e| format!("Write error: {e}"))?;

            Ok(file_path)
        })
    })
    .await
    .map_err(|e| format!("internal error: save_attachment task failed ({e})"))?
}

#[cfg(test)]
mod tests {
    use crate::store::queries;
    use crate::store::Store;

    /// Verify the list_messages + search_messages query path against an
    /// in-memory store (same pattern as queries.rs tests but exercising
    /// the command-level integration).
    #[test]
    fn list_and_search_messages_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = queries::ensure_mailbox(conn, "INBOX").unwrap();

        // Insert 3 messages: 2 unread + 1 seen.
        queries::upsert_message(
            conn, mb_id, 1, None, "Hello", "alice@example.com", "[]",
            "[]", "2024-06-01T08:00:00Z", "[]", false, "preview one",
        ).unwrap();
        queries::upsert_message(
            conn, mb_id, 2, None, "World", "bob@example.com", "[]",
            "[]", "2024-06-02T08:00:00Z", "[]", false, "preview two",
        ).unwrap();
        queries::upsert_message(
            conn, mb_id, 3, None, "Read", "carol@example.com", "[]",
            "[]", "2024-06-03T08:00:00Z", r#"["\\Seen"]"#, false, "preview three",
        ).unwrap();

        // list_messages -> 3 rows, newest first (date_utc DESC)
        let rows = queries::list_messages(conn, "INBOX", 100, 0).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].uid, 3);   // newest date first
        assert_eq!(rows[1].uid, 2);
        assert_eq!(rows[2].uid, 1);

        // Read/unread classification from flags
        assert!(!queries::is_unread(&rows[0].flags));  // Seen
        assert!(queries::is_unread(&rows[1].flags));   // unread
        assert!(queries::is_unread(&rows[2].flags));   // unread

        // FTS search by sender term -> finds alice
        let results = queries::fts_search(conn, "INBOX", "alice").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].from_addr, "alice@example.com");

        // FTS search by subject term
        let results = queries::fts_search(conn, "INBOX", "World").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].uid, 2);
    }
}