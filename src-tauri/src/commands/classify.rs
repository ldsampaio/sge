//! Classification Tauri commands (Phase 17): manual single classify,
//! status surface, and the behind-sync hook + drain trigger.
//!
//! The hook only ENQUEUES (idempotent, excluded folders skipped); draining
//! runs detached under `ClassifyGate`. Sync never awaits either.

use std::sync::Arc;

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::classify::bridge::Bridge;
use crate::classify::suggest::DEFAULT_THRESHOLD;
use crate::classify::worker::{self, ClassificationReady, ClassifyGate};
use crate::store::queries;
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct ClassifyStatusPayload {
    pub sidecar: String,
    pub pending: i64,
    pub excluded: Vec<String>,
}

/// Queue depth + sidecar state + exclusions. Never contains key material.
#[tauri::command]
pub async fn classify_status(
    state: State<'_, AppState>,
) -> Result<ClassifyStatusPayload, String> {
    // Short critical sections, never across await.
    let (pending, excluded) = {
        let guard = state.store.lock().unwrap();
        let conn = guard.conn();
        let pending = queries::queue_depth(conn).map_err(|e| e.to_string())?;
        let excluded = queries::excluded_folders(conn).map_err(|e| e.to_string())?;
        (pending, excluded)
    };
    let sidecar = {
        let guard = state.sidecar.lock().unwrap();
        match guard.as_ref() {
            Some(sup) => match sup.status() {
                crate::sidecar::SidecarStatus::Running { .. } => "running".to_string(),
                crate::sidecar::SidecarStatus::Starting { .. } => "starting".to_string(),
                crate::sidecar::SidecarStatus::Down => "down".to_string(),
                crate::sidecar::SidecarStatus::Stopped => "stopped".to_string(),
            },
            None => "stopped".to_string(),
        }
    };
    Ok(ClassifyStatusPayload { sidecar, pending, excluded })
}

/// Manually classify (or reclassify) one message. Works on excluded
/// folders — explicit user intent overrides the automatic skip.
///
/// Lock discipline: the store Mutex is locked only in two short sync
/// sections (field fetch, label persist) — never across the bridge await,
// so the command future stays Send.
#[tauri::command]
pub async fn classify_message(
    state: State<'_, AppState>,
    message_id: u64,
) -> Result<ClassificationReady, String> {
    let store = state.store.clone();
    let sup = state
        .sidecar
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "classifier is not running".to_string())?;
    let (url, key) = sup.bridge_params();
    // Short lock: fetch cached fields only.
    let (subject, snippet, from_addr, has_attachments) = {
        let guard = store.lock().unwrap();
        queries::message_classify_fields(guard.conn(), message_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "message not found in local cache".to_string())?
    };
    let domain = from_addr.rsplit('@').next().unwrap_or("").to_string();
    let is_reply = subject.to_lowercase().starts_with("re:");
    let bridge = Bridge::new(&url, key.to_string()).map_err(|e| e.to_string())?;
    // No locks held across this await.
    let ready = worker::suggest_label(
        &bridge,
        &subject,
        &snippet,
        &domain,
        has_attachments,
        is_reply,
        DEFAULT_THRESHOLD,
    )
    .await
    .map_err(|e| e.to_string())?;
    // Short lock: persist only.
    {
        let guard = store.lock().unwrap();
        worker::persist_label(
            &guard,
            message_id,
            &ready.primary_id,
            ready.child_id.as_deref(),
            ready.secondary_id.as_deref(),
            ready.confidence,
            DEFAULT_THRESHOLD,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(ClassificationReady { message_id, ..ready })
}

/// Post-sync hook: enqueue unlabeled messages of `mailbox` (skips excluded
/// folders), then fire-and-forget a drain. Call from sync completion paths;
/// returns immediately, never fails the sync.
pub fn hook_after_sync(state: &AppState, mailbox: &str, app: &tauri::AppHandle) {
    {
        let guard = state.store.lock().unwrap();
        let conn = guard.conn();
        let _ = queries::seed_default_exclusions(conn);
    }
    let excluded = {
        let guard = state.store.lock().unwrap();
        queries::is_excluded(guard.conn(), mailbox).unwrap_or(false)
    };
    if excluded {
        return;
    }
    let (to_enqueue, uidv) = {
        let guard = state.store.lock().unwrap();
        let conn = guard.conn();
        let ids = queries::unlabeled_in_mailbox(conn, mailbox).unwrap_or_default();
        let uidv = conn
            .query_row(
                "SELECT uid_validity FROM mailboxes WHERE name = ?1",
                rusqlite::params![mailbox],
                |r| r.get::<_, u32>(0),
            )
            .unwrap_or(0);
        (ids, uidv)
    };
    {
        let guard = state.store.lock().unwrap();
        let conn = guard.conn();
        for id in to_enqueue {
            let _ = queries::enqueue_classify(conn, id, mailbox, uidv);
        }
    }
    trigger_drain(app);
}

/// Fire-and-forget drain under the shared gate. No-op when the sidecar is
/// absent (rows stay pending for the next trigger).
pub fn trigger_drain(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let sup = state.sidecar.lock().unwrap().clone();
    let Some(sup) = sup else { return };
    let store = state.store.clone();
    let gate: Arc<ClassifyGate> = state.classify_gate.clone();
    let app_h = app.clone();
    tauri::async_runtime::spawn(async move {
        let (url, key) = sup.bridge_params();
        let Ok(bridge) = Bridge::new(&url, key.to_string()) else {
            return;
        };
        let fetch_store = store.clone();
        let fetch = move |message_id: u64| {
            let guard = fetch_store.lock().unwrap();
            let conn = guard.conn();
            queries::message_classify_fields(conn, message_id)
                .ok()
                .flatten()
                .map(|(subject, snippet, from_addr, atts)| {
                    let domain = from_addr.rsplit('@').next().unwrap_or("").to_string();
                    let is_reply = subject.to_lowercase().starts_with("re:");
                    (subject, snippet, domain, atts, is_reply)
                })
        };
        let (labeled, _pending) =
            worker::drain(&store, &bridge, &gate, &fetch, 25, DEFAULT_THRESHOLD).await;
        if labeled > 0 {
            let _ = app_h.emit("classification-drained", labeled);
        }
    });
}
