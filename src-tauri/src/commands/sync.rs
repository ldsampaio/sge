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
use crate::imap::{AccountConfig, SecurityMode, SyncError};
use crate::imap::mutf7::{encode_modified_utf7, validate_leaf, FolderNameError};
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

/// Combined durable-queue depth (flag + delete/move) for `mailbox_id`.
/// Surfaced in every delete/move/expunge result and `sync_status` as the
/// pending indicator.
fn pending_depth(conn: &rusqlite::Connection, mailbox_id: u64) -> i64 {
    queries::outbox_count(conn, mailbox_id).unwrap_or(0)
        + queries::imap_outbox_count(conn, mailbox_id).unwrap_or(0)
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

/// Return the latest sync status from SQLite: last_sync_at + counts.
///
/// Read-only -- no IMAP round-trip. Used by the frontend to show
/// "Up-to-date <timestamp>" or "Offline -- last synced <timestamp>".
/// `pending_count` sums BOTH durable-outbox depths (flag toggles +
/// delete/move ops) for the pending indicator.
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
            // Hierarchy delimiter for tree rendering (M6).
            let _ = queries::set_mailbox_delimiter(conn, &folder.name, &folder.delimiter);
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
            // top-level creates join nothing.
            let parent_wire = parent.filter(|p| !p.is_empty());
            let delimiter = match &parent_wire {
                Some(p) => cached
                    .iter()
                    .find(|r| r.name == *p)
                    .map(|r| r.delimiter.clone())
                    .unwrap_or_default(),
                None => String::new(),
            };
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

/// Pure pre-wire guard for `create_folder` (Plan 11-01): validate, then
/// join `parent + delimiter + leaf`, encode, then exists pre-check against
/// the cached tree. Returns the RAW wire name or the exact pt-BR UI-SPEC
/// copy. Never touches the network — every `Err` here returns before any
/// verb call (T-11-02 hierarchy escape, T-11-03 INBOX variant).
fn prepare_create_wire(
    cached: &[queries::MailboxRow],
    parent: Option<&str>,
    leaf: &str,
    delimiter: &str,
) -> Result<String, String> {
    let leaf = leaf.trim();
    if let Err(e) = validate_leaf(leaf, delimiter) {
        return Err(match e {
            FolderNameError::Empty => "Dê um nome para a pasta.".to_string(),
            FolderNameError::ContainsDelimiter(d) => format!(
                "O nome não pode conter '{d}' — ele separa pastas. Crie uma pasta por vez."
            ),
            FolderNameError::ReservedInbox => {
                "INBOX é uma pasta reservada do servidor — escolha outro nome.".to_string()
            }
        });
    }
    let joined = match parent {
        Some(p) => format!("{p}{delimiter}{leaf}"),
        None => leaf.to_string(),
    };
    let wire = encode_modified_utf7(&joined);
    if cached.iter().any(|r| r.name == wire) {
        return Err("Já existe uma pasta com esse nome.".to_string());
    }
    Ok(wire)
}

/// Map a failed CREATE to plain language (Plan 11-01): offline is loud
/// (no queue exists), already-exists repeats the exists copy, anything
/// else surfaces the server detail instead of a bare NO.
fn map_create_error(e: &SyncError) -> String {
    let msg = e.to_string();
    if is_connectivity_error(&msg) {
        return "Sem conexão — pastas só podem ser alteradas online. Tente de novo ao reconectar."
            .to_string();
    }
    let lower = msg.to_lowercase();
    if lower.contains("already exist") || lower.contains("mailbox exists") {
        return "Já existe uma pasta com esse nome.".to_string();
    }
    format!("Não foi possível criar a pasta: {msg}")
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
    use crate::store::queries;
    use crate::store::Store;

    use super::{is_connectivity_error, map_create_error, pending_depth, prepare_create_wire};

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

    /// Cached-tree fixture for the folder guards: INBOX plus one
    /// hierarchical parent (delimiter `/`).
    fn folder_tree() -> Vec<queries::MailboxRow> {
        vec![
            queries::MailboxRow {
                id: 1,
                name: "INBOX".to_string(),
                display_name: "INBOX".to_string(),
                delimiter: "".to_string(),
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
                uid_validity: 100,
                uid_next: 1,
                last_sync_at: None,
                unread_count: 0,
                unseen_count: 0,
            },
        ]
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
}