//! Classification Tauri commands (Phase 17): manual single classify,
//! status surface, and the behind-sync hook + drain trigger.
//!
//! The hook only ENQUEUES (idempotent, excluded folders skipped); draining
//! runs detached under `ClassifyGate`. Sync never awaits either.

use std::sync::Arc;

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::classify::bridge::Bridge;
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
/// so the command future stays Send.
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
    let (subject, snippet, from_addr, has_attachments, threshold) = {
        let guard = store.lock().unwrap();
        let fields = queries::message_classify_fields(guard.conn(), message_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "message not found in local cache".to_string())?;
        let threshold = queries::effective_threshold(guard.conn());
        (fields.0, fields.1, fields.2, fields.3, threshold)
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
        threshold,
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
            threshold,
            ready.needs_review,
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
        let threshold = {
            let guard = store.lock().unwrap();
            queries::effective_threshold(guard.conn())
        };
        let (labeled, _pending) =
            worker::drain(&store, &bridge, &gate, &fetch, 25, threshold).await;
        if labeled > 0 {
            let _ = app_h.emit("classification-drained", labeled);
        }
    });
}

// ── Phase 18: confirm-gated filing commands ──────────────────────────

use crate::classify::filing::{self, ConfirmResult};
use crate::classify::taxonomy::load_default;
use crate::commands::sync::{load_account_config, manager_for};

/// Confirm a suggestion: suggest → confirm → MOVE into `Auto/<Top>/<Child>`.
///
/// Backend-enforced: refuses without a label row (nothing suggested),
/// refuses stale rows (taxonomy changed — reclassify first). Offline
/// confirms replay via `imap_outbox`. `allow_nest` answers the one-time
/// `Auto`-collision dialog (user-owned `Auto` without our marker).
#[tauri::command]
pub async fn confirm_suggestion(
    state: State<'_, AppState>,
    message_id: u64,
    allow_nest: Option<bool>,
) -> Result<ConfirmResult, String> {
    let store = state.store.clone();
    let tax = load_default().map_err(|e| e.to_string())?;
    // Short lock: plan only.
    let plan = {
        let guard = store.lock().unwrap();
        filing::plan_filing(guard.conn(), message_id, &tax).map_err(|e| e.to_string())?
    };
    if plan.needs_review {
        // Review items confirm too (the user judged them right) — the flag
        // is informational, not a refusal. Only missing/stale refuse.
    }
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let dest_wire = filing::ensure_auto_tree(&manager, &store, &plan, allow_nest.unwrap_or(false))
        .await
        .map_err(|e| e.to_string())?;
    filing::execute_filing_move(&manager, &store, &plan, &dest_wire)
        .await
        .map_err(|e| e.to_string())
}

/// Override a classification in one click: MOVE to the corrected folder +
/// log the override (pinned: the hook never re-suggests labeled mail, and
/// `latest_override` lets future syncs respect the correction).
#[tauri::command]
pub async fn override_label(
    state: State<'_, AppState>,
    message_id: u64,
    to_category: String,
) -> Result<ConfirmResult, String> {
    let store = state.store.clone();
    let tax = load_default().map_err(|e| e.to_string())?;
    let (dest_display, top_name, child_name) =
        filing::auto_path_for_id(&tax, &to_category).map_err(|e| e.to_string())?;
    let (mailbox, uid, from_id) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let label = crate::store::queries::get_label(conn, message_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "sem sugestão para este e-mail".to_string())?;
        let (mailbox, uid) = crate::store::queries::message_location(conn, message_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "message not found in local cache".to_string())?;
        (mailbox, uid, label.primary_id.clone())
    };
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let plan = filing::FilePlan {
        message_id,
        mailbox,
        uid,
        dest_display,
        top_name,
        child_name,
        threshold_used: 0.0,
        confidence: 1.0,
        needs_review: false,
    };
    let dest_wire = filing::ensure_auto_tree(&manager, &store, &plan, true)
        .await
        .map_err(|e| e.to_string())?;
    let result = filing::execute_filing_move(&manager, &store, &plan, &dest_wire)
        .await
        .map_err(|e| e.to_string())?;
    // Log + pin (short lock, after the move resolves).
    {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let _ = crate::store::queries::log_override(conn, message_id, &from_id, &to_category);
        let _ = crate::store::queries::upsert_label(
            conn, message_id, &to_category, None, 1.0, 0.0, false,
        );
    }
    Ok(result)
}

/// Dismiss a suggestion: mail stays put, review list hides it.
#[tauri::command]
pub async fn dismiss_suggestion(
    state: State<'_, AppState>,
    message_id: u64,
) -> Result<(), String> {
    let guard = state.store.lock().unwrap();
    crate::store::queries::dismiss_label(guard.conn(), message_id).map_err(|e| e.to_string())
}

/// Tune the confidence threshold (0–1). Future suggestions only.
#[tauri::command]
pub async fn set_confidence_threshold(
    state: State<'_, AppState>,
    value: f64,
) -> Result<(), String> {
    if !(0.0..=1.0).contains(&value) {
        return Err("limiar deve estar entre 0 e 1".to_string());
    }
    let guard = state.store.lock().unwrap();
    crate::store::queries::set_setting(guard.conn(), "threshold", &value.to_string())
        .map_err(|e| e.to_string())
}

/// Review list rows for the `A Classificar` view.
#[tauri::command]
pub async fn review_list(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<crate::store::queries::LabelRow>, String> {
    let guard = state.store.lock().unwrap();
    crate::store::queries::review_list(guard.conn(), limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

// ── Phase 18 UI data commands ────────────────────────────────────

use crate::classify::suggest::justification;

#[derive(Debug, Clone, Serialize)]
pub struct SuggestionDetail {
    pub message_id: u64,
    pub primary_id: String,
    pub primary_name: String,
    pub child_name: Option<String>,
    pub secondary_name: Option<String>,
    pub confidence: f64,
    pub needs_review: bool,
    pub stale: bool,
    pub dismissed: bool,
    pub dest_display: String,
    /// Transient redacted justification (never persisted).
    pub justification: String,
    pub tops: Vec<(String, String)>,
}

/// Everything the confirm surface needs for one message. None when the
/// message was never classified.
#[tauri::command]
pub async fn suggestion_detail(
    state: State<'_, AppState>,
    message_id: u64,
) -> Result<Option<SuggestionDetail>, String> {
    let tax = load_default().map_err(|e| e.to_string())?;
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let label = match crate::store::queries::get_label(conn, message_id)
        .map_err(|e| e.to_string())?
    {
        Some(l) => l,
        None => return Ok(None),
    };
    let dismissed = crate::store::queries::is_dismissed(conn, message_id)
        .map_err(|e| e.to_string())?;
    let fields = crate::store::queries::message_classify_fields(conn, message_id)
        .map_err(|e| e.to_string())?;
    let top_name = tax
        .categories
        .iter()
        .find(|c| c.id == label.primary_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| label.primary_id.clone());
    let (child_name, justification) = match fields {
        Some((subject, snippet, from_addr, atts)) => {
            let domain = from_addr.rsplit('@').next().unwrap_or("");
            let is_reply = subject.to_lowercase().starts_with("re:");
            let ev = crate::classify::evidence::build_evidence(
                &subject, &snippet, domain, atts, is_reply,
            );
            let kids: Vec<&crate::classify::taxonomy::Category> =
                crate::classify::taxonomy::children_of(&tax, &label.primary_id);
            let child = crate::classify::taxonomy::match_keywords(&ev, &kids)
                .into_iter()
                .next()
                .and_then(|(id, _)| {
                    tax.categories.iter().find(|c| c.id == id).map(|c| c.name.clone())
                });
            let just = crate::classify::redact::filter_output(&justification(
                &label.primary_id,
                &ev,
                &tax,
            ));
            (child, just)
        }
        None => (None, String::new()),
    };
    let secondary_name = label.secondary_id.as_ref().and_then(|sid| {
        tax.categories.iter().find(|c| &c.id == sid).map(|c| c.name.clone())
    });
    let dest_display = match &child_name {
        Some(c) => format!("Auto/{top_name}/{c}"),
        None => format!("Auto/{top_name}"),
    };
    let tops = tax
        .categories
        .iter()
        .filter(|c| c.parent.is_none())
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    Ok(Some(SuggestionDetail {
        message_id,
        primary_id: label.primary_id,
        primary_name: top_name,
        child_name,
        secondary_name,
        confidence: label.confidence,
        needs_review: label.needs_review,
        stale: label.stale,
        dismissed,
        dest_display,
        justification,
        tops,
    }))
}

#[derive(Debug, Clone, Serialize)]
pub struct MailboxLabel {
    pub uid: u32,
    pub primary_name: String,
    pub secondary_name: Option<String>,
    pub confidence: f64,
    pub needs_review: bool,
}

/// Bulk labels for list badges (one query, IDs resolved to names here).
#[tauri::command]
pub async fn mailbox_labels(
    state: State<'_, AppState>,
    mailbox: String,
) -> Result<Vec<MailboxLabel>, String> {
    let tax = load_default().map_err(|e| e.to_string())?;
    let guard = state.store.lock().unwrap();
    let rows = crate::store::queries::labels_in_mailbox(guard.conn(), &mailbox)
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(uid, l)| {
            let primary_name = tax
                .categories
                .iter()
                .find(|c| c.id == l.primary_id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| l.primary_id.clone());
            let secondary_name = l.secondary_id.as_ref().and_then(|sid| {
                tax.categories.iter().find(|c| &c.id == sid).map(|c| c.name.clone())
            });
            MailboxLabel {
                uid,
                primary_name,
                secondary_name,
                confidence: l.confidence,
                needs_review: l.needs_review,
            }
        })
        .collect())
}

/// Suggestion detail addressed by (mailbox, uid) — the reader pane knows
/// UIDs, not row ids.
#[tauri::command]
pub async fn suggestion_for_uid(
    state: State<'_, AppState>,
    mailbox: String,
    uid: u32,
) -> Result<Option<SuggestionDetail>, String> {
    let message_id = {
        let guard = state.store.lock().unwrap();
        match crate::store::queries::find_message_id(guard.conn(), mailbox_id_of(guard.conn(), &mailbox)?, uid)
            .map_err(|e| e.to_string())?
        {
            Some(id) => id,
            None => return Ok(None),
        }
    };
    suggestion_detail(state, message_id).await
}

fn mailbox_id_of(conn: &rusqlite::Connection, mailbox: &str) -> Result<u64, String> {
    crate::store::queries::mailbox_id(conn, mailbox)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "pasta desconhecida".to_string())
}

// ── Phase 19: taxonomy editor commands ───────────────────────────

use crate::classify::taxedit::{self, EditError};
use crate::classify::taxonomy::Taxonomy;
use crate::commands::sync::{guard_delete, guard_rename, refresh_mailbox_tree};

fn read_taxonomy(conn: &rusqlite::Connection) -> Result<Taxonomy, String> {
    let row = crate::store::queries::get_taxonomy(conn).map_err(|e| e.to_string())?;
    match row {
        Some(t) => serde_json::from_str::<Taxonomy>(&t.json).map_err(|e| format!("taxonomia corrompida: {e}")),
        None => crate::classify::taxonomy::load_default().map_err(|e| e.to_string()),
    }
}

/// Persist a structural taxonomy change: validate → version++ → install →
/// stale-flag vanished ids. Returns the new version.
fn persist_taxonomy(conn: &rusqlite::Connection, tax: &Taxonomy) -> Result<u32, String> {
    let mut tax = tax.clone();
    tax.version += 1;
    let json = serde_json::to_string(&tax).map_err(|e| e.to_string())?;
    crate::store::queries::install_taxonomy(conn, tax.version, &tax.name, &json)
        .map_err(|e| e.to_string())?;
    let ids: Vec<&str> = tax.categories.iter().map(|c| c.id.as_str()).collect();
    crate::store::queries::mark_stale_unknown(conn, &ids).map_err(|e| e.to_string())?;
    Ok(tax.version)
}

#[derive(Debug, Clone, Serialize)]
pub struct TaxonomyView {
    pub version: u32,
    pub name: String,
    pub categories: Vec<TaxonomyCategoryView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaxonomyCategoryView {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub keywords: Vec<String>,
    pub rule: String,
    pub labels: i64,
}

/// Full taxonomy + per-category label counts for the editor.
#[tauri::command]
pub async fn list_taxonomy(state: State<'_, AppState>) -> Result<TaxonomyView, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let tax = read_taxonomy(conn)?;
    let mut categories = Vec::new();
    for c in &tax.categories {
        let labels = crate::store::queries::labels_using(conn, &c.id).map_err(|e| e.to_string())?;
        categories.push(TaxonomyCategoryView {
            id: c.id.clone(),
            name: c.name.clone(),
            parent: c.parent.clone(),
            keywords: c.keywords.clone(),
            rule: c.rule.clone(),
            labels,
        });
    }
    Ok(TaxonomyView { version: tax.version, name: tax.name.clone(), categories })
}

fn edit_err(e: EditError) -> String {
    e.to_string()
}

/// Add a category (top when `parent` is null). Creates the folder immediately.
#[tauri::command]
pub async fn add_category(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    parent: Option<String>,
    name: String,
    keywords: Vec<String>,
    rule: String,
) -> Result<String, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    // 1. Pure transform + persist (brief lock).
    let (id, top_name, child_name) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mut tax = read_taxonomy(conn)?;
        let id = taxedit::apply_add(&mut tax, parent.as_deref(), &name, keywords, &rule)
            .map_err(edit_err)?;
        let version = persist_taxonomy(conn, &tax)?;
        let _ = version;
        let cat = tax.categories.iter().find(|c| c.id == id).unwrap().clone();
        let (top_name, child_name) = match &cat.parent {
            None => (cat.name.clone(), None),
            Some(pid) => {
                let top = tax.categories.iter().find(|c| &c.id == pid).unwrap().name.clone();
                (top, Some(cat.name.clone()))
            }
        };
        (id, top_name, child_name)
    };
    // 2. CREATE the folder (online-only, loud offline copy like create_folder).
    let plan = filing::FilePlan {
        message_id: 0,
        mailbox: String::new(),
        uid: 0,
        dest_display: String::new(),
        top_name,
        child_name,
        threshold_used: 0.0,
        confidence: 0.0,
        needs_review: false,
    };
    filing::ensure_auto_tree(&manager, &store, &plan, true)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app;
    Ok(id)
}

/// Rename a category (display name only — ids/labels untouched) + RENAME
/// the folder via the Phase 11 guarded path (guards → verb → cache
/// migrate → re-LIST, same shape as `rename_folder`).
#[tauri::command]
pub async fn rename_category(
    state: State<'_, AppState>,
    id: String,
    new_name: String,
) -> Result<(), String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let new_leaf = new_name.trim().to_string();
    if new_leaf.is_empty() {
        return Err("dê um nome para a categoria".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Snapshot + resolve old wire from the CURRENT display path.
            let (cached, old_wire) = {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let tax = read_taxonomy(conn)?;
                let old_display = old_display_path(&tax, &id);
                if old_display.is_empty() {
                    return Err("categoria desconhecida".to_string());
                }
                let cached = crate::store::queries::list_mailboxes(conn)
                    .map_err(|e| format!("store: {e}"))?;
                let old_wire = cached
                    .iter()
                    .find(|r| r.display_name == old_display)
                    .map(|r| r.name.clone())
                    .ok_or_else(|| {
                        "pasta da categoria não encontrada — sincronize primeiro".to_string()
                    })?;
                (cached, old_wire)
            };
            // 2. Pure guards (no verb has fired on Err).
            let (old_w, new_w) =
                guard_rename(&cached, &old_wire, &new_leaf).map_err(|e| e.to_string())?;
            // 3. Taxonomy rename (pure) — persisted before the verb so a
            // verb failure leaves taxonomy+labels consistent (folder rename
            // retries via the next refresh; ids never changed).
            {
                let guard = store.lock().unwrap();
                let conn = guard.conn();
                let mut tax = read_taxonomy(conn)?;
                taxedit::apply_rename(&mut tax, &id, &new_leaf).map_err(edit_err)?;
                persist_taxonomy(conn, &tax)?;
            }
            // 4. Folder RENAME verb + cache migrate + re-LIST.
            manager
                .rename_mailbox_in(&old_w, &new_w)
                .await
                .map_err(|e| format!("não foi possível renomear: {e}"))?;
            {
                let guard = store.lock().unwrap();
                // Delimiter from the renamed row's own cached entry.
                let delimiter = crate::store::queries::list_mailboxes(guard.conn())
                    .map(|rows| {
                        rows.iter()
                            .find(|r| r.name == old_w)
                            .map(|r| r.delimiter.clone())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let _ = crate::store::queries::rename_mailbox_cache(
                    guard.conn(),
                    &old_w,
                    &new_w,
                    &delimiter,
                );
            }
            refresh_mailbox_tree(&manager, &store).await?;
            Ok::<_, String>(())
        })
    })
    .await
    .map_err(|e| format!("internal error: rename task failed ({e})"))??;
    Ok(())
}

fn old_display_path(tax: &Taxonomy, id: &str) -> String {
    filing::auto_path_for_id(tax, id).map(|(d, _, _)| d).unwrap_or_default()
}

/// Merge `from` into `into` (siblings): move all mail, remap labels,
/// delete the source folder + category.
#[tauri::command]
pub async fn merge_categories(
    state: State<'_, AppState>,
    from: String,
    into: String,
) -> Result<usize, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    // 1. Resolve folders + mail (brief lock).
    let tax = {
        let guard = store.lock().unwrap();
        read_taxonomy(guard.conn())?
    };
    let (from_display, _, _) = filing::auto_path_for_id(&tax, &from).map_err(|e| e.to_string())?;
    let (into_display, _, _) = filing::auto_path_for_id(&tax, &into).map_err(|e| e.to_string())?;
    // Locate wire names + mailboxes rows.
    let (from_wire, into_wire, uids, mailbox_name) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let cached = crate::store::queries::list_mailboxes(conn).map_err(|e| e.to_string())?;
        let wire_of = |display: &str| {
            cached
                .iter()
                .find(|r| r.display_name == *display)
                .map(|r| r.name.clone())
        };
        let from_wire = wire_of(&from_display);
        let into_wire = wire_of(&into_display).ok_or_else(|| {
            "pasta de destino não existe — confirme a categoria antes de fundir".to_string()
        })?;
        // Mail currently filed under the source folder (by display or wire).
        let (uids, mailbox_name) = match from_wire {
            Some(ref w) => {
                let mb = crate::store::queries::mailbox_id(conn, w).map_err(|e| e.to_string())?;
                match mb {
                    Some(mb_id) => (
                        crate::store::queries::all_local_uids(conn, mb_id)
                            .map_err(|e| e.to_string())?,
                        w.clone(),
                    ),
                    None => (vec![], w.clone()),
                }
            }
            None => (vec![], String::new()),
        };
        (from_wire, into_wire, uids, mailbox_name)
    };
    // 2. Move every mail old → new (outbox-durable each).
    let mut moved = 0usize;
    for uid in uids {
        filing::move_one_uid(
            &manager,
            &store,
            &mailbox_name,
            uid,
            &into_wire,
            &format!("Fundida em {into_display}"),
        )
        .await
        .map_err(|e| e.to_string())?;
        moved += 1;
    }
    // 3. Delete the (now empty) source folder when it exists.
    if let Some(w) = from_wire {
        let cached = {
            let guard = store.lock().unwrap();
            crate::store::queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
        };
        match guard_delete(&cached, &w, 0, true, None).map_err(|e| e.to_string())? {
            crate::commands::sync::DeleteDecision::Proceed { wire, .. } => {
                manager.delete_mailbox_in(&wire).await.map_err(|e| e.to_string())?;
            }
            other => return Err(format!("exclusão bloqueada: {other:?}")),
        }
        refresh_mailbox_tree(&manager, &store).await?;
    }
    // 4. Taxonomy merge + label remap + version++ (brief lock).
    {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mut tax = read_taxonomy(conn)?;
        taxedit::apply_merge(&mut tax, &from, &into).map_err(edit_err)?;
        persist_taxonomy(conn, &tax)?;
        crate::store::queries::remap_label_ids(conn, &from, &into).map_err(|e| e.to_string())?;
    }
    Ok(moved)
}

/// Delete a category: mail moves to `reassign_to` first, then folder DELETE
/// guards, label remap, taxonomy update. Refuses when mail would orphan.
#[tauri::command]
pub async fn delete_category(
    state: State<'_, AppState>,
    id: String,
    reassign_to: String,
) -> Result<usize, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let tax = {
        let guard = store.lock().unwrap();
        read_taxonomy(guard.conn())?
    };
    if id == reassign_to {
        return Err("escolha outra categoria para receber os e-mails".to_string());
    }
    let (from_display, _, _) = filing::auto_path_for_id(&tax, &id).map_err(|e| e.to_string())?;
    let (into_display, _, _) =
        filing::auto_path_for_id(&tax, &reassign_to).map_err(|e| e.to_string())?;
    let (from_wire, into_wire, uids, mailbox_name) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let cached = crate::store::queries::list_mailboxes(conn).map_err(|e| e.to_string())?;
        let wire_of = |display: &str| {
            cached.iter().find(|r| r.display_name == *display).map(|r| r.name.clone())
        };
        let into_wire = wire_of(&into_display)
            .ok_or_else(|| "pasta de destino não existe".to_string())?;
        let (uids, mailbox_name) = match wire_of(&from_display) {
            Some(w) => {
                let uids = match crate::store::queries::mailbox_id(conn, &w).map_err(|e| e.to_string())? {
                    Some(mb_id) => crate::store::queries::all_local_uids(conn, mb_id)
                        .map_err(|e| e.to_string())?,
                    None => vec![],
                };
                (uids, w)
            }
            None => (vec![], String::new()),
        };
        (wire_of(&from_display), into_wire, uids, mailbox_name)
    };
    let mut moved = 0usize;
    for uid in uids {
        filing::move_one_uid(&manager, &store, &mailbox_name, uid, &into_wire, &format!("Movido para {into_display}"))
            .await
            .map_err(|e| e.to_string())?;
        moved += 1;
    }
    if let Some(w) = from_wire {
        let cached = {
            let guard = store.lock().unwrap();
            crate::store::queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
        };
        match guard_delete(&cached, &w, 0, true, None).map_err(|e| e.to_string())? {
            crate::commands::sync::DeleteDecision::Proceed { wire, .. } => {
                manager.delete_mailbox_in(&wire).await.map_err(|e| e.to_string())?;
            }
            other => return Err(format!("exclusão bloqueada: {other:?}")),
        }
        refresh_mailbox_tree(&manager, &store).await?;
    }
    {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mut tax = read_taxonomy(conn)?;
        taxedit::apply_delete(&mut tax, &id).map_err(edit_err)?;
        persist_taxonomy(conn, &tax)?;
        crate::store::queries::remap_label_ids(conn, &id, &reassign_to)
            .map_err(|e| e.to_string())?;
        let orphans = crate::store::queries::labels_using(conn, &id).map_err(|e| e.to_string())?;
        if orphans > 0 {
            return Err(format!("{orphans} rótulos órfãos — migração incompleta"));
        }
    }
    Ok(moved)
}

/// Edit keywords/rules in place (no version bump, no folder touch).
#[tauri::command]
pub async fn update_category_keywords(
    state: State<'_, AppState>,
    id: String,
    keywords: Vec<String>,
    rule: String,
) -> Result<(), String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let mut tax = read_taxonomy(conn)?;
    taxedit::apply_keywords(&mut tax, &id, keywords, &rule).map_err(edit_err)?;
    let json = serde_json::to_string(&tax).map_err(|e| e.to_string())?;
    crate::store::queries::install_taxonomy(conn, tax.version, &tax.name, &json)
        .map_err(|e| e.to_string())
}

/// Import taxonomy JSON: sanitize → validate → version++ → stale-flag.
/// Invalid files rejected with a plain-language reason; nothing changes.
#[tauri::command]
pub async fn import_taxonomy(
    state: State<'_, AppState>,
    json: String,
) -> Result<u32, String> {
    let tax = taxedit::sanitize_import(&json).map_err(edit_err)?;
    let guard = state.store.lock().unwrap();
    persist_taxonomy(guard.conn(), &tax)
}

/// Export the current taxonomy JSON (portable across machines).
#[tauri::command]
pub async fn export_taxonomy(state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let tax = read_taxonomy(conn)?;
    serde_json::to_string_pretty(&tax).map_err(|e| e.to_string())
}

// ── Phase 20: batch reorganization commands ──────────────────────

use crate::classify::batch::{self, BatchProgress, BatchReport};

/// Start (or resume) a whole-account batch run. Unconfirmed by design —
/// one explicit op with progress + report + undo. Returns the run id.
#[tauri::command]
pub async fn batch_classify(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    scope: Option<String>,
    resume_run: Option<i64>,
) -> Result<i64, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let sup = state.sidecar.lock().unwrap().clone()
        .ok_or_else(|| "classificador parado — inicie o app com o sidecar".to_string())?;
    let (url, key) = sup.bridge_params();
    let bridge =
        crate::classify::bridge::Bridge::new(&url, key.to_string()).map_err(|e| e.to_string())?;
    let cancel = state.batch_cancel.clone();
    cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    // Pre-create the full tree up front (dialog-off means no mid-run asks).
    batch::ensure_full_tree(&manager, &store).await?;
    // Run id: fresh or resumed.
    let run_id = {
        let guard = store.lock().unwrap();
        match resume_run {
            Some(id) => id,
            None => crate::store::queries::create_batch_run(guard.conn()).map_err(|e| e.to_string())?,
        }
    };
    let tax = load_default().map_err(|e| e.to_string())?;
    let threshold = {
        let guard = store.lock().unwrap();
        crate::store::queries::effective_threshold(guard.conn())
    };
    // Candidates (resume skips journaled).
    let mut candidates = {
        let guard = store.lock().unwrap();
        batch::collect_candidates(guard.conn(), scope.as_deref())?
    };
    if resume_run.is_some() {
        let guard = store.lock().unwrap();
        let done = batch::journaled_ids(guard.conn(), run_id)?;
        candidates.retain(|(mid, _, _)| !done.contains(mid));
    }
    // Folder epochs at run start (per-chunk UIDVALIDITY re-check).
    let epochs: std::collections::HashMap<String, u32> = {
        let guard = store.lock().unwrap();
        let rows: Vec<(String, u32)> = guard
            .conn()
            .prepare("SELECT name, uid_validity FROM mailboxes")
            .map_err(|e| e.to_string())?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        rows.into_iter().collect()
    };
    let total = candidates.len();
    let app_h = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut done = 0usize;
        let mut moved = 0usize;
        let mut review = 0usize;
        let mut chunk_no = 0usize;
        for chunk in candidates.chunks(batch::BATCH_CHUNK) {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            // Per-chunk UIDVALIDITY re-check: abort folders that shifted.
            let mut live_chunk: Vec<(u64, String, u32)> = Vec::new();
            for (mid, mb, uid) in chunk {
                let epoch_now: u32 = {
                    let guard = store.lock().unwrap();
                    guard
                        .conn()
                        .query_row(
                            "SELECT uid_validity FROM mailboxes WHERE name = ?1",
                            rusqlite::params![mb],
                            |r| r.get(0),
                        )
                        .unwrap_or(0)
                };
                if epochs.get(mb).copied().unwrap_or(0) != epoch_now {
                    continue; // UIDVALIDITY shifted mid-run: skip (abort-to-resync for this folder)
                }
                live_chunk.push((*mid, mb.clone(), *uid));
            }
            for (mid, _mb, _uid) in live_chunk {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // Fields → suggest (no confirm — batch auto-confirms ≥ threshold).
                let fields = {
                    let guard = store.lock().unwrap();
                    crate::store::queries::message_classify_fields(guard.conn(), mid)
                        .ok()
                        .flatten()
                };
                let Some((subject, snippet, from_addr, atts)) = fields else {
                    done += 1;
                    continue;
                };
                let domain = from_addr.rsplit('@').next().unwrap_or("").to_string();
                let is_reply = subject.to_lowercase().starts_with("re:");
                let ready = match crate::classify::worker::suggest_label(
                    &bridge, &subject, &snippet, &domain, atts, is_reply, threshold,
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        done += 1;
                        continue;
                    }
                };
                if ready.needs_review {
                    // Below threshold/vetoed → remainder (never moved).
                    let guard = store.lock().unwrap();
                    let _ = crate::store::queries::upsert_label(
                        guard.conn(), mid, &ready.primary_id,
                        ready.secondary_id.as_deref(), ready.confidence, threshold,
                        true,
                    );
                    review += 1;
                    done += 1;
                    continue;
                }
                // Destination + journal BEFORE the verb.
                let dest_display = match &ready.child_id {
                    Some(_) => {
                        let top_name = tax.categories.iter()
                            .find(|c| c.id == ready.primary_id).map(|c| c.name.clone())
                            .unwrap_or_else(|| ready.primary_id.clone());
                        let child_name = tax.categories.iter()
                            .find(|c| Some(c.id.clone()) == ready.child_id).map(|c| c.name.clone());
                        match child_name {
                            Some(cn) => format!("Auto/{top_name}/{cn}"),
                            None => format!("Auto/{top_name}"),
                        }
                    }
                    None => {
                        let top_name = tax.categories.iter()
                            .find(|c| c.id == ready.primary_id).map(|c| c.name.clone())
                            .unwrap_or_else(|| ready.primary_id.clone());
                        format!("Auto/{top_name}")
                    }
                };
                // Resolve wire: reuse ensure path per message? Tree is
                // pre-created — wire = display with delimiter. Resolve
                // delimiter + existing wire from cache.
                let (from_folder, dest_wire) = {
                    let guard = store.lock().unwrap();
                    let conn = guard.conn();
                    let loc = crate::store::queries::message_location(conn, mid).ok().flatten();
                    let cached = crate::store::queries::list_mailboxes(conn).ok().unwrap_or_default();
                    match loc {
                        Some((mb, _)) => {
                            let delim = cached.first().map(|r| r.delimiter.clone()).unwrap_or_default();
                            let wire = if delim.is_empty() {
                                dest_display.replace('/', "/")
                            } else {
                                // Encode leaves (non-ASCII) like prepare_create_wire does.
                                let parts: Vec<&str> = dest_display.split('/').collect();
                                let mut w = String::new();
                                for (i, p) in parts.iter().enumerate() {
                                    if i > 0 {
                                        w.push_str(&delim);
                                    }
                                    // Only encode non-ASCII leaves; ASCII passes through.
                                    w.push_str(&crate::imap::mutf7::encode_modified_utf7(p));
                                }
                                w
                            };
                            (mb, wire)
                        }
                        None => continue,
                    }
                };
                {
                    let guard = store.lock().unwrap();
                    let _ = batch::journal_intent(
                        guard.conn(), run_id, mid, &from_folder, &dest_wire,
                        &ready.primary_id, chunk_no,
                    );
                }
                // Current uid may have shifted; re-read location for the verb.
                let verb_loc = {
                    let guard = store.lock().unwrap();
                    crate::store::queries::message_location(guard.conn(), mid).ok().flatten()
                };
                if let Some((mb_now, uid_now)) = verb_loc {
                    if mb_now == dest_wire {
                        let guard = store.lock().unwrap();
                        let _ = batch::journal_moved(guard.conn(), run_id, mid);
                        moved += 1;
                    } else if crate::classify::filing::move_one_uid(
                        &manager, &store, &mb_now, uid_now, &dest_wire,
                        &format!("Arquivado em {dest_display}"),
                    )
                    .await
                    .is_ok()
                    {
                        let guard = store.lock().unwrap();
                        let _ = batch::journal_moved(guard.conn(), run_id, mid);
                        let _ = crate::store::queries::upsert_label(
                            guard.conn(), mid, &ready.primary_id,
                            ready.secondary_id.as_deref(), ready.confidence, threshold, false,
                        );
                        moved += 1;
                    }
                }
                done += 1;
            }
            chunk_no += 1;
            let _ = app_h.emit(
                "batch-progress",
                BatchProgress {
                    run_id,
                    done,
                    total,
                    moved,
                    review,
                    state: "running".to_string(),
                },
            );
        }
        let interrupted = cancel.load(std::sync::atomic::Ordering::SeqCst);
        let totals = serde_json::json!({ "done": done, "moved": moved, "review": review }).to_string();
        {
            let guard = store.lock().unwrap();
            let _ = crate::store::queries::finish_batch_run(
                guard.conn(), run_id,
                if interrupted { "interrupted" } else { "done" }, &totals,
            );
        }
        let _ = app_h.emit(
            "batch-progress",
            BatchProgress {
                run_id,
                done,
                total,
                moved,
                review,
                state: if interrupted { "interrupted".to_string() } else { "done".to_string() },
            },
        );
    });
    Ok(run_id)
}

/// Cancel the running batch (cooperative, chunk boundary).
#[tauri::command]
pub async fn cancel_batch(state: State<'_, AppState>) -> Result<(), String> {
    state.batch_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Persisted per-category report + remainder for a run.
#[tauri::command]
pub async fn batch_report(
    state: State<'_, AppState>,
    run_id: i64,
) -> Result<BatchReport, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let (state_s, totals): (String, String) = conn
        .query_row(
            "SELECT state, totals_json FROM batch_runs WHERE id = ?1",
            rusqlite::params![run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let per_category: Vec<(String, usize)> = conn
        .prepare(
            "SELECT label_id, count(*) FROM batch_items WHERE run_id = ?1 AND moved = 1 GROUP BY label_id",
        )
        .map_err(|e| e.to_string())?
        .query_map(rusqlite::params![run_id], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let remainder: i64 = conn
        .query_row(
            "SELECT count(*) FROM labels WHERE needs_review = 1 AND dismissed = 0",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let undone: i64 = conn
        .query_row(
            "SELECT count(*) FROM batch_items WHERE run_id = ?1 AND undone = 1",
            rusqlite::params![run_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let _ = totals;
    Ok(BatchReport { run_id, state: state_s, per_category, remainder: remainder as usize, undone: undone as usize })
}

/// Undo-batch: restore originals in reverse journal order.
#[tauri::command]
pub async fn undo_batch(state: State<'_, AppState>, run_id: i64) -> Result<usize, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    batch::undo_run(&manager, &store, run_id).await
}

// ── Integration-closeout commands (audit fixes) ──────────────────

/// Classify by (mailbox, uid) — the reader knows UIDs, not row ids.
/// Used for unlabeled/excluded mail (no suggestion row exists yet).
#[tauri::command]
pub async fn classify_message_uid(
    state: State<'_, AppState>,
    mailbox: String,
    uid: u32,
) -> Result<ClassificationReady, String> {
    let message_id = {
        let guard = state.store.lock().unwrap();
        let mb = mailbox_id_of(guard.conn(), &mailbox)?;
        crate::store::queries::find_message_id(guard.conn(), mb, uid)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "message not found in local cache".to_string())?
    };
    classify_message(state, message_id).await
}

/// Exclude (or re-include) a folder from auto-classify. Manual classify
/// still works on excluded folders.
#[tauri::command]
pub async fn set_folder_excluded(
    state: State<'_, AppState>,
    folder: String,
    excluded: bool,
) -> Result<Vec<String>, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    if excluded {
        crate::store::queries::exclude_folder(conn, &folder, "user opt-out")
            .map_err(|e| e.to_string())?;
    } else {
        crate::store::queries::include_folder(conn, &folder).map_err(|e| e.to_string())?;
    }
    crate::store::queries::excluded_folders(conn).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchRunRow {
    pub id: i64,
    pub state: String,
    pub started_at: String,
}

/// Past runs for the resume affordance (newest first).
#[tauri::command]
pub async fn batch_runs_list(state: State<'_, AppState>) -> Result<Vec<BatchRunRow>, String> {
    let guard = state.store.lock().unwrap();
    let rows: Vec<BatchRunRow> = guard
        .conn()
        .prepare("SELECT id, state, started_at FROM batch_runs ORDER BY id DESC LIMIT 20")
        .map_err(|e| e.to_string())?
        .query_map([], |r| {
            Ok(BatchRunRow { id: r.get(0)?, state: r.get(1)?, started_at: r.get(2)? })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}
