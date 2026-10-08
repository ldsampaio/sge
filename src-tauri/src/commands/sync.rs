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
use crate::imap::trash::{detect_trash, TrashResolution};
use crate::imap::roles::{resolve_roles, Role};
use crate::imap::{AccountConfig, MailboxInfo, SecurityMode, SyncError};
use crate::imap::mutf7::{
    decode_modified_utf7, encode_modified_utf7, validate_leaf, FolderNameError,
};
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

    // Plan 10-02 precondition: the pass runs under the account's
    // SessionManager lease — one owned connection shared with (and
    // serialized against) destructive ops, never a fresh session per pass.
    let manager = manager_for(&state, &account_cfg);

    eprintln!("[SGE sync] Connecting to {}:{}...", account_cfg.host, account_cfg.port);

    // Run sync on a dedicated blocking thread.
    let store = state.store.clone();
    let gate = state.sync_gate.clone();
    let cancel_flag = state.sync_cancel.clone();
    let app_data = state.app_data.clone();
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
            // Lease SELECTs `mailbox` on the owned session (connecting
            // lazily on first use); the worker borrows it for the pass.
            // `connect_sync` stays reserved for bootstrap/probe paths only.
            let mut lease = manager.lease_for(&mailbox).await
                .map_err(|e| format!("IMAP lease: {e}"))?;
            eprintln!("[SGE sync] Leased session -- starting worker...");
            let worker = SyncWorker::with_cancel(store, cancel_flag)
                .with_attachment_root(app_data);
            let cb: SyncCallback = Arc::new(move |event| {
                let _ = on_event.send(event);
            });
            let result = worker.sync_with_borrowed(lease.session(), &mailbox, cb).await
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

/// Combined durable-queue depth (flag + delete/move + dirty drafts) for
/// `mailbox_id`. Surfaced in every delete/move/expunge/draft result and
/// `sync_status` as the pending indicator.
fn pending_depth(conn: &rusqlite::Connection, mailbox_id: u64) -> i64 {
    queries::outbox_count(conn, mailbox_id).unwrap_or(0)
        + queries::imap_outbox_count(conn, mailbox_id).unwrap_or(0)
        + queries::dirty_draft_count_for(conn, mailbox_id).unwrap_or(0)
}

/// Resolve the Trash wire name: per-account memory cache → LIST + detect.
///
/// Returns the RAW wire name (the only form valid for SELECT/MOVE), or
/// `None` when the server has no Trash — the caller prompts confirm, then
/// retries with `create_trash = true`.
async fn resolve_trash(
    state: &State<'_, crate::AppState>,
    manager: &Arc<SessionManager>,
    account_key: &str,
) -> Result<Option<String>, String> {
    if let Some(cached) = state
        .trash_cache
        .lock()
        .unwrap()
        .get(account_key)
        .cloned()
    {
        return Ok(Some(cached));
    }
    let folders = manager
        .list_mailboxes()
        .await
        .map_err(|e| e.to_string())?;
    match detect_trash(&folders) {
        TrashResolution::Found(name) => {
            state
                .trash_cache
                .lock()
                .unwrap()
                .insert(account_key.to_string(), name.clone());
            Ok(Some(name))
        }
        TrashResolution::Missing => Ok(None),
    }
}

/// Outcome of a `delete_message` / `move_message` call returned to the frontend.
///
/// The message hides locally instantly (optimistic `pending_delete`).
/// `acked` tells the UI whether the server confirmed the move or the op
/// stays queued in the durable outbox (undo valid iff still queued).
#[derive(Debug, Clone, Serialize)]
pub struct DeleteMoveResult {
    pub uid: u32,
    pub acked: bool,
    pub pending_count: i64,
    pub detail: String,
}

/// Delete a message: optimistic hide + durable move-to-Trash.
///
/// Under one store lock the row hides (`pending_delete`) and a `delete` op
/// (stored Trash dest) enqueues — consuming any same-key flag toggle —
/// then an immediate `move_message_in(mailbox → Trash)` goes out through
/// the cached [`SessionManager`] lease. Ack dequeues (row stays hidden
/// until the next sweep's expunge-diff removes it); failure stays queued
/// with the error recorded for pre-sweep replay. A loud refusal drops the
/// op immediately (replay would drop it too — never re-COPY).
///
/// Trash resolution: SPECIAL-USE → name match → `Missing`. Without Trash
/// and without `create_trash`, returns a `need_trash_confirm:` error for
/// the UI confirm path; the confirmed retry passes `create_trash = true`
/// and CREATEs `Trash` once. BODY.PEEK untouched; UID-only.
#[tauri::command]
pub async fn delete_message(
    state: State<'_, crate::AppState>,
    uid: u32,
    mailbox: Option<String>,
    create_trash: Option<bool>,
) -> Result<DeleteMoveResult, String> {
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let account_key = format!(
        "{}:{}:{}",
        account_cfg.host, account_cfg.port, account_cfg.username
    );
    let manager = manager_for(&state, &account_cfg);
    // Trash needs an async manager call — resolve before the blocking section.
    let trash = match resolve_trash(&state, &manager, &account_key).await? {
        Some(t) => t,
        None if create_trash.unwrap_or(false) => {
            manager
                .create_trash()
                .await
                .map_err(|e| e.to_string())?;
            let t = "Trash".to_string();
            state
                .trash_cache
                .lock()
                .unwrap()
                .insert(account_key.clone(), t.clone());
            t
        }
        None => {
            return Err("need_trash_confirm: nenhuma pasta Trash encontrada no servidor — confirme para criar 'Trash'".to_string());
        }
    };
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Optimistic hide + durable enqueue under one lock.
            let (mailbox_id, _epoch) = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mb = queries::ensure_mailbox(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?;
                let epoch = queries::get_sync_state(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?
                    .map(|(v, _)| v)
                    .unwrap_or(0);
                queries::set_pending_delete(conn, mb, uid, true)
                    .map_err(|e| format!("store: {e}"))?;
                queries::enqueue_imap_outbox(
                    conn,
                    mb,
                    uid,
                    queries::IMAP_OP_DELETE,
                    Some(&trash),
                    epoch,
                )
                .map_err(|e| format!("store: {e}"))?;
                (mb, epoch)
            };

            // 2. Immediate move-to-Trash; ack dequeues, failure stays queued.
            let (acked, detail) = match manager
                .move_message_in(&mailbox, &uid.to_string(), &trash)
                .await
            {
                Ok(_) => {
                    let guard = store.lock().unwrap();
                    let _ =
                        queries::delete_imap_outbox_op(guard.conn(), mailbox_id, uid);
                    (true, format!("Mensagem movida para {trash}"))
                }
                Err(SyncError::Refused(msg)) => {
                    let guard = store.lock().unwrap();
                    let _ =
                        queries::delete_imap_outbox_op(guard.conn(), mailbox_id, uid);
                    let _ =
                        queries::set_pending_delete(guard.conn(), mailbox_id, uid, false);
                    (false, format!("IMAP refused: {msg}"))
                }
                Err(e) => {
                    let guard = store.lock().unwrap();
                    let _ = queries::record_imap_outbox_error(
                        guard.conn(),
                        mailbox_id,
                        uid,
                        &e.to_string(),
                    );
                    eprintln!(
                        "[SGE sync] delete uid {uid} not acknowledged ({e}) — stays queued"
                    );
                    (
                        false,
                        format!("Sem conexão — será enviada no próximo sync ({e})"),
                    )
                }
            };

            let pending_count = {
                let guard = store.lock().unwrap();
                pending_depth(guard.conn(), mailbox_id)
            };
            Ok(DeleteMoveResult {
                uid,
                acked,
                pending_count,
                detail,
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: delete task failed ({e})"))?
}

/// Move a message to `dest` (raw wire name, never display_name).
///
/// Same optimistic + enqueue + immediate shape as [`delete_message`]:
/// hides the src row, enqueues a `move` op (consuming any pending Seen
/// toggle as dest-side intent), then `move_message_in(src → dest)`.
/// Restoring out of Trash is the same call with `dest = INBOX`.
#[tauri::command]
pub async fn move_message(
    state: State<'_, crate::AppState>,
    uid: u32,
    dest: String,
    mailbox: Option<String>,
) -> Result<DeleteMoveResult, String> {
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    if dest == mailbox {
        return Err("origem e destino são iguais — nada a mover".to_string());
    }
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Optimistic hide + durable enqueue under one lock.
            let mailbox_id = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mb = queries::ensure_mailbox(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?;
                let epoch = queries::get_sync_state(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?
                    .map(|(v, _)| v)
                    .unwrap_or(0);
                queries::set_pending_delete(conn, mb, uid, true)
                    .map_err(|e| format!("store: {e}"))?;
                queries::enqueue_imap_outbox(
                    conn,
                    mb,
                    uid,
                    queries::IMAP_OP_MOVE,
                    Some(&dest),
                    epoch,
                )
                .map_err(|e| format!("store: {e}"))?;
                mb
            };

            // 2. Immediate move; ack dequeues, failure stays queued.
            let (acked, detail) = match manager
                .move_message_in(&mailbox, &uid.to_string(), &dest)
                .await
            {
                Ok(outcome) => {
                    let guard = store.lock().unwrap();
                    let _ =
                        queries::delete_imap_outbox_op(guard.conn(), mailbox_id, uid);
                    let via = if outcome.used_fallback {
                        " (via cópia)"
                    } else {
                        ""
                    };
                    (true, format!("Mensagem movida para {dest}{via}"))
                }
                Err(SyncError::Refused(msg)) => {
                    let guard = store.lock().unwrap();
                    let _ =
                        queries::delete_imap_outbox_op(guard.conn(), mailbox_id, uid);
                    let _ =
                        queries::set_pending_delete(guard.conn(), mailbox_id, uid, false);
                    (false, format!("IMAP refused: {msg}"))
                }
                Err(e) => {
                    let guard = store.lock().unwrap();
                    let _ = queries::record_imap_outbox_error(
                        guard.conn(),
                        mailbox_id,
                        uid,
                        &e.to_string(),
                    );
                    eprintln!(
                        "[SGE sync] move uid {uid} not acknowledged ({e}) — stays queued"
                    );
                    (
                        false,
                        format!("Sem conexão — será enviada no próximo sync ({e})"),
                    )
                }
            };

            let pending_count = {
                let guard = store.lock().unwrap();
                pending_depth(guard.conn(), mailbox_id)
            };
            Ok(DeleteMoveResult {
                uid,
                acked,
                pending_count,
                detail,
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: move task failed ({e})"))?
}

/// Outcome of an `expunge_messages` call returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ExpungeResult {
    /// UIDs removed locally (== UIDs the server confirmed expunged).
    pub removed: usize,
    pub acked: bool,
    pub pending_count: i64,
    pub detail: String,
}

/// Permanently delete exactly `uids` (confirmed UID-scoped expunge).
///
/// UID-scoped `UID EXPUNGE` only (~200-UID chunks) — never a bare
/// `expunge()`, so other clients' `\Deleted` marks are untouched. On
/// success the local rows + bodies/parts + queued ops go and attachment
/// dirs clean best-effort; on failure nothing changes locally (no
/// optimistic write, so no rollback needed).
#[tauri::command]
pub async fn expunge_messages(
    state: State<'_, crate::AppState>,
    uids: Vec<u32>,
    mailbox: Option<String>,
) -> Result<ExpungeResult, String> {
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    if uids.is_empty() {
        return Err("nenhuma mensagem selecionada".to_string());
    }
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    let app_data = state.app_data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Scoped removal on the server first (chunked, UID-only).
            for chunk in uids.chunks(200) {
                let set = chunk
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                manager
                    .uid_expunge_in(&mailbox, &set)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            // 2. Server confirmed: delete local rows + dependents + queued ops.
            let (mailbox_id, epoch, removed) = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mb = queries::ensure_mailbox(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?;
                let epoch = queries::get_sync_state(conn, &mailbox)
                    .map_err(|e| format!("store: {e}"))?
                    .map(|(v, _)| v)
                    .unwrap_or(0);
                let removed = queries::expunge_uids_local(conn, mb, &uids)
                    .map_err(|e| format!("store: {e}"))?;
                (mb, epoch, removed)
            };
            // 3. Best-effort attachment dirs (never fail the command on fs error).
            for uid in &removed {
                let dir = crate::store::attachment_dir(&app_data, epoch, *uid);
                if dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&dir) {
                        eprintln!("[SGE sync] expunge attachment cleanup uid {uid} failed ({e})");
                    }
                }
            }
            let pending_count = {
                let guard = store.lock().unwrap();
                pending_depth(guard.conn(), mailbox_id)
            };
            Ok(ExpungeResult {
                removed: removed.len(),
                acked: true,
                pending_count,
                detail: format!(
                    "{} mensagem(ns) apagada(s) para sempre",
                    removed.len()
                ),
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: expunge task failed ({e})"))?
}

/// Outcome of an `undo_queued_op` call returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct UndoResult {
    pub uid: u32,
    pub restored: bool,
    pub detail: String,
}

/// Undo a still-queued delete/move: clear the hidden flag and drop the
/// outbox row (valid iff the op is still queued, i.e. until next sync).
///
/// Store-only path — no IMAP round-trip. Returns `restored = false` when
/// the op already replayed (nothing to undo).
#[tauri::command]
pub async fn undo_queued_op(
    state: State<'_, crate::AppState>,
    uid: u32,
    mailbox: Option<String>,
) -> Result<UndoResult, String> {
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string());
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let restored = match queries::mailbox_id(conn, &mailbox)
            .map_err(|e| format!("store: {e}"))?
        {
            Some(mb) => queries::undo_pending_op(conn, mb, uid)
                .map_err(|e| format!("store: {e}"))?,
            None => false,
        };
        Ok(UndoResult {
            uid,
            restored,
            detail: if restored {
                "Mensagem restaurada".to_string()
            } else {
                "Nada a desfazer — operação já sincronizada".to_string()
            },
        })
    })
    .await
    .map_err(|e| format!("internal error: undo task failed ({e})"))?
}

/// Outcome of a `save_draft` call returned to the frontend.
///
/// The save lands locally instantly (local-first, no network wait).
/// `acked` tells the UI whether the server confirmed the copy or the row
/// stays `dirty=1` for the reconnect flush; `server_uid` + `dirty` are the
/// Phase 13 DRAFT-03 send-transaction handoff.
#[derive(Debug, Clone, Serialize)]
pub struct DraftSaveResult {
    pub id: String,
    pub dirty: bool,
    pub server_uid: Option<u32>,
    pub acked: bool,
    pub pending_count: i64,
}

/// Outcome of a `discard_draft` call returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct DiscardResult {
    pub id: String,
    pub discarded: bool,
}

/// Find the Drafts wire name in a LIST result via role resolution
/// (Phase 11 roles: SPECIAL-USE `\Drafts` first, then known names).
/// Pure helper — unit-tested without network.
fn find_drafts_wire(mailboxes: &[MailboxInfo]) -> Option<String> {
    resolve_roles(mailboxes)
        .into_iter()
        .find(|(_, role)| *role == Role::Drafts)
        .map(|(wire, _)| wire)
}

/// Find the Drafts wire name in the LOCALLY CACHED folder tree (no network).
///
/// Role column first (`drafts`, written on every LIST refresh — Phase 11
/// T-11-07); case-insensitive known-name fallback for trees synced before
/// roles existed. Missing → `None` so the caller can fall back to a LIST
/// refresh or refuse with the frozen `drafts-missing:` prefix (Phase 11
/// create-confirm flow). Never gates on network by itself: the local-first
/// upsert must survive offline (DRAFT-01, CR-01).
fn find_drafts_wire_cached(conn: &rusqlite::Connection) -> Option<String> {
    let rows = queries::list_mailboxes(conn).ok()?;
    if let Some(hit) = rows.iter().find(|m| m.role == "drafts") {
        return Some(hit.name.clone());
    }
    rows.iter()
        .find(|m| {
            let lower = m.name.to_lowercase();
            lower == "drafts" || lower == "rascunhos" || lower == "[gmail]/drafts"
        })
        .map(|m| m.name.clone())
}

/// Resolve the Drafts wire name: an explicit `mailbox` override wins
/// (no network); otherwise LIST + role resolution. Missing → a
/// `drafts-missing:` refusal so the UI can run the Phase 11
/// create-confirm flow, then retry.
async fn resolve_drafts_wire(
    manager: &Arc<SessionManager>,
    mailbox: Option<String>,
) -> Result<String, String> {
    if let Some(wire) = mailbox {
        return Ok(wire);
    }
    let folders = manager
        .list_mailboxes()
        .await
        .map_err(|e| e.to_string())?;
    find_drafts_wire(&folders).ok_or_else(|| {
        "drafts-missing: nenhuma pasta Rascunhos encontrada no servidor — confirme para criar".to_string()
    })
}

/// Save a draft: local-first write, then APPEND-new + expunge-old.
///
/// Under one store lock the compose session upserts `dirty=1` (stable
/// `message_id` per session — read from the existing row, generated once
/// via `new_message_id`), so the call returns even offline. The wire name
/// resolves from the locally cached folder tree first (CR-01: a network
/// LIST before the upsert would fail offline before persisting anything);
/// a LIST refresh runs only on a cold cache. When online,
/// the manager persists the copy and the row marks clean with the
/// reconciled `server_uid`; on failure the row stays dirty with
/// `acked=false` and a plain-language detail (no secret leakage). A
/// UIDVALIDITY bump since the last sync drops the stale `server_uid`
/// (`old_uid=None` path — the orphan is reaped by the sweep).
///
/// Last-writer-wins: a server copy edited elsewhere is overwritten by the
/// next local save with no merge (documented, no conflict UI in MVP).
#[tauri::command]
pub async fn save_draft(
    state: State<'_, crate::AppState>,
    id: String,
    subject: String,
    body: String,
    to: String,
    cc: String,
    bcc: String,
    mailbox: Option<String>,
) -> Result<DraftSaveResult, String> {
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let from_addr = account_cfg.username.clone();
    let manager = manager_for(&state, &account_cfg);
    // CR-01 (local-first ordering): explicit override wins; otherwise the
    // cached tree — never a network LIST ahead of the local write. A LIST
    // refresh is only a cold-cache fallback; offline with a warm cache the
    // upsert below still lands `dirty=1` with `acked=false`.
    let wire: String = match mailbox {
        Some(w) => w,
        None => {
            let cached = {
                let guard = state.store.lock().unwrap();
                find_drafts_wire_cached(guard.conn())
            };
            match cached {
                Some(w) => w,
                None => resolve_drafts_wire(&manager, None).await?,
            }
        }
    };
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Local-first upsert under one lock (returns even offline).
            // The Message-ID is stable per session: read from the existing
            // row, generated once via `new_message_id` (T-12-03).
            let (mailbox_id, message_id, old_uid, epoch) = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mb = queries::ensure_mailbox(conn, &wire)
                    .map_err(|e| format!("store: {e}"))?;
                let existing = queries::get_draft(conn, &id)
                    .map_err(|e| format!("store: {e}"))?;
                let (msg_id, prev_uid) = match existing {
                    Some(row) => (row.message_id, row.server_uid),
                    None => (crate::drafts::new_message_id(&id), None),
                };
                let epoch = queries::get_sync_state(conn, &wire)
                    .map_err(|e| format!("store: {e}"))?
                    .map(|(v, _)| v);
                queries::upsert_draft(conn, &id, mb, &msg_id, &subject, &body, &to, &cc, &bcc)
                    .map_err(|e| format!("store: {e}"))?;
                (mb, msg_id, prev_uid, epoch)
            };

            // 2. Render + APPEND-new + expunge-old; failure stays dirty.
            let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S +0000").to_string();
            let split = |s: &str| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            };
            let fields = crate::drafts::DraftFields {
                from: from_addr,
                to: split(&to),
                cc: split(&cc),
                bcc: split(&bcc),
                subject: subject.clone(),
                body: body.clone(),
                message_id: message_id.clone(),
            };
            let bytes = crate::drafts::render_draft_rfc5322(&fields, &date);
            let (acked, server_uid) =
                match manager.save_draft_copy_in(&wire, &message_id, &bytes, old_uid, epoch).await
                {
                    Ok(new_uid) => {
                        let guard = store.lock().unwrap();
                        let _ = queries::mark_draft_clean(guard.conn(), &id, new_uid);
                        eprintln!("[SGE sync] save_draft {id} acknowledged as uid {new_uid}");
                        (true, Some(new_uid))
                    }
                    Err(e) => {
                        eprintln!(
                            "[SGE sync] save_draft {id} not acknowledged ({e}) — stays dirty"
                        );
                        (false, old_uid)
                    }
                };

            let (dirty, pending_count) = {
                let guard = store.lock().unwrap();
                let dirty = queries::get_draft(guard.conn(), &id)
                    .map_err(|e| format!("store: {e}"))?
                    .map(|r| r.dirty)
                    .unwrap_or(!acked);
                (dirty, pending_depth(guard.conn(), mailbox_id))
            };
            Ok(DraftSaveResult {
                id,
                dirty,
                server_uid,
                acked,
                pending_count,
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: save_draft task failed ({e})"))?
}

/// Load one draft for editing (local-first, no network wait).
///
/// Returns the local row including `server_uid` + `dirty` (the Phase 13
/// DRAFT-03 handoff). When the local row is missing — a server-only copy
/// with no local session — falls back to the server: the Message-ID is
/// deterministic per compose session (`new_message_id`), so the copy
/// reconciles via `UID SEARCH HEADER Message-ID` and the BODY.PEEK fetch
/// never sets `\Seen`. The fallback row persists locally as clean so the
/// next open is instant.
#[tauri::command]
pub async fn get_draft(
    state: State<'_, crate::AppState>,
    id: String,
) -> Result<queries::DraftRow, String> {
    let store = state.store.clone();
    let local: Option<queries::DraftRow> = {
        let guard = store.lock().unwrap();
        queries::get_draft(guard.conn(), &id).map_err(|e| format!("store: {e}"))?
    };
    if let Some(row) = local {
        return Ok(row);
    }
    // Rare server fallback (no local row).
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let wire = resolve_drafts_wire(&manager, None).await?;
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            let mut lease = manager
                .lease_for(&wire)
                .await
                .map_err(|e| e.to_string())?;
            let msg_id = crate::drafts::new_message_id(&id);
            let mut hits = lease
                .session()
                .uid_search_header("Message-ID", &msg_id)
                .await
                .map_err(|e| e.to_string())?;
            hits.sort_unstable();
            let uid = hits.into_iter().max().ok_or_else(|| {
                "Rascunho não encontrado — nem local nem no servidor".to_string()
            })?;
            // BODY.PEEK-only: the fallback never sets `\Seen`.
            let raw = lease
                .session()
                .fetch_body(uid)
                .await
                .map_err(|e| e.to_string())?;
            drop(lease);
            let parsed = mail_parser::MessageParser::new()
                .parse(&raw)
                .ok_or_else(|| "Não foi possível ler o rascunho do servidor".to_string())?;
            let addr_list = |a: Option<&mail_parser::Address<'_>>| {
                a.map(|addr| {
                    addr.iter()
                        .filter_map(|e| e.address().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
            };
            let (subject, body, to, cc, bcc) = (
                parsed.subject().unwrap_or_default().to_string(),
                parsed.body_text(0).map(|c| c.to_string()).unwrap_or_default(),
                addr_list(parsed.to()),
                addr_list(parsed.cc()),
                addr_list(parsed.bcc()),
            );
            let from = parsed
                .from()
                .and_then(|a| a.first())
                .and_then(|e| e.address().map(|s| s.to_string()))
                .unwrap_or_default();
            let _ = from;
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let mb = queries::ensure_mailbox(conn, &wire)
                .map_err(|e| format!("store: {e}"))?;
            queries::upsert_draft(conn, &id, mb, &msg_id, &subject, &body, &to, &cc, &bcc)
                .map_err(|e| format!("store: {e}"))?;
            queries::mark_draft_clean(conn, &id, uid)
                .map_err(|e| format!("store: {e}"))?;
            queries::get_draft(conn, &id)
                .map_err(|e| format!("store: {e}"))?
                .ok_or_else(|| "Rascunho não encontrado após leitura do servidor".to_string())
        })
    })
    .await
    .map_err(|e| format!("internal error: get_draft task failed ({e})"))?
}

/// Discard a draft: delete the local row + expunge the tracked server copy.
///
/// The server leg is best-effort online: if the network fails (or the
/// Drafts folder is missing), the local row is already gone and the orphan
/// is reaped by the next sweep's expunge-diff. Always succeeds once the
/// local row is deleted — the UI confirms only when dirty content exists
/// (UI-side rule).
#[tauri::command]
pub async fn discard_draft(
    state: State<'_, crate::AppState>,
    id: String,
) -> Result<DiscardResult, String> {
    let store = state.store.clone();
    let tracked: Option<(u64, u32)> = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let row = queries::get_draft(conn, &id).map_err(|e| format!("store: {e}"))?;
        let tracked = row.and_then(|r| r.server_uid.map(|u| (r.mailbox_id, u)));
        queries::delete_draft(conn, &id).map_err(|e| format!("store: {e}"))?;
        tracked
    };
    if let Some((_mailbox_id, uid)) = tracked {
        // Best-effort server cleanup — failures only log (T-12-05: scoped
        // single-UID expunge, never a bare `expunge()`).
        let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
        let manager = manager_for(&state, &account_cfg);
        match resolve_drafts_wire(&manager, None).await {
            Ok(wire) => {
                if let Err(e) = manager.discard_server_copy_in(&wire, uid).await {
                    eprintln!(
                        "[SGE sync] discard_draft {id} uid {uid} server cleanup failed ({e}) — orphan reaped by sweep"
                    );
                }
            }
            Err(e) => eprintln!(
                "[SGE sync] discard_draft {id}: no Drafts folder ({e}) — local row already gone"
            ),
        }
    }
    Ok(DiscardResult {
        id,
        discarded: true,
    })
}

/// Return the latest sync status from SQLite: last_sync_at + counts.
///
/// Read-only -- no IMAP round-trip. Used by the frontend to show
/// "Up-to-date <timestamp>" or "Offline -- last synced <timestamp>".
/// `pending_count` sums durable-outbox depths (flag toggles +
/// delete/move ops + dirty drafts) for the pending indicator.
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
            .map(|mb| pending_depth(conn, mb))
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

/// Full-text search over the local FTS5 index (offline, no IMAP).
///
/// Calls `queries::fts_search` which joins `messages_fts` against `messages`
/// with BM25 ranking. The search query string is passed as a parameter
/// binding to FTS5 -- no string interpolation (T-03-05 mitigations).
/// `mailbox = None` searches the whole account (all folders); every row
/// carries its folder in `mailbox` so results can jump to the right folder.
#[tauri::command]
pub async fn search_messages(
    state: State<'_, crate::AppState>,
    mailbox: Option<String>,
    query: String,
) -> Result<Vec<crate::store::queries::MessageRow>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        crate::store::queries::fts_search(conn, mailbox.as_deref(), &query)
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
/// Thin wrapper over [`refresh_mailbox_tree`]: `LIST "" "*"` via the
/// SessionManager, then `STATUS (UIDVALIDITY UIDNEXT UNSEEN)` per selectable
/// folder, cached with `set_mailbox_status`.
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
        async_std::task::block_on(refresh_mailbox_tree(&manager, &store))
    })
    .await
    .map_err(|e| format!("internal error: list mailboxes task failed ({e})"))?
}

/// Shared LIST + STATUS refresh body, reused verbatim by every folder
/// command after its verb succeeds (Plan 11-01): folder ops return the
/// fresh tree so the sidebar re-renders from one code path.
///
/// Caller runs this inside `spawn_blocking` + `block_on` (same as the
/// folder commands); the store lock sections are brief and sync-only.
async fn refresh_mailbox_tree(
    manager: &Arc<SessionManager>,
    store: &Arc<std::sync::Mutex<crate::store::Store>>,
) -> Result<Vec<crate::store::queries::MailboxRow>, String> {
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
    // Role schema (Plan 11-03): resolved once per refresh from the fresh
    // LIST, persisted to the M8 columns — the column is a cache, LIST is
    // truth (recomputed every refresh, T-11-07).
    let roles = resolve_roles(&discovered);
    let role_of = |name: &str| {
        roles
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| *r)
            .unwrap_or(Role::Custom)
    };
    // STATUS probes run WITHOUT the store lock held: the mutex is a plain
    // `std::sync::Mutex` and each probe is a network round-trip — holding
    // the lock across `.await` would stall every other store reader for
    // the whole refresh. Results are collected first, written in one
    // brief locked section below.
    let mut probed: Vec<(&MailboxInfo, Option<crate::imap::MailboxStatus>, String)> = Vec::new();
    for folder in &discovered {
        if has_noselect_attr(&folder.attributes) {
            continue;
        }
        let status = match manager.mailbox_status(&folder.name).await {
            Ok(status) => Some(status),
            Err(e) => {
                eprintln!(
                    "[SGE sync] STATUS {} failed ({e}) — folder cached without unseen",
                    folder.name
                );
                None
            }
        };
        probed.push((folder, status, role_of(&folder.name).as_str().to_string()));
    }
    {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        for (folder, status, role) in &probed {
            if let Some(status) = status {
                let _ = queries::set_mailbox_status(
                    conn,
                    &folder.name,
                    status.uid_validity,
                    status.uid_next.unwrap_or(0),
                    status.unseen,
                );
            }
            let _ = queries::ensure_mailbox(conn, &folder.name);
            // Hierarchy delimiter for tree rendering (M6).
            let _ = queries::set_mailbox_delimiter(conn, &folder.name, &folder.delimiter);
            // Resolved role + raw attributes for the role-aware guards (M8).
            let _ = queries::set_mailbox_role(
                conn,
                &folder.name,
                role,
                &folder.attributes.join(" "),
            );
        }
    }
    let guard = store.lock().unwrap();
    queries::list_mailboxes(guard.conn()).map_err(|e| format!("store: {e}"))
}

/// True when an IMAP-layer error smells like lost connectivity (as opposed
/// to a server refusal): folder ops are online-only, so these map to the
/// loud offline copy instead of being queued (no folder-op queue exists).
fn is_connectivity_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    [
        "connect",
        "reconnect",
        "timeout",
        "timed out",
        "dns",
        "network",
        "unreachable",
        "no route",
        "broken pipe",
        "connection reset",
        "connection refused",
        "i/o error",
        "io error",
    ]
    .iter()
    .any(|s| m.contains(s))
}

/// Outcome of a successful `create_folder` call: the created folder's RAW
/// wire name (so the UI selects/highlights it) plus the refreshed tree.
#[derive(Debug, Clone, Serialize)]
pub struct FolderTreeResult {
    pub created: String,
    pub mailboxes: Vec<queries::MailboxRow>,
}

/// Create one folder: validate → CREATE → re-LIST (FOLD-04, Plan 11-01).
///
/// `leaf` is the raw user-typed name, `parent` the RAW wire name of the
/// parent folder (`None`/empty = top level). The delimiter comes from the
/// parent's cached LIST row; the name is joined as
/// `parent + delimiter + leaf` in raw modified-UTF-7 wire form and encoded
/// backend-side (the UI never encodes — T-11-01). After CREATE the sidebar
/// tree refreshes through the shared [`refresh_mailbox_tree`] body.
///
/// Online-only: on connectivity failure returns the loud offline copy — NO
/// outbox enqueue (no folder-op queue exists). Validation and exists
/// pre-checks return before any verb call.
#[tauri::command]
pub async fn create_folder(
    state: State<'_, crate::AppState>,
    leaf: String,
    parent: Option<String>,
) -> Result<FolderTreeResult, String> {
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Snapshot the cached tree (brief lock, never across `.await`).
            let cached = {
                let guard = store.lock().unwrap();
                queries::list_mailboxes(guard.conn()).map_err(|e| format!("store: {e}"))?
            };
            // 2. Resolve the delimiter from the parent's cached row;
            // top-level creates join nothing. An unknown parent (stale
            // picker selection) refuses instead of joining with an empty
            // delimiter into a garbage top-level name — no verb fires.
            let parent_wire = parent.filter(|p| !p.is_empty());
            if let Some(p) = &parent_wire {
                if !cached.iter().any(|r| r.name == *p) {
                    return Err(
                        "Essa pasta não existe mais na lista — atualize a lista e tente de novo."
                            .to_string(),
                    );
                }
            }
            let delimiter = match &parent_wire {
                Some(p) => cached
                    .iter()
                    .find(|r| r.name == *p)
                    .map(|r| r.delimiter.clone())
                    .unwrap_or_default(),
                None => String::new(),
            };
            if parent_wire.is_some() && delimiter.is_empty() {
                // Flat namespace (no hierarchy delimiter): there is no
                // "inside" to create into — joining would concatenate two
                // names into one garbage top-level folder on the server.
                return Err(
                    "Não foi possível criar dentro dessa pasta — ela não aceita subpastas."
                        .to_string(),
                );
            }
            // 3. Validate + join + encode + exists pre-check (pure — no
            // verb has fired if this returns Err).
            let wire =
                prepare_create_wire(&cached, parent_wire.as_deref(), &leaf, &delimiter)?;
            // 4. Wire CREATE; online-only, no queue.
            if let Err(e) = manager.create_mailbox_in(&wire).await {
                return Err(map_create_error(&e));
            }
            // 5. Re-LIST refresh (shared body) + return the fresh tree.
            let mailboxes = refresh_mailbox_tree(&manager, &store).await?;
            Ok(FolderTreeResult {
                created: wire,
                mailboxes,
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: create folder task failed ({e})"))?
}

/// Exact pt-BR UI-SPEC copy for a rejected folder leaf (shared by the
/// create and rename pre-wire guards).
fn folder_name_error_copy(e: FolderNameError) -> String {
    match e {
        FolderNameError::Empty => "Dê um nome para a pasta.".to_string(),
        FolderNameError::ContainsDelimiter(d) => format!(
            "O nome não pode conter '{d}' — ele separa pastas. Crie uma pasta por vez."
        ),
        FolderNameError::ReservedInbox => {
            "INBOX é uma pasta reservada do servidor — escolha outro nome.".to_string()
        }
    }
}

/// Pure pre-wire guard for `create_folder` (Plan 11-01): validate, then
/// join `parent + delimiter + encoded leaf`, then exists pre-check against
/// the cached tree. Returns the RAW wire name or the exact pt-BR UI-SPEC
/// copy. Never touches the network — every `Err` here returns before any
/// verb call (T-11-02 hierarchy escape, T-11-03 INBOX variant).
///
/// Only the user-typed leaf is encoded: `parent` is already a RAW wire
/// name from the cached tree, and re-encoding it would corrupt the `&…-`
/// shift sequences of non-ASCII parents (`Caf&AOk-` → `Caf&-AOk-`).
fn prepare_create_wire(
    cached: &[queries::MailboxRow],
    parent: Option<&str>,
    leaf: &str,
    delimiter: &str,
) -> Result<String, String> {
    let leaf = leaf.trim();
    if let Err(e) = validate_leaf(leaf, delimiter) {
        return Err(folder_name_error_copy(e));
    }
    let encoded_leaf = encode_modified_utf7(leaf);
    let wire = match parent {
        Some(p) => format!("{p}{delimiter}{encoded_leaf}"),
        None => encoded_leaf,
    };
    if cached.iter().any(|r| r.name == wire) {
        return Err("Já existe uma pasta com esse nome.".to_string());
    }
    Ok(wire)
}

/// Map a failed CREATE to plain language (Plan 11-01): offline is loud
/// (no queue exists), already-exists repeats the exists copy, anything
/// else surfaces the server detail instead of a bare NO.
fn map_create_error(e: &SyncError) -> String {
    map_folder_error(e, "create", "")
}

/// Map a failed folder op to plain language (Plan 11-02): offline is loud
/// (no queue exists for any folder op), already-exists repeats the exists
/// copy, otherwise the op shapes the message — DELETE has an exact UI-SPEC
/// server-refusal copy, probe failures (LIST/STATUS reads) stay neutral.
fn map_folder_error(e: &SyncError, op: &str, display: &str) -> String {
    let msg = e.to_string();
    if is_connectivity_error(&msg) {
        return "Sem conexão — pastas só podem ser alteradas online. Tente de novo ao reconectar."
            .to_string();
    }
    let lower = msg.to_lowercase();
    if lower.contains("already exist") || lower.contains("mailbox exists") {
        return "Já existe uma pasta com esse nome.".to_string();
    }
    match op {
        "delete" => format!("O servidor não permitiu excluir {display}: {msg}."),
        "rename" => format!("Não foi possível renomear a pasta: {msg}"),
        "probe" => format!("Não foi possível ler as pastas do servidor: {msg}"),
        _ => format!("Não foi possível criar a pasta: {msg}"),
    }
}

/// True when a LIST attribute marks a hierarchy placeholder (`\Noselect`
/// in any backslash spelling) — never a rename/delete target.
fn has_noselect_attr(attributes: &[String]) -> bool {
    attributes.iter().any(|a| a.contains("NoSelect"))
}

/// System-role detection for the rename/delete double-guard (Plan 11-03).
///
/// Delegates to the single [`resolve_roles`](crate::imap::roles::resolve_roles)
/// schema over the cached M8 rows (attributes included, so SPECIAL-USE wins
/// exactly like the server view) — no name lists live here, never a second
/// detector (T-11-08). Returns the pt-BR role label for the disabled-state copy.
fn is_system_role(cached: &[queries::MailboxRow], name: &str) -> Option<&'static str> {
    let infos: Vec<MailboxInfo> = cached
        .iter()
        .map(|r| MailboxInfo {
            name: r.name.clone(),
            display_name: r.display_name.clone(),
            delimiter: r.delimiter.clone(),
            attributes: r.attributes.split_whitespace().map(|s| s.to_string()).collect(),
        })
        .collect();
    system_role_label(&infos, name)
}

/// Role-label lookup over a fresh or cached [`MailboxInfo`] slice via the
/// single `resolve_roles` schema. Shared by `is_system_role` (cached tree)
/// and the `delete_folder` fresh-LIST check (stale caches resolve Custom
/// and would pass — the fresh LIST is the authority, same as `\Noselect`).
fn system_role_label(infos: &[MailboxInfo], name: &str) -> Option<&'static str> {
    match resolve_roles(infos)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, role)| role)?
    {
        Role::Trash => Some("Lixeira"),
        Role::Sent => Some("Enviadas"),
        Role::Drafts => Some("Rascunhos"),
        Role::Inbox | Role::Custom => None,
    }
}

/// Effective hierarchy delimiter for a cached folder row.
///
/// The row's own delimiter normally; when the cache predates M6 (empty)
/// but the RAW wire name visibly carries hierarchy, the unambiguous
/// single-kind delimiter is inferred so a rename keeps its parent prefix
/// instead of silently promoting the folder to top level. A wire name
/// carrying BOTH `/` and `.` is ambiguous — refuse with the refresh copy
/// rather than guess the wrong parent (T-11-04 wrong-target RENAME).
fn effective_delimiter(old: &str, cached_delimiter: &str) -> Result<String, String> {
    if !cached_delimiter.is_empty() {
        return Ok(cached_delimiter.to_string());
    }
    let slash = old.contains('/');
    let dot = old.contains('.');
    match (slash, dot) {
        (true, false) => Ok("/".to_string()),
        (false, true) => Ok(".".to_string()),
        (true, true) => Err(
            "Essa pasta não existe mais na lista — atualize a lista e tente de novo.".to_string(),
        ),
        (false, false) => Ok(String::new()),
    }
}

/// Pure pre-wire guard for `rename_folder` (Plan 11-02): INBOX refusal →
/// system-role double-guard → `validate_leaf` on the new leaf (same
/// delimiter rules as create) → join + encode → target-exists. Returns
/// `(old_wire, new_wire)` or the exact pt-BR copy. Never touches the
/// network — every `Err` returns before any verb call (T-11-04).
///
/// Only the user-typed leaf is encoded: the kept parent prefix is already
/// a RAW wire name, and re-encoding it would corrupt the `&…-` shift
/// sequences of non-ASCII parents.
fn guard_rename(
    cached: &[queries::MailboxRow],
    old: &str,
    new_leaf: &str,
) -> Result<(String, String), String> {
    if old.eq_ignore_ascii_case("inbox") {
        return Err("A INBOX não pode ser renomeada — ela é fixa do servidor.".to_string());
    }
    let row = cached.iter().find(|r| r.name == old).ok_or_else(|| {
        "Essa pasta não existe mais na lista — atualize a lista e tente de novo.".to_string()
    })?;
    if let Some(role) = is_system_role(cached, old) {
        return Err(format!("{role} do sistema — o nome é fixo."));
    }
    let delimiter = effective_delimiter(old, &row.delimiter)?;
    let new_leaf = new_leaf.trim();
    if let Err(e) = validate_leaf(new_leaf, &delimiter) {
        return Err(folder_name_error_copy(e));
    }
    let encoded_leaf = encode_modified_utf7(new_leaf);
    let new_wire = match old.rsplit_once(delimiter.as_str()) {
        Some((parent, _)) if !delimiter.is_empty() => {
            format!("{parent}{}{encoded_leaf}", delimiter)
        }
        _ => encoded_leaf,
    };
    if cached.iter().any(|r| r.name == new_wire) {
        return Err("Já existe uma pasta com esse nome.".to_string());
    }
    Ok((old.to_string(), new_wire))
}

/// Outcome of the pure delete pre-wire guard (Plan 11-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeleteDecision {
    /// All guards passed — issue the DELETE verb.
    Proceed { wire: String, display: String },
    /// Non-empty folder without confirmation — surface the count modal.
    NeedCount(u32),
    /// Confirmed but the typed name does not match — surface typed confirm.
    NeedTyped(String),
}

/// Pure pre-wire guard for `delete_folder` (Plan 11-02): INBOX refusal →
/// unknown → system-role refusal (same `resolve_roles` schema as
/// `guard_rename`, T-11-03) → children (`wire + delimiter` prefix over ALL
/// cached delimiters) → non-empty confirm gates. The `\Noselect` check needs
/// fresh LIST attributes, so the command applies it separately before calling
/// this. Every refusal returns before any verb call (T-11-04/T-11-06).
fn guard_delete(
    cached: &[queries::MailboxRow],
    wire: &str,
    messages: u32,
    confirmed: bool,
    typed_name: Option<&str>,
) -> Result<DeleteDecision, String> {
    if wire.eq_ignore_ascii_case("inbox") {
        return Err("A INBOX não pode ser excluída — ela é fixa do servidor.".to_string());
    }
    let row = cached.iter().find(|r| r.name == wire).ok_or_else(|| {
        "Essa pasta não existe mais na lista — atualize a lista e tente de novo.".to_string()
    })?;
    if let Some(role) = is_system_role(cached, wire) {
        return Err(format!("{role} do sistema — a exclusão não é permitida."));
    }
    let mut delimiters: Vec<&str> = cached
        .iter()
        .map(|r| r.delimiter.as_str())
        .filter(|d| !d.is_empty())
        .collect();
    delimiters.push(row.delimiter.as_str());
    let has_children = cached.iter().any(|r| {
        r.name != wire && delimiters.iter().any(|d| !d.is_empty() && r.name.starts_with(&format!("{wire}{d}")))
    });
    if has_children {
        return Err(format!(
            "A pasta {} tem subpastas — exclua ou mova as subpastas primeiro.",
            row.display_name
        ));
    }
    if messages > 0 && !confirmed {
        return Ok(DeleteDecision::NeedCount(messages));
    }
    if messages > 0 && confirmed && typed_name != Some(row.display_name.as_str()) {
        return Ok(DeleteDecision::NeedTyped(row.display_name.clone()));
    }
    Ok(DeleteDecision::Proceed {
        wire: wire.to_string(),
        display: row.display_name.clone(),
    })
}

/// Outcome of a successful `rename_folder` call: old + new RAW wire names
/// (so the UI migrates selection atomically) plus the refreshed tree.
/// `warning` carries the post-rename UIDVALIDITY-bump notice when the
/// server did not preserve it (treated like an epoch change).
#[derive(Debug, Clone, Serialize)]
pub struct RenameFolderResult {
    pub old: String,
    pub new: String,
    pub warning: Option<String>,
    pub mailboxes: Vec<queries::MailboxRow>,
}

/// Rename one folder: guards → RENAME → cache migrate → re-LIST (FOLD-05).
///
/// `old` is the RAW wire name, `new_leaf` the raw user-typed leaf (same
/// delimiter rules as create; encoded backend-side). UIDs are preserved —
/// the cache row keeps its `mailbox_id`/`uid_validity`, only the name (and
/// subtree prefixes) change. Online-only with the loud offline copy.
#[tauri::command]
pub async fn rename_folder(
    state: State<'_, crate::AppState>,
    old: String,
    new_leaf: String,
) -> Result<RenameFolderResult, String> {
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Snapshot the cached tree (brief lock, never across `.await`).
            let cached = {
                let guard = store.lock().unwrap();
                queries::list_mailboxes(guard.conn()).map_err(|e| format!("store: {e}"))?
            };
            // 2. Pure guards (no verb has fired on Err).
            let (old_wire, new_wire) = guard_rename(&cached, &old, &new_leaf)?;
            // Same effective delimiter the guard joined with, so the
            // subtree prefix migration below moves exactly the children
            // the guard assumed (infallible here — the guard just
            // succeeded with the same inputs).
            let delimiter = cached
                .iter()
                .find(|r| r.name == old_wire)
                .map(|r| effective_delimiter(&old_wire, &r.delimiter))
                .transpose()?
                .unwrap_or_default();
            // 3. Wire RENAME; a UIDVALIDITY bump becomes a warning, not a
            // silent accept — the new name stands (epoch-change rule).
            let mut warning: Option<String> = None;
            if let Err(e) = manager.rename_mailbox_in(&old_wire, &new_wire).await {
                if matches!(e, SyncError::State(_)) {
                    warning = Some(
                        "A pasta foi renomeada, mas o servidor atualizou os dados da pasta — sincronize para conferir as mensagens.".to_string(),
                    );
                } else {
                    let display = cached
                        .iter()
                        .find(|r| r.name == old_wire)
                        .map(|r| r.display_name.clone())
                        .unwrap_or_else(|| decode_modified_utf7(&old_wire));
                    return Err(map_folder_error(&e, "rename", &display));
                }
            }
            // 4. Migrate the cache row + subtree (same lock section).
            {
                let guard = store.lock().unwrap();
                queries::rename_mailbox_cache(guard.conn(), &old_wire, &new_wire, &delimiter)
                    .map_err(|e| format!("store: {e}"))?;
            }
            // 5. Re-LIST refresh (shared body) + return old/new for atomic
            // selection migration.
            let mailboxes = refresh_mailbox_tree(&manager, &store).await?;
            Ok(RenameFolderResult {
                old: old_wire,
                new: new_wire,
                warning,
                mailboxes,
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: rename folder task failed ({e})"))?
}

/// Outcome of a successful `delete_folder` call: the deleted RAW wire name,
/// the refreshed tree, and the INBOX fallback hint for selection.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteFolderResult {
    pub deleted: String,
    pub mailboxes: Vec<queries::MailboxRow>,
    pub fallback: String,
}

/// Delete one folder: guards → DELETE → cache cascade → re-LIST (FOLD-06).
///
/// `confirmed` + `typed_name` drive the non-empty double gate: without
/// confirmation a non-empty folder returns `need_delete_confirm:{count}`
/// (no wire call); with confirmation but a mismatched typed name returns
/// `need_typed_confirm:{display}` (no wire call). Empty-then-delete is
/// never automated — refuse loudly instead. Online-only, INBOX/`\Noselect`/
/// system-role folders never reach the wire, selection falls back to INBOX.
#[tauri::command]
pub async fn delete_folder(
    state: State<'_, crate::AppState>,
    name: String,
    confirmed: bool,
    typed_name: Option<String>,
) -> Result<DeleteFolderResult, String> {
    let account_cfg: AccountConfig = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. INBOX fast refusal before any I/O (guard_delete repeats it
            // as the authority — same copy).
            if name.eq_ignore_ascii_case("inbox") {
                return Err("A INBOX não pode ser excluída — ela é fixa do servidor.".to_string());
            }
            // 2. Fresh LIST doubles as the online probe and the `\Noselect`
            // guard (cached rows carry no attributes until M8 in 11-03).
            let discovered = manager
                .list_mailboxes()
                .await
                .map_err(|e| map_folder_error(&e, "probe", &decode_modified_utf7(&name)))?;
            if let Some(mb) = discovered.iter().find(|m| m.name == name) {
                if has_noselect_attr(&mb.attributes) {
                    return Err(format!(
                        "A pasta {} é um marcador do servidor e não pode ser excluída.",
                        decode_modified_utf7(&name)
                    ));
                }
            }
            // 2b. Fresh-LIST system-role guard (same `resolve_roles` schema
            // as `guard_rename`/`guard_delete`): a stale cache resolves
            // Custom and would pass, so the fresh LIST is the authority.
            if let Some(role) = system_role_label(&discovered, &name) {
                return Err(format!("{role} do sistema — a exclusão não é permitida."));
            }
            // 3. Snapshot the cached tree (brief lock, never across `.await`).
            let cached = {
                let guard = store.lock().unwrap();
                queries::list_mailboxes(guard.conn()).map_err(|e| format!("store: {e}"))?
            };
            let display = cached
                .iter()
                .find(|r| r.name == name)
                .map(|r| r.display_name.clone())
                .unwrap_or_else(|| decode_modified_utf7(&name));
            // 4. Fresh STATUS feeds the non-empty guard.
            let messages = manager
                .mailbox_status(&name)
                .await
                .map_err(|e| map_folder_error(&e, "probe", &display))?
                .messages;
            // 5. Pure guards — every refusal returns before any verb call.
            // The decision carries the canonical (wire, display) pair used
            // below, so no second lookup can drift.
            let (target, display) = match guard_delete(
                &cached,
                &name,
                messages,
                confirmed,
                typed_name.as_deref(),
            )? {
                DeleteDecision::NeedCount(n) => {
                    return Err(format!("need_delete_confirm:{n}"));
                }
                DeleteDecision::NeedTyped(d) => {
                    return Err(format!("need_typed_confirm:{d}"));
                }
                DeleteDecision::Proceed { wire, display } => (wire, display),
            };
            // 6. Wire DELETE, then cascade the cache in the same lock section.
            if let Err(e) = manager.delete_mailbox_in(&target).await {
                return Err(map_folder_error(&e, "delete", &display));
            }
            {
                let guard = store.lock().unwrap();
                queries::delete_mailbox_cache(guard.conn(), &target)
                    .map_err(|e| format!("store: {e}"))?;
            }
            // 7. Re-LIST refresh (shared body); selection falls back to INBOX.
            let mailboxes = refresh_mailbox_tree(&manager, &store).await?;
            Ok(DeleteFolderResult {
                deleted: target,
                mailboxes,
                fallback: "INBOX".to_string(),
            })
        })
    })
    .await
    .map_err(|e| format!("internal error: delete folder task failed ({e})"))?
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
    use crate::imap::SyncError;
    use crate::imap::MailboxInfo;
    use crate::store::queries;
    use crate::store::Store;

    use super::{
        find_drafts_wire, guard_delete, guard_rename, has_noselect_attr, is_connectivity_error,
        map_create_error, map_folder_error, pending_depth, prepare_create_wire,
        DeleteDecision,
    };

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
        let results = queries::fts_search(conn, Some("INBOX"), "alice").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].from_addr, "alice@example.com");

        // FTS search by subject term
        let results = queries::fts_search(conn, Some("INBOX"), "World").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].uid, 2);
    }

    /// The pending indicator backing delete/move/expunge results sums BOTH
    /// durable queues (flag toggles + delete/move ops).
    #[test]
    fn pending_depth_sums_both_outboxes() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = queries::ensure_mailbox(conn, "INBOX").unwrap();
        assert_eq!(pending_depth(conn, mb_id), 0);

        queries::enqueue_outbox(conn, mb_id, 1, true, 100).unwrap();
        assert_eq!(pending_depth(conn, mb_id), 1);

        queries::enqueue_imap_outbox(
            conn, mb_id, 2, queries::IMAP_OP_DELETE, Some("Trash"), 100,
        )
        .unwrap();
        assert_eq!(pending_depth(conn, mb_id), 2);

        // Enqueueing the delete consumed no flag row here (different UID),
        // but same-UID enqueue collapses: flag row drops, imap row stands.
        queries::enqueue_outbox(conn, mb_id, 3, false, 100).unwrap();
        queries::enqueue_imap_outbox(
            conn, mb_id, 3, queries::IMAP_OP_MOVE, Some("Archive"), 100,
        )
        .unwrap();
        assert_eq!(pending_depth(conn, mb_id), 3);

        // Other mailboxes are isolated.
        let other = queries::ensure_mailbox(conn, "Sent").unwrap();
        assert_eq!(pending_depth(conn, other), 0);
    }

    /// Draft rows feed the same pending indicator: a dirty draft counts,
    /// a clean (acknowledged) one does not. Drives the `save_draft`
    /// `pending_count` contract (frozen UI shape: `DraftSaveResult`
    /// carries the depth so the indicator updates on every save).
    #[test]
    fn pending_depth_counts_dirty_drafts() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let drafts_mb = queries::ensure_mailbox(conn, "Drafts").unwrap();
        assert_eq!(pending_depth(conn, drafts_mb), 0);

        queries::upsert_draft(
            conn, "compose-1", drafts_mb, "<compose-1@sge.local>",
            "Subject", "Body", "to@example.com", "", "",
        )
        .unwrap();
        assert_eq!(pending_depth(conn, drafts_mb), 1);

        // Acknowledged (server copy confirmed) → depth drains.
        queries::mark_draft_clean(conn, "compose-1", 5).unwrap();
        assert_eq!(pending_depth(conn, drafts_mb), 0);

        // Drafts in another mailbox never leak into this folder's depth.
        let other = queries::ensure_mailbox(conn, "INBOX").unwrap();
        queries::upsert_draft(
            conn, "compose-2", other, "<compose-2@sge.local>",
            "Subject", "Body", "to@example.com", "", "",
        )
        .unwrap();
        assert_eq!(pending_depth(conn, other), 1);
        assert_eq!(pending_depth(conn, drafts_mb), 0);
    }

    /// `save_draft`/`get_draft` return the local row including `server_uid`
    /// + `dirty` — the Phase 13 DRAFT-03 send-transaction handoff. A clean
    /// row exposes the reconciled UID; a freshly upserted row is dirty
    /// with no server copy yet.
    #[test]
    fn draft_row_exposes_server_uid_and_dirty() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let drafts_mb = queries::ensure_mailbox(conn, "Drafts").unwrap();

        queries::upsert_draft(
            conn, "compose-9", drafts_mb, "<compose-9@sge.local>",
            "Subject", "Body", "to@example.com", "", "",
        )
        .unwrap();
        let row = queries::get_draft(conn, "compose-9").unwrap().unwrap();
        assert!(row.dirty);
        assert_eq!(row.server_uid, None);

        queries::mark_draft_clean(conn, "compose-9", 7).unwrap();
        let row = queries::get_draft(conn, "compose-9").unwrap().unwrap();
        assert!(!row.dirty);
        assert_eq!(row.server_uid, Some(7));
    }

    fn fixture_mailbox(name: &str, attributes: &[&str]) -> MailboxInfo {
        MailboxInfo {
            name: name.to_string(),
            display_name: name.to_string(),
            delimiter: "/".to_string(),
            attributes: attributes.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Drafts wire resolution: SPECIAL-USE `\Drafts` wins (the
    /// `resolve_drafts_wire` path `save_draft`/`discard_draft` take before
    /// any APPEND/expunge verb runs).
    #[test]
    fn find_drafts_wire_resolves_special_use() {
        let folders = vec![
            fixture_mailbox("INBOX", &[]),
            fixture_mailbox("Rascunhos", &["\\Drafts"]),
        ];
        assert_eq!(
            find_drafts_wire(&folders),
            Some("Rascunhos".to_string())
        );
    }

    /// No Drafts role anywhere → `None`, and the command layer turns that
    /// into the `drafts-missing:` refusal (UI runs the Phase 11
    /// create-confirm flow, then retries) — before any verb call.
    #[test]
    fn find_drafts_wire_missing_returns_none() {
        let folders = vec![
            fixture_mailbox("INBOX", &[]),
            fixture_mailbox("Archive", &[]),
        ];
        assert_eq!(find_drafts_wire(&folders), None);
    }

    /// Cached-tree fixture for the folder guards: INBOX plus one
    /// hierarchical parent (delimiter `/`).
    fn folder_tree() -> Vec<queries::MailboxRow> {
        vec![
            queries::MailboxRow {
                id: 1,
                name: "INBOX".to_string(),
                display_name: "INBOX".to_string(),
                delimiter: "".to_string(),
                role: "inbox".to_string(),
                attributes: "".to_string(),
                uid_validity: 100,
                uid_next: 1,
                last_sync_at: None,
                unread_count: 0,
                unseen_count: 0,
            },
            queries::MailboxRow {
                id: 2,
                name: "Pai".to_string(),
                display_name: "Pai".to_string(),
                delimiter: "/".to_string(),
                role: "".to_string(),
                attributes: "".to_string(),
                uid_validity: 100,
                uid_next: 1,
                last_sync_at: None,
                unread_count: 0,
                unseen_count: 0,
            },
        ]
    }

    /// Richer cached tree for the rename/delete guards: INBOX, a parent
    /// with one child, a lookalike (`Pai2`), and a system Trash by name.
    fn folder_tree_full() -> Vec<queries::MailboxRow> {
        let mut tree = folder_tree();
        let row = |id: u64, name: &str, delimiter: &str| queries::MailboxRow {
            id,
            name: name.to_string(),
            display_name: name.to_string(),
            delimiter: delimiter.to_string(),
            role: "".to_string(),
            attributes: "".to_string(),
            uid_validity: 100,
            uid_next: 1,
            last_sync_at: None,
            unread_count: 0,
            unseen_count: 0,
        };
        tree.push(row(3, "Pai/Sub", "/"));
        tree.push(row(4, "Pai2", ""));
        tree.push(row(5, "Lixeira", ""));
        tree
    }

    /// The create pre-wire guard returns UI-SPEC copy for every refusal —
    /// and because it is pure, every `Err` here provably returns before
    /// any verb call (T-11-02/T-11-03).
    #[test]
    fn prepare_create_wire_refusals_use_ui_spec_copy() {
        let tree = folder_tree();
        // Empty / whitespace-only.
        assert_eq!(
            prepare_create_wire(&tree, None, "", ""),
            Err("Dê um nome para a pasta.".to_string())
        );
        assert_eq!(
            prepare_create_wire(&tree, None, "   ", ""),
            Err("Dê um nome para a pasta.".to_string())
        );
        // Delimiter in leaf (hierarchy escape refused client-side).
        assert_eq!(
            prepare_create_wire(&tree, Some("Pai"), "A/B", "/"),
            Err("O nome não pode conter '/' — ele separa pastas. Crie uma pasta por vez.".to_string())
        );
        // INBOX variants reserved on any delimiter.
        for inbox in ["inbox", "INBOX", "Inbox"] {
            assert_eq!(
                prepare_create_wire(&tree, None, inbox, ""),
                Err("INBOX é uma pasta reservada do servidor — escolha outro nome.".to_string()),
                "{inbox:?} must be reserved"
            );
        }
        // Existing name short-circuits (raw wire comparison).
        assert_eq!(
            prepare_create_wire(&tree, None, "Pai", ""),
            Err("Já existe uma pasta com esse nome.".to_string())
        );
    }

    /// Happy paths join `parent + delimiter + leaf` and encode backend-side.
    #[test]
    fn prepare_create_wire_joins_and_encodes() {
        let tree = folder_tree();
        assert_eq!(
            prepare_create_wire(&tree, None, "Projetos", ""),
            Ok("Projetos".to_string())
        );
        assert_eq!(
            prepare_create_wire(&tree, Some("Pai"), "Filho", "/"),
            Ok("Pai/Filho".to_string())
        );
        // Non-ASCII leaf is encoded; the UI never does this (T-11-01).
        assert_eq!(
            prepare_create_wire(&tree, Some("Pai"), "Café", "/"),
            Ok("Pai/Caf&AOk-".to_string())
        );
        // Surrounding whitespace trims before join.
        assert_eq!(
            prepare_create_wire(&tree, None, "  Projetos  ", ""),
            Ok("Projetos".to_string())
        );
    }

    /// CREATE error mapping: offline is loud, already-exists repeats the
    /// exists copy, other NOs surface the server detail.
    #[test]
    fn map_create_error_copies() {
        let offline = SyncError::Protocol("SessionManager connect: dial failed".to_string());
        assert!(is_connectivity_error(&offline.to_string()));
        assert_eq!(
            map_create_error(&offline),
            "Sem conexão — pastas só podem ser alteradas online. Tente de novo ao reconectar.".to_string()
        );
        let exists =
            SyncError::Protocol("CREATE Pai: NO Mailbox already exists".to_string());
        assert_eq!(
            map_create_error(&exists),
            "Já existe uma pasta com esse nome.".to_string()
        );
        let other = SyncError::Protocol("CREATE x: NO bad separator".to_string());
        assert!(
            map_create_error(&other).starts_with("Não foi possível criar a pasta:"),
            "unexpected: {}",
            map_create_error(&other)
        );
    }

    /// The rename guard refuses INBOX / system roles / unknown / bad leaves /
    /// existing targets before any verb call — and joins + encodes on success.
    #[test]
    fn guard_rename_refusals_and_happy_paths() {
        let tree = folder_tree_full();
        // INBOX (any case) never reaches the wire.
        for inbox in ["INBOX", "inbox"] {
            assert_eq!(
                guard_rename(&tree, inbox, "Nova"),
                Err("A INBOX não pode ser renomeada — ela é fixa do servidor.".to_string())
            );
        }
        // System Trash by name is double-guarded here (UI disables upstream).
        assert_eq!(
            guard_rename(&tree, "Lixeira", "Outra"),
            Err("Lixeira do sistema — o nome é fixo.".to_string())
        );
        // Unknown folder: refuse, do not invent a server round-trip.
        assert_eq!(
            guard_rename(&tree, "Fantasma", "Nova"),
            Err("Essa pasta não existe mais na lista — atualize a lista e tente de novo.".to_string())
        );
        // New-leaf validation mirrors create (same delimiter rules).
        assert_eq!(
            guard_rename(&tree, "Pai", "A/B"),
            Err("O nome não pode conter '/' — ele separa pastas. Crie uma pasta por vez.".to_string())
        );
        assert_eq!(
            guard_rename(&tree, "Pai", "inbox"),
            Err("INBOX é uma pasta reservada do servidor — escolha outro nome.".to_string())
        );
        // Target exists (including renaming onto itself).
        assert_eq!(
            guard_rename(&tree, "Pai/Sub", "Sub"),
            Err("Já existe uma pasta com esse nome.".to_string())
        );
        assert_eq!(
            guard_rename(&tree, "Pai2", "Pai2"),
            Err("Já existe uma pasta com esse nome.".to_string())
        );
        // Happy paths: top-level stays top-level, nested keeps its parent
        // prefix, encoding happens backend-side.
        assert_eq!(
            guard_rename(&tree, "Pai", "Novo"),
            Ok(("Pai".to_string(), "Novo".to_string()))
        );
        assert_eq!(
            guard_rename(&tree, "Pai/Sub", "Novo"),
            Ok(("Pai/Sub".to_string(), "Pai/Novo".to_string()))
        );
        assert_eq!(
            guard_rename(&tree, "Pai2", "Novo2"),
            Ok(("Pai2".to_string(), "Novo2".to_string()))
        );
        assert_eq!(
            guard_rename(&tree, "Pai/Sub", "Café"),
            Ok(("Pai/Sub".to_string(), "Pai/Caf&AOk-".to_string()))
        );
    }

    /// The delete guard refuses INBOX / unknown / system-role / parents
    /// before any verb call, and gates non-empty folders behind the two
    /// confirm steps.
    #[test]
    fn guard_delete_refusals_and_confirm_gates() {
        let tree = folder_tree_full();
        assert_eq!(
            guard_delete(&tree, "INBOX", 0, false, None),
            Err("A INBOX não pode ser excluída — ela é fixa do servidor.".to_string())
        );
        assert_eq!(
            guard_delete(&tree, "Fantasma", 0, false, None),
            Err("Essa pasta não existe mais na lista — atualize a lista e tente de novo.".to_string())
        );
        // System-role folders refuse backend-side too (same schema as
        // guard_rename — direct IPC invoke cannot delete Trash/Sent/Drafts).
        assert_eq!(
            guard_delete(&tree, "Lixeira", 0, false, None),
            Err("Lixeira do sistema — a exclusão não é permitida.".to_string())
        );
        // Parent with a child refuses loudly (move-out guidance, T-11-06).
        assert_eq!(
            guard_delete(&tree, "Pai", 0, false, None),
            Err("A pasta Pai tem subpastas — exclua ou mova as subpastas primeiro.".to_string())
        );
        // Lookalike `Pai2` is not a child of `Pai` — proceeds.
        assert_eq!(
            guard_delete(&tree, "Pai2", 0, false, None),
            Ok(DeleteDecision::Proceed {
                wire: "Pai2".to_string(),
                display: "Pai2".to_string(),
            })
        );
        // Non-empty without confirmation: count gate, zero verbs.
        assert_eq!(
            guard_delete(&tree, "Pai2", 5, false, None),
            Ok(DeleteDecision::NeedCount(5))
        );
        // Confirmed but typed name mismatches: typed gate, zero verbs.
        assert_eq!(
            guard_delete(&tree, "Pai2", 5, true, Some("outra")),
            Ok(DeleteDecision::NeedTyped("Pai2".to_string()))
        );
        assert_eq!(
            guard_delete(&tree, "Pai2", 5, true, None),
            Ok(DeleteDecision::NeedTyped("Pai2".to_string()))
        );
        // Confirmed with the exact display name: proceeds.
        assert_eq!(
            guard_delete(&tree, "Pai2", 5, true, Some("Pai2")),
            Ok(DeleteDecision::Proceed {
                wire: "Pai2".to_string(),
                display: "Pai2".to_string(),
            })
        );
    }

    /// `\Noselect` detection is spelling-agnostic (one- and two-backslash).
    #[test]
    fn noselect_attr_spellings() {
        assert!(has_noselect_attr(&["\\NoSelect".to_string()]));
        assert!(has_noselect_attr(&["\\\\NoSelect".to_string()]));
        assert!(!has_noselect_attr(&["\\HasNoChildren".to_string()]));
        assert!(!has_noselect_attr(&[]));
    }

    /// Folder error mapping per op: offline loud, exists repeated, DELETE
    /// and probe copies exact.
    #[test]
    fn map_folder_error_copies_per_op() {
        let offline = SyncError::Protocol("SessionManager reconnect: reset".to_string());
        assert_eq!(
            map_folder_error(&offline, "delete", "X"),
            "Sem conexão — pastas só podem ser alteradas online. Tente de novo ao reconectar.".to_string()
        );
        let exists = SyncError::Protocol("RENAME a -> b: NO already exists".to_string());
        assert_eq!(
            map_folder_error(&exists, "rename", "a"),
            "Já existe uma pasta com esse nome.".to_string()
        );
        let denied = SyncError::Protocol("DELETE Velha: NO cannot delete".to_string());
        assert_eq!(
            map_folder_error(&denied, "delete", "Velha"),
            "O servidor não permitiu excluir Velha: IMAP protocol error: DELETE Velha: NO cannot delete.".to_string()
        );
        let probe = SyncError::Protocol("LIST failed: boom".to_string());
        assert!(
            map_folder_error(&probe, "probe", "X").starts_with("Não foi possível ler as pastas do servidor:")
        );
        assert!(
            map_folder_error(&probe, "rename", "X").starts_with("Não foi possível renomear a pasta:")
        );
    }

    /// Cached-tree fixture with a non-ASCII parent (RAW wire name, as the
    /// refresh caches it): `Café` → `Caf&AOk-`.
    fn folder_tree_nonascii_parent() -> Vec<queries::MailboxRow> {
        vec![
            queries::MailboxRow {
                id: 1,
                name: "INBOX".to_string(),
                display_name: "INBOX".to_string(),
                delimiter: "".to_string(),
                role: "inbox".to_string(),
                attributes: "".to_string(),
                uid_validity: 100,
                uid_next: 1,
                last_sync_at: None,
                unread_count: 0,
                unseen_count: 0,
            },
            queries::MailboxRow {
                id: 2,
                name: "Caf&AOk-".to_string(),
                display_name: "Café".to_string(),
                delimiter: "/".to_string(),
                role: "".to_string(),
                attributes: "".to_string(),
                uid_validity: 100,
                uid_next: 1,
                last_sync_at: None,
                unread_count: 0,
                unseen_count: 0,
            },
            queries::MailboxRow {
                id: 3,
                name: "Caf&AOk-/Sub".to_string(),
                display_name: "Café/Sub".to_string(),
                delimiter: "/".to_string(),
                role: "".to_string(),
                attributes: "".to_string(),
                uid_validity: 100,
                uid_next: 1,
                last_sync_at: None,
                unread_count: 0,
                unseen_count: 0,
            },
        ]
    }

    /// Only the user-typed leaf is encoded: an already-encoded non-ASCII
    /// parent passes through byte-identical (re-encoding would corrupt the
    /// `&…-` shift into `&-…-` and CREATE on the wrong name).
    #[test]
    fn prepare_create_wire_never_reencodes_parent() {
        let tree = folder_tree_nonascii_parent();
        assert_eq!(
            prepare_create_wire(&tree, Some("Caf&AOk-"), "Sub2", "/"),
            Ok("Caf&AOk-/Sub2".to_string())
        );
        // Non-ASCII leaf under a non-ASCII parent: parent bytes intact,
        // leaf encoded.
        assert_eq!(
            prepare_create_wire(&tree, Some("Caf&AOk-"), "Té", "/"),
            Ok("Caf&AOk-/T&AOk-".to_string())
        );
    }

    /// Rename keeps a non-ASCII parent prefix byte-identical (same
    /// no-re-encode rule as create).
    #[test]
    fn guard_rename_never_reencodes_parent_prefix() {
        let tree = folder_tree_nonascii_parent();
        assert_eq!(
            guard_rename(&tree, "Caf&AOk-/Sub", "Novo"),
            Ok(("Caf&AOk-/Sub".to_string(), "Caf&AOk-/Novo".to_string()))
        );
        assert_eq!(
            guard_rename(&tree, "Caf&AOk-", "Novo"),
            Ok(("Caf&AOk-".to_string(), "Novo".to_string()))
        );
    }

    /// Pre-M6 cache rows (empty delimiter) with a hierarchical wire name
    /// infer the unambiguous delimiter instead of promoting to top level;
    /// ambiguous (`/` + `.`) wires refuse with the refresh copy.
    #[test]
    fn guard_rename_infers_missing_delimiter() {
        let mut tree = folder_tree();
        tree.push(queries::MailboxRow {
            id: 3,
            name: "Pai/Sub".to_string(),
            display_name: "Pai/Sub".to_string(),
            delimiter: "".to_string(),
            role: "".to_string(),
            attributes: "".to_string(),
            uid_validity: 100,
            uid_next: 1,
            last_sync_at: None,
            unread_count: 0,
            unseen_count: 0,
        });
        assert_eq!(
            guard_rename(&tree, "Pai/Sub", "Novo"),
            Ok(("Pai/Sub".to_string(), "Pai/Novo".to_string()))
        );
        // Ambiguous hierarchy: refuse, do not guess the parent.
        tree.push(queries::MailboxRow {
            id: 4,
            name: "A/B.C".to_string(),
            display_name: "A/B.C".to_string(),
            delimiter: "".to_string(),
            role: "".to_string(),
            attributes: "".to_string(),
            uid_validity: 100,
            uid_next: 1,
            last_sync_at: None,
            unread_count: 0,
            unseen_count: 0,
        });
        assert_eq!(
            guard_rename(&tree, "A/B.C", "Novo"),
            Err("Essa pasta não existe mais na lista — atualize a lista e tente de novo.".to_string())
        );
    }
}