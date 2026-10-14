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
    // Phase 25: fire-and-forget rule evaluation behind sync (3 lines:
    // excluded folders skip inside; sync never awaits, failures never
    // fail the sync — same discipline as the classify hook above).
    rule_after_sync(state, mailbox, app);
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
/// (or a rule destination — see below).
///
/// Backend-enforced: refuses without a label row (nothing suggested),
/// refuses stale rows (taxonomy changed — reclassify first). Offline
/// confirms replay via `imap_outbox`. `allow_nest` answers the one-time
/// `Auto`-collision dialog (user-owned `Auto` without our marker).
/// `prefer` resolves rule-vs-classifier conflicts: `"classifier"` confirms
/// the classifier label (clearing the rule hit); anything else confirms
/// the rule hit when one exists, else the label. Dismissal (via
/// `dismiss_suggestion`) always keeps mail in place.
#[tauri::command]
pub async fn confirm_suggestion(
    state: State<'_, AppState>,
    message_id: u64,
    allow_nest: Option<bool>,
    prefer: Option<String>,
) -> Result<ConfirmResult, String> {
    let store = state.store.clone();
    // Phase 25: rule-sourced rows confirm through this SAME path unchanged
    // in contract — the hit carries its own destination (plain folder or
    // taxonomy category) instead of a classifier label row. The move below
    // reuses Phase 10 `move_one_uid` + `imap_outbox` (UID-only addressing,
    // UIDVALIDITY epoch gating); the suggestion row clears on resolution
    // and the log carries rule-id + message-id only (T-25-06).
    let rule_hit = if prefer.as_deref() == Some("classifier") {
        None
    } else {
        let guard = store.lock().unwrap();
        plan_rule_filing(guard.conn(), message_id).ok()
    };
    if let Some(plan) = rule_hit {
        let account_cfg = load_account_config(state.clone()).await?;
        let manager = manager_for(&state, &account_cfg);
        // Destination resolution: taxonomy categories AND plain-folder
        // destinations nested under `Auto/` reuse `ensure_auto_tree`
        // (confirm-then-nest collision semantics unchanged); other plain
        // folders reuse the Phase 11 CREATE guards.
        let (fplan, dest_display) = if plan.dest_kind == "category" {
            let tax = load_default().map_err(|e| e.to_string())?;
            let (dest_display, top_name, child_name) =
                filing::auto_path_for_id(&tax, &plan.destination)
                    .map_err(|e| e.to_string())?;
            (
                filing::FilePlan {
                    message_id,
                    mailbox: plan.mailbox.clone(),
                    uid: plan.uid,
                    dest_display: dest_display.clone(),
                    top_name,
                    child_name,
                    threshold_used: 0.0,
                    confidence: 1.0,
                    needs_review: false,
                },
                dest_display,
            )
        } else if let Some((top_name, child_name)) = split_auto_nest(&plan.destination) {
            (
                filing::FilePlan {
                    message_id,
                    mailbox: plan.mailbox.clone(),
                    uid: plan.uid,
                    dest_display: plan.destination.clone(),
                    top_name,
                    child_name,
                    threshold_used: 0.0,
                    confidence: 1.0,
                    needs_review: false,
                },
                plan.destination.clone(),
            )
        } else {
            let wire = ensure_rule_folder(&manager, &store, &plan.destination).await?;
            (
                filing::FilePlan {
                    message_id,
                    mailbox: plan.mailbox.clone(),
                    uid: plan.uid,
                    dest_display: plan.destination.clone(),
                    top_name: plan.destination.clone(),
                    child_name: None,
                    threshold_used: 0.0,
                    confidence: 1.0,
                    needs_review: false,
                },
                wire,
            )
        };
        // Auto-tree destinations (category + Auto/-nested) go through the
        // tree ensure (collision dialog via `allow_nest`); plain folders
        // already ensured above carry their wire in `dest_display`.
        let dest_wire = if plan.dest_kind == "category"
            || split_auto_nest(&plan.destination).is_some()
        {
            filing::ensure_auto_tree(&manager, &store, &fplan, allow_nest.unwrap_or(false))
                .await
                .map_err(|e| e.to_string())?
        } else {
            dest_display.clone()
        };
        let result = filing::execute_filing_move(&manager, &store, &fplan, &dest_wire)
            .await
            .map_err(|e| e.to_string())?;
        {
            let guard = store.lock().unwrap();
            let _ =
                crate::store::queries::delete_rule_suggestion(guard.conn(), message_id);
            // Pin the outcome: a confirmed rule move must not re-fire on
            // the next sync (the message still matches the rule in its new
            // folder). `latest_override` presence blocks re-evaluation.
            let _ = crate::store::queries::log_override(
                guard.conn(),
                message_id,
                &plan.rule_id,
                &plan.destination,
            );
        }
        // Phase 28: fire-and-forget override_events log (action="confirm")
        {
            let guard = store.lock().unwrap();
            let _ = crate::store::queries::log_override_event(
                guard.conn(),
                &message_id.to_string(),
                "confirm",
                Some(&plan.rule_id),
                Some(&plan.destination),
                Some(1.0),
                Some(0.0),
                None,
                None,
                None,
                None,
            );
        }
        eprintln!(
            "[SGE rules] confirmed rule={} message={message_id} -> {dest_wire}",
            plan.rule_id
        );
        return Ok(result);
    }
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
    let result = filing::execute_filing_move(&manager, &store, &plan, &dest_wire)
        .await
        .map_err(|e| e.to_string())?;
    // The user picked the classifier side of a rule-vs-classifier
    // conflict: the pending rule hit resolves away (no re-fire — the
    // message now carries a label row, which evaluation skips).
    {
        let guard = store.lock().unwrap();
        let _ = crate::store::queries::delete_rule_suggestion(guard.conn(), message_id);
    }
    // Phase 28: fire-and-forget override_events log (action="confirm")
    {
        let guard = store.lock().unwrap();
        // Extract from_category from the label row (primary_id)
        let from_category = {
            let conn = guard.conn();
            crate::store::queries::get_label(conn, message_id)
                .map_err(|e| e.to_string())?
                .map(|l| l.primary_id)
        };
        // Extract to_category from the plan (dest_display parsed to category ID)
        let to_category = if let Some((_, child)) = split_auto_nest(&dest_wire) {
            child
        } else if dest_wire.starts_with("Auto/") {
            dest_wire.strip_prefix("Auto/").map(|s| s.to_string())
        } else {
            None
        };
        // Confidence/threshold from label row
        let (confidence, threshold) = {
            let conn = guard.conn();
            crate::store::queries::get_label(conn, message_id)
                .map_err(|e| e.to_string())?
                .map(|l| (l.confidence, l.threshold))
                .unwrap_or((1.0, 0.0))
        };
        // Keywords from evidence (serialize as JSON array) - placeholder for now
        let _ = crate::store::queries::log_override_event(
            guard.conn(),
            &message_id.to_string(),
            "confirm",
            from_category.as_deref(),
            to_category.as_deref(),
            Some(confidence),
            Some(threshold),
            None,
            None,
            None,
            None,
        );
    }
    Ok(result)
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
        // Phase 28: fire-and-forget override_events log (action="override")
        let _ = crate::store::queries::log_override_event(
            conn,
            &message_id.to_string(),
            "override",
            Some(&from_id),
            Some(&to_category),
            Some(1.0),
            Some(0.0),
            None,
            None,
            None,
            None,
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
    crate::store::queries::dismiss_label(guard.conn(), message_id).map_err(|e| e.to_string())?;
    // Dismissal keeps mail in place: a pending rule hit for the same
    // message clears alongside the label dismissal.
    let _ = crate::store::queries::delete_rule_suggestion(guard.conn(), message_id);
    // Phase 28: fire-and-forget override_events log (action="dismiss")
    // Extract from_category from the label row before it's dismissed
    let from_category = {
        let conn = guard.conn();
        crate::store::queries::get_label(conn, message_id)
            .map_err(|e| e.to_string())?
            .map(|l| l.primary_id)
    };
    let _ = crate::store::queries::log_override_event(
        guard.conn(),
        &message_id.to_string(),
        "dismiss",
        from_category.as_deref(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    Ok(())
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
    /// Phase 25: rule-vs-classifier comparison. A rule hit NEVER silently
    /// overrides the classifier label — when both exist the confirm dialog
    /// shows both and the user picks; dismissal keeps mail in place.
    pub rule_alternative: Option<RuleAlternative>,
}

/// Rule side of the confirm comparison (transient, redacted).
#[derive(Debug, Clone, Serialize)]
pub struct RuleAlternative {
    pub rule_id: String,
    pub rule_name: String,
    pub dest_kind: String,
    pub destination: String,
    pub justification: String,
    /// Some when the destination overlaps the taxonomy-managed tree
    /// (RULE-01 verbatim overlap wording for the dialog).
    pub overlap_note: Option<String>,
}

/// Rule-vs-taxonomy overlap (RULE-01 verbatim): a destination inside the
/// `Auto/` tree or a taxonomy-managed folder resolves by user confirmation
/// reusing `confirm_suggestion`. Category-kind destinations always resolve
/// into the `Auto/` tree, so they always overlap.
pub fn rule_overlap_note(dest_kind: &str, destination: &str) -> Option<String> {
    let d = destination.trim();
    let under_auto = dest_kind == "category"
        || d.eq_ignore_ascii_case("auto")
        || d.len() > 5
            && (d.starts_with("Auto/") || d.starts_with("auto/") || d.starts_with("AUTO/"));
    if under_auto {
        Some(
            "esta pasta é gerenciada pelo classificador — confirme para mover".to_string(),
        )
    } else {
        None
    }
}

/// Everything the confirm surface needs for one message. None when the
/// message was never classified AND has no rule hit.
#[tauri::command]
pub async fn suggestion_detail(
    state: State<'_, AppState>,
    message_id: u64,
) -> Result<Option<SuggestionDetail>, String> {
    let tax = load_default().map_err(|e| e.to_string())?;
    let guard = state.store.lock().unwrap();
    suggestion_detail_inner(guard.conn(), message_id, &tax)
}

/// Pure detail builder (testable without Tauri state).
pub fn suggestion_detail_inner(
    conn: &rusqlite::Connection,
    message_id: u64,
    tax: &Taxonomy,
) -> Result<Option<SuggestionDetail>, String> {
    let label = match crate::store::queries::get_label(conn, message_id)
        .map_err(|e| e.to_string())?
    {
        Some(l) => Some(l),
        None => None,
    };
    let rule_hit = crate::store::queries::get_rule_suggestion(conn, message_id)
        .map_err(|e| e.to_string())?;
    if label.is_none() && rule_hit.is_none() {
        return Ok(None);
    }
    let dismissed = match &label {
        Some(_) => crate::store::queries::is_dismissed(conn, message_id)
            .map_err(|e| e.to_string())?,
        None => false,
    };
    let fields = crate::store::queries::message_classify_fields(conn, message_id)
        .map_err(|e| e.to_string())?;
    // Rule side of the comparison (transient redacted justification +
    // overlap note for the dialog). Present whenever a rule hit exists —
    // alone (rule-only suggestion) or beside a classifier label.
    let rule_alternative: Option<RuleAlternative> = match &rule_hit {
        Some(hit) => {
            let rule = crate::store::queries::get_rule(conn, &hit.rule_id)
                .map_err(|e| e.to_string())?;
            let (rule_name, justification) = match rule {
                Some(r) => {
                    let conditions: RuleConditions =
                        serde_json::from_str(&r.conditions_json).unwrap_or_default();
                    let destination = if r.dest_kind == "category" {
                        RuleDestination::TaxonomyCategory { id: r.destination.clone() }
                    } else {
                        RuleDestination::Folder { display: r.destination.clone() }
                    };
                    let typed = Rule {
                        id: r.id.clone(),
                        name: r.name.clone(),
                        priority: r.priority,
                        conditions,
                        destination,
                        scope: RuleScope { folders: vec![] },
                        enabled: r.enabled,
                    };
                    let just = crate::classify::redact::filter_output(
                        &rules::rule_justification(&typed),
                    );
                    (r.name, just)
                }
                None => (hit.rule_id.clone(), String::new()),
            };
            Some(RuleAlternative {
                rule_id: hit.rule_id.clone(),
                rule_name,
                dest_kind: hit.dest_kind.clone(),
                destination: hit.destination.clone(),
                justification,
                overlap_note: rule_overlap_note(&hit.dest_kind, &hit.destination),
            })
        }
        None => None,
    };
    // Classifier side. Rule-only hits (no label row) surface with the rule
    // destination as the primary so the confirm dialog has one shape.
    let (
        primary_id,
        primary_name,
        child_name,
        secondary_name,
        confidence,
        needs_review,
        stale,
        dest_display,
        justification,
    ) = match &label {
        Some(l) => {
            let top_name = tax
                .categories
                .iter()
                .find(|c| c.id == l.primary_id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| l.primary_id.clone());
            let (child_name, justification) = match &fields {
                Some((subject, snippet, from_addr, atts)) => {
                    let domain = from_addr.rsplit('@').next().unwrap_or("");
                    let is_reply = subject.to_lowercase().starts_with("re:");
                    let ev = crate::classify::evidence::build_evidence(
                        &subject, &snippet, domain, *atts, is_reply,
                    );
                    let kids: Vec<&crate::classify::taxonomy::Category> =
                        crate::classify::taxonomy::children_of(&tax, &l.primary_id);
                    let child = crate::classify::taxonomy::match_keywords(&ev, &kids)
                        .into_iter()
                        .next()
                        .and_then(|(id, _)| {
                            tax.categories.iter().find(|c| c.id == id).map(|c| c.name.clone())
                        });
                    let just = crate::classify::redact::filter_output(&justification(
                        &l.primary_id,
                        &ev,
                        &tax,
                    ));
                    (child, just)
                }
                None => (None, String::new()),
            };
            let secondary_name = l.secondary_id.as_ref().and_then(|sid| {
                tax.categories.iter().find(|c| &c.id == sid).map(|c| c.name.clone())
            });
            let dest_display = match &child_name {
                Some(c) => format!("Auto/{top_name}/{c}"),
                None => format!("Auto/{top_name}"),
            };
            (
                l.primary_id.clone(),
                top_name,
                child_name,
                secondary_name,
                l.confidence,
                l.needs_review,
                l.stale,
                dest_display,
                justification,
            )
        }
        None => {
            // Rule-only: primary mirrors the rule destination.
            let alt = rule_alternative.as_ref().cloned().unwrap_or(RuleAlternative {
                rule_id: String::new(),
                rule_name: String::new(),
                dest_kind: "folder".to_string(),
                destination: String::new(),
                justification: String::new(),
                overlap_note: None,
            });
            let dest_display = if alt.dest_kind == "category" {
                crate::classify::filing::auto_path_for_id(tax, &alt.destination)
                    .map(|(d, _, _)| d)
                    .unwrap_or_else(|_| alt.destination.clone())
            } else {
                alt.destination.clone()
            };
            (
                alt.destination.clone(),
                dest_display.clone(),
                None,
                None,
                1.0,
                false,
                false,
                dest_display,
                alt.justification.clone(),
            )
        }
    };
    let tops = tax
        .categories
        .iter()
        .filter(|c| c.parent.is_none())
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    Ok(Some(SuggestionDetail {
        message_id,
        primary_id,
        primary_name,
        child_name,
        secondary_name,
        confidence,
        needs_review,
        stale,
        dismissed,
        dest_display,
        justification,
        tops,
        rule_alternative,
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
/// stale-flag vanished ids → revalidate rules (never drop). Returns the new
/// version.
fn persist_taxonomy(conn: &rusqlite::Connection, tax: &Taxonomy) -> Result<u32, String> {
    let mut tax = tax.clone();
    tax.version += 1;
    let json = serde_json::to_string(&tax).map_err(|e| e.to_string())?;
    crate::store::queries::install_taxonomy(conn, tax.version, &tax.name, &json)
        .map_err(|e| e.to_string())?;
    let ids: Vec<&str> = tax.categories.iter().map(|c| c.id.as_str()).collect();
    crate::store::queries::mark_stale_unknown(conn, &ids).map_err(|e| e.to_string())?;
    // Rules survive bumps via ID-stable references: revalidate, never drop.
    for notice in revalidate_rules(conn, &tax)? {
        eprintln!("[SGE rules] taxonomy v{}: {notice}", tax.version);
    }
    Ok(tax.version)
}

/// Revalidate category-kind rules against a (possibly bumped) taxonomy.
/// Rules are NEVER silently dropped: ones whose destination id vanished
/// are DISABLED in place (row kept) and their names returned so the UI
/// can carry the plain-language notice.
pub fn revalidate_rules(
    conn: &rusqlite::Connection,
    tax: &Taxonomy,
) -> Result<Vec<String>, String> {
    let ids = crate::classify::taxonomy::category_ids(tax);
    let rules = crate::store::queries::list_rules(conn).map_err(|e| e.to_string())?;
    let mut disabled = Vec::new();
    for r in rules.iter().filter(|r| r.enabled && r.dest_kind == "category") {
        if !ids.iter().any(|id| id == &r.destination) {
            let mut row = r.clone();
            row.enabled = false;
            crate::store::queries::update_rule(conn, &row).map_err(|e| e.to_string())?;
            disabled.push(format!(
                "regra “{}” desativada — a categoria de destino não existe mais",
                r.name
            ));
        }
    }
    Ok(disabled)
}

/// Follow a category delete: rules pointing at the deleted id fall back to
/// the reassigned id; when the target cannot hold them they disable with a
/// notice (never silently dropped). Returns (remapped, disabled-notices).
pub fn reassign_deleted_rules(
    conn: &rusqlite::Connection,
    from: &str,
    to: &str,
) -> Result<(usize, Vec<String>), String> {
    let tax = read_taxonomy(conn)?;
    let ids = crate::classify::taxonomy::category_ids(&tax);
    // NOTE: read_taxonomy returns the CURRENT taxonomy — the delete caller
    // persists the removal first, so `from` is already gone and `to` must
    // be present for a remap.
    let target_ok = ids.iter().any(|id| id == to);
    let rules =
        crate::store::queries::rules_using_category(conn, from).map_err(|e| e.to_string())?;
    let mut remapped = 0usize;
    let mut disabled = Vec::new();
    for r in rules {
        if target_ok {
            if let Some(new_dest) =
                taxedit::remap_rule_destination(&r.dest_kind, &r.destination, from, to)
            {
                let mut row = r.clone();
                row.destination = new_dest;
                // Re-enable: the row may have been disabled by the bump
                // revalidation an instant earlier — its destination is
                // valid again by construction, so the rule stays alive.
                row.enabled = true;
                crate::store::queries::update_rule(conn, &row)
                    .map_err(|e| e.to_string())?;
                remapped += 1;
            }
        } else {
            let mut row = r.clone();
            row.enabled = false;
            crate::store::queries::update_rule(conn, &row).map_err(|e| e.to_string())?;
            disabled.push(format!(
                "regra “{}” desativada — a categoria de destino foi excluída",
                r.name
            ));
        }
    }
    Ok((remapped, disabled))
}

/// Outcome of a structural taxonomy change for the editor notice surface:
/// mail moved + rule destinations remapped + rules disabled (with reasons).
#[derive(Debug, Clone, Serialize)]
pub struct TaxonomyChangeReport {
    pub moved: usize,
    pub rules_remapped: usize,
    pub rules_disabled: Vec<String>,
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
) -> Result<TaxonomyChangeReport, String> {
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
    // 4. Taxonomy merge + label remap + rule destination remap + version++
    // (brief lock). Rule remap rides the SAME migration as the label remap
    // and runs BEFORE persist so the bump revalidation sees clean rows.
    let rules_remapped = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mut tax = read_taxonomy(conn)?;
        taxedit::apply_merge(&mut tax, &from, &into).map_err(edit_err)?;
        crate::store::queries::remap_label_ids(conn, &from, &into).map_err(|e| e.to_string())?;
        let remapped = crate::store::queries::remap_rule_destinations(conn, &from, &into)
            .map_err(|e| e.to_string())?;
        persist_taxonomy(conn, &tax)?;
        remapped
    };
    Ok(TaxonomyChangeReport { moved, rules_remapped, rules_disabled: vec![] })
}

/// Delete a category: mail moves to `reassign_to` first, then folder DELETE
/// guards, label remap, taxonomy update. Refuses when mail would orphan.
#[tauri::command]
pub async fn delete_category(
    state: State<'_, AppState>,
    id: String,
    reassign_to: String,
) -> Result<TaxonomyChangeReport, String> {
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
    // 4. Taxonomy delete + label remap + rule reassign-or-disable +
    // version++ (brief lock). Rule reassignment rides the same migration
    // and runs BEFORE persist so the bump revalidation sees clean rows;
    // un-reassignable rules disable with a plain-language notice (never
    // silently dropped).
    let (rules_remapped, rules_disabled) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mut tax = read_taxonomy(conn)?;
        taxedit::apply_delete(&mut tax, &id).map_err(edit_err)?;
        crate::store::queries::remap_label_ids(conn, &id, &reassign_to)
            .map_err(|e| e.to_string())?;
        let orphans = crate::store::queries::labels_using(conn, &id).map_err(|e| e.to_string())?;
        if orphans > 0 {
            return Err(format!("{orphans} rótulos órfãos — migração incompleta"));
        }
        // Reassign-against-old-taxonomy would see `to` present and remap —
        // but `apply_delete` above only mutated memory. Persist first so
        // the stored taxonomy matches, then reassign (target check reads
        // the NEW taxonomy where `to` must exist), then the bump
        // revalidation can only confirm.
        persist_taxonomy(conn, &tax)?;
        let (remapped, disabled) = reassign_deleted_rules(conn, &id, &reassign_to)?;
        (remapped, disabled)
    };
    Ok(TaxonomyChangeReport { moved, rules_remapped, rules_disabled })
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
                        // Phase 28: fire-and-forget override_events log (action="batch")
                        let _ = crate::store::queries::log_override_event(
                            guard.conn(),
                            &mid.to_string(),
                            "batch",
                            None,
                            Some(&ready.primary_id),
                            Some(ready.confidence),
                            Some(threshold),
                            None,
                            Some(&run_id.to_string()),
                            None,
                            None,
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
    Ok(BatchReport { run_id, state: state_s, per_category, remainder: remainder as usize, undone: undone as usize, totals_json: totals })
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

// ── Phase 24: auto-archive commands (ARCH-01/02) ───────────────────

use crate::classify::archive;

/// Archive rule settings (SQLite-persisted, survives restart — never
/// localStorage). Threshold is enforced backend-side (7–730).
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveSettings {
    pub threshold_days: i64,
    pub included_folders: Vec<String>,
}

/// Read the archive rule (defaults: 90d, INBOX + Auto tops).
#[tauri::command]
pub async fn get_archive_settings(state: State<'_, AppState>) -> Result<ArchiveSettings, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    Ok(ArchiveSettings {
        threshold_days: crate::store::queries::get_archive_threshold(conn),
        included_folders: archive::resolve_included_folders(conn),
    })
}

/// Persist the archive rule. Backend is the enforcer: out-of-range
/// thresholds and empty scopes fail here with plain language, never
/// silently. Hardcoded exclusions (A Classificar/Sent/Drafts/Archive)
/// stay structural in the selector regardless of this list.
#[tauri::command]
pub async fn set_archive_settings(
    state: State<'_, AppState>,
    threshold_days: i64,
    included_folders: Vec<String>,
) -> Result<ArchiveSettings, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    crate::store::queries::set_archive_threshold(conn, threshold_days)?;
    let json = serde_json::to_string(&included_folders).map_err(|e| e.to_string())?;
    crate::store::queries::set_archive_included_json(conn, &json)?;
    Ok(ArchiveSettings {
        threshold_days: crate::store::queries::get_archive_threshold(conn),
        included_folders: archive::resolve_included_folders(conn),
    })
}

/// Start (or resume) an age-based archive run. Manual trigger ONLY — no
/// scheduler, no mount hook, no background poll invokes this command.
///
/// Shape mirrors `batch_classify` line-for-line with an archive selector:
/// reset cancel flag, pre-create the archive subtree, capture folder
/// epochs, spawn the chunk loop over BATCH_CHUNK=25 with per-chunk
/// UIDVALIDITY re-check and cooperative cancel, journal_intent BEFORE
/// each MOVE and journal_moved after, resume via journaled_ids skip, and
/// finish with `totals_json` carrying `{"kind":"archive",...}` so
/// report/list surfaces label archive runs distinctly. Moves go ONLY
/// through `archive_one` (proven `move_one_uid` + `imap_outbox` replay,
/// UID-only addressing, single session). No classifier call anywhere on
/// this path; the shared `undo_batch` reverses archive runs in reverse
/// to→from order. Returns the run id.
#[tauri::command]
pub async fn archive_run(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    resume_run: Option<i64>,
) -> Result<i64, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    // No sidecar lease: age is the only gate (confidence threshold
    // irrelevant on this path by design).
    let cancel = state.batch_cancel.clone();
    cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    let now = chrono::Utc::now();
    // Settings (backend truth).
    let (threshold, included) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        (
            crate::store::queries::get_archive_threshold(conn),
            archive::resolve_included_folders(conn),
        )
    };
    // Candidates + destination per candidate (short locks, no awaits).
    let mut jobs: Vec<(u64, String, u32, String)> = {
        let guard = store.lock().unwrap();
        let cands = archive::collect_archive_candidates(guard.conn(), threshold, &included, now)?;
        let mut jobs = Vec::with_capacity(cands.len());
        for (mid, mb, uid) in cands {
            if let Ok((dest_display, _, _)) = archive::archive_dest_for(guard.conn(), mid, &mb, now) {
                jobs.push((mid, mb, uid, dest_display));
            }
        }
        jobs
    };
    // Run id: fresh or resumed (resume skips journaled rows).
    let run_id = {
        let guard = store.lock().unwrap();
        match resume_run {
            Some(id) => id,
            None => crate::store::queries::create_batch_run(guard.conn()).map_err(|e| e.to_string())?,
        }
    };
    if resume_run.is_some() {
        let guard = store.lock().unwrap();
        let done = batch::journaled_ids(guard.conn(), run_id)?;
        jobs.retain(|(mid, _, _, _)| !done.contains(mid));
    }
    // Pre-create the archive subtree up front (dialog-off: no mid-run asks).
    {
        let mut dests: Vec<String> = jobs.iter().map(|(_, _, _, d)| d.clone()).collect();
        dests.sort();
        dests.dedup();
        archive::ensure_archive_tree(&manager, &store, &dests).await?;
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
    let total = jobs.len();
    let app_h = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut done = 0usize;
        let mut moved = 0usize;
        let mut chunk_no = 0usize;
        for chunk in jobs.chunks(batch::BATCH_CHUNK) {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            // Per-chunk UIDVALIDITY re-check: skip folders that shifted
            // (abort-to-resync for that folder, same as batch_classify).
            let mut live_chunk: Vec<(u64, String, u32, String)> = Vec::new();
            for (mid, mb, uid, dest) in chunk {
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
                    continue;
                }
                live_chunk.push((*mid, mb.clone(), *uid, dest.clone()));
            }
            for (mid, _mb, _uid, dest_display) in live_chunk {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // Resolve wire + current location (short lock, no await).
                let (from_folder, dest_wire) = {
                    let guard = store.lock().unwrap();
                    let conn = guard.conn();
                    let loc = crate::store::queries::message_location(conn, mid).ok().flatten();
                    let cached = crate::store::queries::list_mailboxes(conn).ok().unwrap_or_default();
                    match loc {
                        Some((mb, _)) => {
                            let delim = cached.first().map(|r| r.delimiter.clone()).unwrap_or_default();
                            (mb, archive::display_to_wire(&dest_display, &delim))
                        }
                        None => {
                            done += 1;
                            continue;
                        }
                    }
                };
                {
                    let guard = store.lock().unwrap();
                    let _ = batch::journal_intent(
                        guard.conn(), run_id, mid, &from_folder, &dest_wire,
                        archive::ARCHIVE_LABEL_ID, chunk_no,
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
                    } else if archive::archive_one(
                        &manager, &store, &mb_now, uid_now, &dest_wire, &dest_display,
                    )
                    .await
                    .is_ok()
                    {
                        let guard = store.lock().unwrap();
                        let _ = batch::journal_moved(guard.conn(), run_id, mid);
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
                    review: 0,
                    state: "running".to_string(),
                },
            );
        }
        let interrupted = cancel.load(std::sync::atomic::Ordering::SeqCst);
        let totals = serde_json::json!({
            "kind": "archive",
            "threshold_days": threshold,
            "done": done,
            "moved": moved,
        })
        .to_string();
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
                review: 0,
                state: if interrupted { "interrupted".to_string() } else { "done".to_string() },
            },
        );
    });
    Ok(run_id)
}

// ── Phase 25: smart folder rules — validated CRUD (RULE-01) ─────────
//
// Every write path validates backend-side (`rules::validate_rule`) with
// plain-language errors; UI restraint is never the only guard. Rule hits
// become `rule_suggestions` rows confirmed through the UNMODIFIED
// `confirm_suggestion` path — rules never move mail directly.

use crate::classify::rules::{self, RuleConditions, RuleDestination, RuleScope};
use crate::store::queries::RuleRow;

/// Rule surface for the options UI (conditions/destination as data).
#[derive(Debug, Clone, Serialize)]
pub struct RuleView {
    pub id: String,
    pub name: String,
    pub priority: i64,
    pub conditions: RuleConditions,
    pub destination: RuleDestination,
    pub scope_folders: Vec<String>,
    pub enabled: bool,
}

fn rule_view(row: &RuleRow) -> Result<RuleView, String> {
    let conditions: RuleConditions =
        serde_json::from_str(&row.conditions_json).map_err(|e| e.to_string())?;
    let destination = if row.dest_kind == "category" {
        RuleDestination::TaxonomyCategory { id: row.destination.clone() }
    } else {
        RuleDestination::Folder { display: row.destination.clone() }
    };
    let scope_folders: Vec<String> =
        serde_json::from_str(&row.scope_json).unwrap_or_default();
    Ok(RuleView {
        id: row.id.clone(),
        name: row.name.clone(),
        priority: row.priority,
        conditions,
        destination,
        scope_folders,
        enabled: row.enabled,
    })
}

/// Next free priority (append at the bottom — explicit reorder lowers it).
fn next_rule_priority(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT IFNULL(MAX(priority), 99) + 1 FROM folder_rules", [], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(rules::RULE_DEFAULT_PRIORITY)
}

/// Dedupe a slug id (`boletos`, `boletos-2`, …).
fn unique_rule_id(conn: &rusqlite::Connection, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2;
    loop {
        let taken: i64 = conn
            .query_row(
                "SELECT count(*) FROM folder_rules WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        if taken == 0 {
            return id;
        }
        id = format!("{base}-{n}");
        n += 1;
    }
}

/// Create a rule: validate → assign id/priority → persist. Any validation
/// failure rejects with a plain-language reason; nothing is stored.
#[tauri::command]
pub async fn create_rule(
    state: State<'_, AppState>,
    name: String,
    conditions: RuleConditions,
    destination: RuleDestination,
    scope_folders: Vec<String>,
) -> Result<RuleView, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let scope = RuleScope { folders: scope_folders };
    rules::validate_rule(conn, &name, &conditions, &destination, &scope, None)
        .map_err(|e| e.to_string())?;
    let id = unique_rule_id(conn, &rules::slug_for(&name));
    let row = RuleRow {
        id,
        name: name.trim().to_string(),
        priority: next_rule_priority(conn),
        conditions_json: serde_json::to_string(&conditions).map_err(|e| e.to_string())?,
        dest_kind: match &destination {
            RuleDestination::Folder { .. } => "folder".to_string(),
            RuleDestination::TaxonomyCategory { .. } => "category".to_string(),
        },
        destination: match &destination {
            RuleDestination::Folder { display } => display.trim().to_string(),
            RuleDestination::TaxonomyCategory { id } => id.clone(),
        },
        scope_json: serde_json::to_string(&scope.folders).map_err(|e| e.to_string())?,
        enabled: true,
    };
    crate::store::queries::insert_rule(conn, &row).map_err(|e| e.to_string())?;
    rule_view(&row)
}

/// List all rules in evaluation order (priority ASC, oldest wins ties).
#[tauri::command]
pub async fn list_rules(state: State<'_, AppState>) -> Result<Vec<RuleView>, String> {
    let guard = state.store.lock().unwrap();
    let rows = crate::store::queries::list_rules(guard.conn()).map_err(|e| e.to_string())?;
    rows.iter().map(rule_view).collect()
}

/// Edit a rule: re-validates the NEW values; an invalid edit is rejected
/// and the old rule is kept untouched (validate-before-write).
#[tauri::command]
pub async fn update_rule(
    state: State<'_, AppState>,
    id: String,
    name: String,
    conditions: RuleConditions,
    destination: RuleDestination,
    scope_folders: Vec<String>,
    enabled: bool,
) -> Result<RuleView, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let existing = crate::store::queries::get_rule(conn, &id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "regra desconhecida".to_string())?;
    let scope = RuleScope { folders: scope_folders };
    rules::validate_rule(conn, &name, &conditions, &destination, &scope, Some(&id))
        .map_err(|e| e.to_string())?;
    let row = RuleRow {
        id: id.clone(),
        name: name.trim().to_string(),
        priority: existing.priority,
        conditions_json: serde_json::to_string(&conditions).map_err(|e| e.to_string())?,
        dest_kind: match &destination {
            RuleDestination::Folder { .. } => "folder".to_string(),
            RuleDestination::TaxonomyCategory { .. } => "category".to_string(),
        },
        destination: match &destination {
            RuleDestination::Folder { display } => display.trim().to_string(),
            RuleDestination::TaxonomyCategory { id } => id.clone(),
        },
        scope_json: serde_json::to_string(&scope.folders).map_err(|e| e.to_string())?,
        enabled,
    };
    crate::store::queries::update_rule(conn, &row).map_err(|e| e.to_string())?;
    rule_view(&row)
}

/// Delete a rule: immediate, no confirm (rules hold no mail — mirrors
/// Phase 23 filter-delete semantics). Pending suggestion rows cascade.
#[tauri::command]
pub async fn delete_rule(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let guard = state.store.lock().unwrap();
    crate::store::queries::delete_rule(guard.conn(), &id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Enable/disable toggle (no confirm).
#[tauri::command]
pub async fn set_rule_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let guard = state.store.lock().unwrap();
    crate::store::queries::set_rule_enabled(guard.conn(), &id, enabled)
        .map_err(|e| e.to_string())
}

/// Explicit integer priority (lower wins; ties by creation order).
/// The UI wires this to move-up/move-down controls.
#[tauri::command]
pub async fn set_rule_priority(
    state: State<'_, AppState>,
    id: String,
    priority: i64,
) -> Result<(), String> {
    let guard = state.store.lock().unwrap();
    crate::store::queries::set_rule_priority(guard.conn(), &id, priority)
        .map_err(|e| e.to_string())
}

// ── Phase 25: at-sync hook + apply_rules through confirm_suggestion ──
// (RULE-02 engine, RULE-03 MOVE-only)
//
// Application point (CONTEXT D-02): post-sync hook enqueues rule
// evaluation fire-and-forget under the EXISTING ClassifyGate
// single-flight (no new gate type) — sync NEVER awaits rules, rules NEVER
// block sync completion or UI paint. Order per message: override-pin
// first (Phase 18 pins always win) → user rules by priority (first match
// wins, evaluation stops) → classifier suggestion (untouched).
// A rule hit writes a rule-sourced suggestion row flowing through the
// SAME confirm_suggestion path unchanged; rules never move mail directly
// in the single-message flow. Zero new IMAP verbs: CREATE-when-missing
// (Phase 11 guards) + MOVE (Phase 10 machinery, UID-only + UIDVALIDITY
// epoch gating). BODY.PEEK-only preserved (matcher input is cached text).

use crate::classify::rules::Rule;
use crate::store::queries::RuleSuggestionRow;

/// One rule hit: message + winning rule + resolved destination.
#[derive(Debug, Clone)]
pub struct RuleHit {
    pub message_id: u64,
    pub rule_id: String,
    pub dest_kind: String,
    pub destination: String,
}

/// Evaluate enabled rules over cached candidates
/// `(message_id, mailbox, sender, subject, snippet)`. Pure store +
/// matcher — no IMAP, never awaits. Skips: override-pinned, already
/// labeled (classifier territory — 25-02 adds the comparison dialog),
/// already suggested, disabled rules, out-of-scope rules. First match
/// wins per message (priority ASC, oldest wins ties — `list_rules`
/// ordering); evaluation stops at the hit.
pub fn evaluate_rules(
    conn: &rusqlite::Connection,
    candidates: &[(u64, String, String, String, String)],
) -> Vec<RuleHit> {
    let rows = match crate::store::queries::list_rules(conn) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut typed: Vec<Rule> = Vec::new();
    for row in rows.iter().filter(|r| r.enabled) {
        let conditions: RuleConditions = match serde_json::from_str(&row.conditions_json) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let scope_folders: Vec<String> =
            serde_json::from_str(&row.scope_json).unwrap_or_default();
        let destination = if row.dest_kind == "category" {
            RuleDestination::TaxonomyCategory { id: row.destination.clone() }
        } else {
            RuleDestination::Folder { display: row.destination.clone() }
        };
        typed.push(Rule {
            id: row.id.clone(),
            name: row.name.clone(),
            priority: row.priority,
            conditions,
            destination,
            scope: RuleScope { folders: scope_folders },
            enabled: true,
        });
    }
    let mut hits = Vec::new();
    for (mid, mailbox, sender, subject, snippet) in candidates {
        if crate::store::queries::latest_override(conn, *mid).ok().flatten().is_some() {
            continue;
        }
        if crate::store::queries::get_label(conn, *mid).ok().flatten().is_some() {
            continue;
        }
        if crate::store::queries::get_rule_suggestion(conn, *mid).ok().flatten().is_some() {
            continue;
        }
        for rule in &typed {
            if !rule.scope.folders.is_empty()
                && !rule.scope.folders.iter().any(|f| f == mailbox)
            {
                continue;
            }
            if rules::evaluate_rule(rule, sender, subject, snippet) {
                let (dest_kind, destination) = match &rule.destination {
                    RuleDestination::Folder { display } => {
                        ("folder".to_string(), display.clone())
                    }
                    RuleDestination::TaxonomyCategory { id } => {
                        ("category".to_string(), id.clone())
                    }
                };
                hits.push(RuleHit {
                    message_id: *mid,
                    rule_id: rule.id.clone(),
                    dest_kind,
                    destination,
                });
                break;
            }
        }
    }
    hits
}

/// Post-sync rule hook: evaluate behind sync, persist hits as confirmable
/// suggestion rows. Fire-and-forget (spawned, gate single-flights);
/// returns immediately and never fails the sync. Folders excluded from
/// auto-classify (Sent/Drafts + opt-outs) are skipped automatically —
/// manual `apply_rules` still serves them on demand.
pub fn rule_after_sync(state: &AppState, mailbox: &str, app: &tauri::AppHandle) {
    let excluded = {
        let guard = state.store.lock().unwrap();
        queries::is_excluded(guard.conn(), mailbox).unwrap_or(false)
    };
    if excluded {
        return;
    }
    let store = state.store.clone();
    let gate: Arc<ClassifyGate> = state.classify_gate.clone();
    let app_h = app.clone();
    let mb = mailbox.to_string();
    tauri::async_runtime::spawn(async move {
        if !gate.try_acquire() {
            return;
        }
        let hits = {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let cands = queries::rule_candidate_rows(conn, Some(&mb)).unwrap_or_default();
            let hits = evaluate_rules(conn, &cands);
            for h in &hits {
                let _ = queries::upsert_rule_suggestion(
                    conn,
                    &RuleSuggestionRow {
                        message_id: h.message_id,
                        rule_id: h.rule_id.clone(),
                        dest_kind: h.dest_kind.clone(),
                        destination: h.destination.clone(),
                    },
                );
            }
            hits.len()
        };
        gate.release();
        if hits > 0 {
            let _ = app_h.emit("rule-suggestions", hits);
        }
    });
}

/// Split an `Auto/`-nested plain-folder destination into
/// `(top, child?)` for the `ensure_auto_tree` path. Returns None for
/// non-`Auto/` destinations and for depths the tree path cannot express
/// (3+ levels fall back to the Phase 11 guarded ensure).
fn split_auto_nest(display: &str) -> Option<(String, Option<String>)> {
    let rest = display
        .strip_prefix("Auto/")
        .or_else(|| display.strip_prefix("auto/"))
        .or_else(|| display.strip_prefix("AUTO/"))?;
    let mut parts = rest.split('/').map(str::trim).filter(|s| !s.is_empty());
    let top = parts.next()?.to_string();
    let child = parts.next().map(str::to_string);
    if parts.next().is_some() {
        return None;
    }
    Some((top, child))
}

/// Pure rule filing plan: suggestion row + cached location (UID-only
/// addressing — no sequence numbers anywhere on this path).
pub struct RuleFilePlan {
    pub message_id: u64,
    pub mailbox: String,
    pub uid: u32,
    pub rule_id: String,
    pub dest_kind: String,
    pub destination: String,
}

pub fn plan_rule_filing(
    conn: &rusqlite::Connection,
    message_id: u64,
) -> Result<RuleFilePlan, String> {
    let hit = crate::store::queries::get_rule_suggestion(conn, message_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "sem sugestão de regra para este e-mail".to_string())?;
    let (mailbox, uid) = crate::store::queries::message_location(conn, message_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "message not found in local cache".to_string())?;
    Ok(RuleFilePlan {
        message_id,
        mailbox,
        uid,
        rule_id: hit.rule_id,
        dest_kind: hit.dest_kind,
        destination: hit.destination,
    })
}

/// Ensure a plain-folder rule destination exists via Phase 11 CREATE
/// guards, returning its wire name. Existing folders pass through
/// untouched (display or wire match). Levels are created top-down with
/// `prepare_create_wire` leaf validation — folder-injection refused
/// backend-side before any verb fires.
async fn ensure_rule_folder(
    manager: &std::sync::Arc<crate::imap::manager::SessionManager>,
    store: &std::sync::Arc<std::sync::Mutex<crate::store::Store>>,
    display: &str,
) -> Result<String, String> {
    use crate::commands::sync::{prepare_create_wire, refresh_mailbox_tree};
    let delimiter = {
        let guard = store.lock().unwrap();
        let cached = queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?;
        if let Some(row) =
            cached.iter().find(|r| r.display_name == display || r.name == display)
        {
            return Ok(row.name.clone());
        }
        cached.first().map(|r| r.delimiter.clone()).unwrap_or_default()
    };
    let mut parent_wire: Option<String> = None;
    let mut display_prefix = String::new();
    for leaf in display.split('/') {
        if !display_prefix.is_empty() {
            display_prefix.push('/');
        }
        display_prefix.push_str(leaf);
        let cached_now = {
            let guard = store.lock().unwrap();
            queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
        };
        if let Some(row) = cached_now.iter().find(|r| r.display_name == display_prefix) {
            parent_wire = Some(row.name.clone());
            continue;
        }
        let wire = prepare_create_wire(&cached_now, parent_wire.as_deref(), leaf, &delimiter)?;
        manager
            .create_mailbox_in(&wire)
            .await
            .map_err(|e| format!("não foi possível criar {display_prefix}: {e}"))?;
        parent_wire = Some(wire);
    }
    refresh_mailbox_tree(manager, store).await?;
    parent_wire.ok_or_else(|| "destino vazio".to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyRulesResult {
    pub evaluated: usize,
    pub hits: usize,
}

/// On-demand rule application over already-synced mail (whole account
/// when `mailbox` is None, else one folder). Manual intent: excluded
/// folders ARE served here. Hits surface as confirmable suggestion rows
/// (dialog ON in the UI through `confirm_suggestion`); whole-account bulk
/// journaling/progress/undo is the 25-02 depth decision — this thin loop
/// is what it builds on.
#[tauri::command]
pub async fn apply_rules(
    state: State<'_, AppState>,
    mailbox: Option<String>,
) -> Result<ApplyRulesResult, String> {
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let cands =
        queries::rule_candidate_rows(conn, mailbox.as_deref()).map_err(|e| e.to_string())?;
    let evaluated = cands.len();
    let hits = evaluate_rules(conn, &cands);
    for h in &hits {
        queries::upsert_rule_suggestion(
            conn,
            &RuleSuggestionRow {
                message_id: h.message_id,
                rule_id: h.rule_id.clone(),
                dest_kind: h.dest_kind.clone(),
                destination: h.destination.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(ApplyRulesResult { evaluated, hits: hits.len() })
}

/// Pending rule hits for the confirm surface: ids + destination + a
/// TRANSIENT redacted justification (rebuilt here, never persisted —
/// RULE-03 verbatim; logs carry rule-id + message-id only).
#[derive(Debug, Clone, Serialize)]
pub struct RuleSuggestionView {
    pub message_id: u64,
    pub rule_id: String,
    pub rule_name: String,
    pub dest_kind: String,
    pub destination: String,
    pub justification: String,
    /// Some when the destination overlaps the taxonomy-managed tree.
    pub overlap_note: Option<String>,
    /// Some when the classifier ALSO labeled this message (disagreement —
    /// the dialog shows both and the user picks).
    pub classifier_alternative: Option<ClassifierAlternative>,
}

/// Classifier side of a rule-vs-classifier disagreement.
#[derive(Debug, Clone, Serialize)]
pub struct ClassifierAlternative {
    pub category_id: String,
    pub category_name: String,
    pub confidence: f64,
}

#[tauri::command]
pub async fn pending_rule_suggestions(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<RuleSuggestionView>, String> {
    let tax = load_default().map_err(|e| e.to_string())?;
    let guard = state.store.lock().unwrap();
    let conn = guard.conn();
    let rows =
        queries::list_rule_suggestions(conn, limit.unwrap_or(100)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for s in rows {
        let rule = queries::get_rule(conn, &s.rule_id).map_err(|e| e.to_string())?;
        let (name, just) = match rule {
            Some(r) => {
                let conditions: RuleConditions =
                    serde_json::from_str(&r.conditions_json).unwrap_or_default();
                let destination = if r.dest_kind == "category" {
                    RuleDestination::TaxonomyCategory { id: r.destination.clone() }
                } else {
                    RuleDestination::Folder { display: r.destination.clone() }
                };
                let typed = Rule {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    priority: r.priority,
                    conditions,
                    destination,
                    scope: RuleScope { folders: vec![] },
                    enabled: r.enabled,
                };
                let just = crate::classify::redact::filter_output(
                    &rules::rule_justification(&typed),
                );
                (r.name, just)
            }
            None => (s.rule_id.clone(), String::new()),
        };
        let classifier_alternative = queries::get_label(conn, s.message_id)
            .map_err(|e| e.to_string())?
            .map(|l| {
                let category_name = tax
                    .categories
                    .iter()
                    .find(|c| c.id == l.primary_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| l.primary_id.clone());
                ClassifierAlternative {
                    category_id: l.primary_id,
                    category_name,
                    confidence: l.confidence,
                }
            });
        out.push(RuleSuggestionView {
            message_id: s.message_id,
            rule_id: s.rule_id,
            rule_name: name,
            dest_kind: s.dest_kind.clone(),
            destination: s.destination.clone(),
            justification: just,
            overlap_note: rule_overlap_note(&s.dest_kind, &s.destination),
            classifier_alternative,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod rule_apply_tests {
    use super::*;
    use crate::store::Store;

    fn seed_db() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter) \
             VALUES ('INBOX', 77, 10, '/'), ('Arquivo', 77, 10, '/')",
            [],
        )
        .unwrap();
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| r.get(0))
            .unwrap();
        // Three cached messages (headers + preview only — the matcher never
        // sees more than this).
        for (uid, subject, from, preview) in [
            (1u32, "Fatura de energia", "contas@utfpr.edu.br", "segue o boleto de outubro"),
            (2, "Reunião quinta", "chefe@empresa.com", "pauta da reunião"),
            (3, "Fatura de água", "contas@utfpr.edu.br", "segunda via do boleto"),
        ] {
            queries::upsert_message(
                conn, mb as u64, uid, None, subject, from, "[]", "[]",
                "2024-01-01T00:00:00Z", "[]", false, preview,
            )
            .unwrap();
        }
        store
    }

    fn seed_rule(
        conn: &rusqlite::Connection,
        id: &str,
        name: &str,
        priority: i64,
        conditions_json: &str,
        enabled: bool,
    ) {
        queries::insert_rule(
            conn,
            &RuleRow {
                id: id.to_string(),
                name: name.to_string(),
                priority,
                conditions_json: conditions_json.to_string(),
                dest_kind: "folder".to_string(),
                destination: "Arquivo".to_string(),
                scope_json: "[]".to_string(),
                enabled,
            },
        )
        .unwrap();
    }

    fn sender_rule_json(domain: &str) -> String {
        format!(r#"{{"sender_domain":{domain:?}}}"#)
    }

    fn cands(conn: &rusqlite::Connection) -> Vec<(u64, String, String, String, String)> {
        queries::rule_candidate_rows(conn, Some("INBOX")).unwrap()
    }

    #[test]
    fn rule_apply_first_match_wins_by_priority() {
        let store = seed_db();
        let conn = store.conn();
        seed_rule(conn, "r-lo", "Baixa", 20, &sender_rule_json("utfpr.edu.br"), true);
        seed_rule(conn, "r-hi", "Alta", 10, &sender_rule_json("utfpr.edu.br"), true);
        let hits = evaluate_rules(conn, &cands(conn));
        // Uids 1 and 3 match; EACH carries only the priority-10 winner.
        assert_eq!(hits.len(), 2);
        for h in &hits {
            assert_eq!(h.rule_id, "r-hi", "lower-priority rule must never fire on the same message");
        }
        // Explicit reorder flips the winner (priority persists).
        queries::set_rule_priority(conn, "r-lo", 5).unwrap();
        let hits2 = evaluate_rules(conn, &cands(conn));
        assert!(hits2.iter().all(|h| h.rule_id == "r-lo"));
    }

    #[test]
    fn rule_apply_override_pin_beats_rules() {
        let store = seed_db();
        let conn = store.conn();
        seed_rule(conn, "r-1", "R", 10, &sender_rule_json("utfpr.edu.br"), true);
        let mid: u64 = conn
            .query_row("SELECT id FROM messages WHERE uid = 1", [], |r| r.get(0))
            .unwrap();
        queries::log_override(conn, mid, "financas", "trabalho").unwrap();
        let hits = evaluate_rules(conn, &cands(conn));
        // Uid 1 pinned away; uid 3 still hits.
        assert_eq!(hits.len(), 1);
        assert_ne!(
            hits[0].message_id, mid,
            "pinned override must beat the rule"
        );
    }

    #[test]
    fn rule_apply_labeled_mail_left_to_classifier() {
        let store = seed_db();
        let conn = store.conn();
        seed_rule(conn, "r-1", "R", 10, &sender_rule_json("utfpr.edu.br"), true);
        let mid: u64 = conn
            .query_row("SELECT id FROM messages WHERE uid = 1", [], |r| r.get(0))
            .unwrap();
        queries::upsert_label(conn, mid, "financas", None, 0.9, 0.6, false).unwrap();
        let hits = evaluate_rules(conn, &cands(conn));
        assert!(hits.iter().all(|h| h.message_id != mid));
    }

    #[test]
    fn rule_apply_disabled_and_out_of_scope_never_fire() {
        let store = seed_db();
        let conn = store.conn();
        seed_rule(conn, "r-off", "Off", 1, &sender_rule_json("utfpr.edu.br"), false);
        queries::insert_rule(
            conn,
            &RuleRow {
                id: "r-scope".to_string(),
                name: "Scoped".to_string(),
                priority: 2,
                conditions_json: sender_rule_json("utfpr.edu.br"),
                dest_kind: "folder".to_string(),
                destination: "Arquivo".to_string(),
                scope_json: r#"["Sent"]"#.to_string(),
                enabled: true,
            },
        )
        .unwrap();
        let hits = evaluate_rules(conn, &cands(conn));
        assert!(hits.is_empty(), "disabled/out-of-scope rules must never fire: {hits:?}");
    }

    #[test]
    fn rule_apply_evaluation_emits_zero_imap_traffic() {
        // Mirrors the Phase 18 secondary-labels assertion style: rule
        // evaluation (match + suggestion persist) must leave BOTH durable
        // outboxes empty — suggestions confirm later through the gated path.
        let store = seed_db();
        let conn = store.conn();
        seed_rule(conn, "r-1", "R", 10, &sender_rule_json("utfpr.edu.br"), true);
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| r.get(0))
            .unwrap();
        let hits = evaluate_rules(conn, &cands(conn));
        assert!(!hits.is_empty());
        for h in &hits {
            queries::upsert_rule_suggestion(
                conn,
                &RuleSuggestionRow {
                    message_id: h.message_id,
                    rule_id: h.rule_id.clone(),
                    dest_kind: h.dest_kind.clone(),
                    destination: h.destination.clone(),
                },
            )
            .unwrap();
        }
        // Re-evaluation is idempotent (PK): no duplicate rows, still hits==2.
        let hits2 = evaluate_rules(conn, &cands(conn));
        assert!(hits2.is_empty(), "already-suggested mail must not re-hit");
        assert_eq!(queries::imap_outbox_count(conn, mb as u64).unwrap(), 0);
        assert_eq!(queries::outbox_count(conn, mb as u64).unwrap(), 0);
    }

    #[test]
    fn rule_apply_plan_carries_uid_only_addressing() {
        let store = seed_db();
        let conn = store.conn();
        seed_rule(conn, "r-1", "R", 10, &sender_rule_json("utfpr.edu.br"), true);
        let hits = evaluate_rules(conn, &cands(conn));
        assert!(!hits.is_empty());
        let h = &hits[0];
        queries::upsert_rule_suggestion(
            conn,
            &RuleSuggestionRow {
                message_id: h.message_id,
                rule_id: h.rule_id.clone(),
                dest_kind: h.dest_kind.clone(),
                destination: h.destination.clone(),
            },
        )
        .unwrap();
        let plan = plan_rule_filing(conn, h.message_id).unwrap();
        assert_eq!(plan.mailbox, "INBOX");
        assert!((1..=3).contains(&plan.uid), "uid-only addressing, got {}", plan.uid);
        assert_eq!(plan.destination, "Arquivo");
    }
}

#[cfg(test)]
mod rule_conflict_tests {
    use super::*;
    use crate::store::Store;

    fn seed_db() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter) \
             VALUES ('INBOX', 77, 10, '/'), ('Arquivo', 77, 10, '/')",
            [],
        )
        .unwrap();
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| r.get(0))
            .unwrap();
        for (uid, subject, from, preview) in [
            (1u32, "Fatura de energia", "contas@utfpr.edu.br", "segue o boleto de outubro"),
            (2, "Reunião quinta", "chefe@empresa.com", "pauta da reunião"),
        ] {
            queries::upsert_message(
                conn, mb as u64, uid, None, subject, from, "[]", "[]",
                "2024-01-01T00:00:00Z", "[]", false, preview,
            )
            .unwrap();
        }
        store
    }

    fn seed_rule(
        conn: &rusqlite::Connection,
        id: &str,
        name: &str,
        priority: i64,
        conditions_json: &str,
        dest_kind: &str,
        destination: &str,
    ) {
        queries::insert_rule(
            conn,
            &RuleRow {
                id: id.to_string(),
                name: name.to_string(),
                priority,
                conditions_json: conditions_json.to_string(),
                dest_kind: dest_kind.to_string(),
                destination: destination.to_string(),
                scope_json: "[]".to_string(),
                enabled: true,
            },
        )
        .unwrap();
    }

    fn mid_for_uid(conn: &rusqlite::Connection, uid: u32) -> u64 {
        conn.query_row("SELECT id FROM messages WHERE uid = ?1", rusqlite::params![uid], |r| {
            r.get(0)
        })
        .unwrap()
    }

    #[test]
    fn rule_conflict_priority_cutoff_with_tie_by_creation() {
        let store = seed_db();
        let conn = store.conn();
        let cond = r#"{"sender_domain":"utfpr.edu.br"}"#;
        // Three overlapping rules; the priority-5 pair ties — oldest wins.
        seed_rule(conn, "r-old", "Antiga", 5, cond, "folder", "Arquivo");
        seed_rule(conn, "r-new", "Nova", 5, cond, "folder", "Arquivo");
        seed_rule(conn, "r-low", "Baixa", 50, cond, "folder", "Arquivo");
        // Force distinct creation order (second-resolution timestamps tie
        // inside fast tests — the ORDER BY falls back deterministically).
        conn.execute(
            "UPDATE folder_rules SET created_at = '2024-01-01 00:00:01' WHERE id = 'r-old'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE folder_rules SET created_at = '2024-01-01 00:00:02' WHERE id = 'r-new'",
            [],
        )
        .unwrap();
        let cands = queries::rule_candidate_rows(conn, Some("INBOX")).unwrap();
        let hits = evaluate_rules(conn, &cands);
        assert_eq!(hits.len(), 1, "exactly one hit expected, got {hits:?}");
        assert_eq!(hits[0].rule_id, "r-old", "tie goes to the oldest rule");
    }

    #[test]
    fn rule_conflict_detail_carries_both_options() {
        let store = seed_db();
        let conn = store.conn();
        let tax = crate::classify::taxonomy::load_default().unwrap();
        seed_rule(
            conn, "r-1", "Boletos", 10,
            r#"{"sender_domain":"utfpr.edu.br"}"#, "folder", "Arquivo",
        );
        let mid = mid_for_uid(conn, 1);
        queries::upsert_label(conn, mid, "academico", None, 0.85, 0.6, false).unwrap();
        queries::upsert_rule_suggestion(
            conn,
            &RuleSuggestionRow {
                message_id: mid,
                rule_id: "r-1".to_string(),
                dest_kind: "folder".to_string(),
                destination: "Arquivo".to_string(),
            },
        )
        .unwrap();
        let detail = suggestion_detail_inner(conn, mid, &tax).unwrap().unwrap();
        // Classifier side intact (never silently overridden)…
        assert_eq!(detail.primary_id, "academico");
        // …and the rule side rides along for the user pick.
        let alt = detail.rule_alternative.expect("rule alternative must be present");
        assert_eq!(alt.rule_id, "r-1");
        assert_eq!(alt.destination, "Arquivo");
        assert!(alt.justification.contains("Boletos"));
        assert!(alt.overlap_note.is_none(), "plain folder must not warn overlap");
    }

    #[test]
    fn rule_conflict_overlap_named_for_auto_destinations() {
        assert!(rule_overlap_note("folder", "Arquivo").is_none());
        let note = rule_overlap_note("folder", "Auto/Finanças")
            .expect("Auto/-nested folder must warn overlap");
        assert!(note.contains("gerenciada pelo classificador"));
        let cat = rule_overlap_note("category", "academico")
            .expect("taxonomy-category destinations always overlap");
        assert!(cat.contains("gerenciada pelo classificador"));
        // Tree-path split: two levels route via ensure_auto_tree.
        assert_eq!(
            split_auto_nest("Auto/Finanças"),
            Some(("Finanças".to_string(), None))
        );
        assert_eq!(
            split_auto_nest("Auto/Finanças/Contas"),
            Some(("Finanças".to_string(), Some("Contas".to_string())))
        );
        assert!(split_auto_nest("Arquivo").is_none());
        assert!(split_auto_nest("Auto/A/B/C").is_none(), "3+ levels fall back to guarded ensure");
    }

    #[test]
    fn rule_conflict_below_threshold_never_auto_moves() {
        // Below-threshold `A Classificar` mail: rules MAY propose, but
        // ALWAYS behind the confirm dialog — evaluation alone must leave
        // every durable outbox empty.
        let store = seed_db();
        let conn = store.conn();
        seed_rule(
            conn, "r-1", "Boletos", 10,
            r#"{"sender_domain":"utfpr.edu.br"}"#, "folder", "Arquivo",
        );
        let mid = mid_for_uid(conn, 1);
        queries::upsert_label(conn, mid, "academico", None, 0.2, 0.6, true).unwrap();
        // Labeled mail is classifier territory: no rule suggestion fires —
        // and nothing moved anywhere.
        let cands = queries::rule_candidate_rows(conn, Some("INBOX")).unwrap();
        assert!(evaluate_rules(conn, &cands).iter().all(|h| h.message_id != mid));
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(queries::imap_outbox_count(conn, mb as u64).unwrap(), 0);
        assert_eq!(queries::outbox_count(conn, mb as u64).unwrap(), 0);
    }

    #[test]
    fn rule_conflict_dismissal_keeps_mail_in_place() {
        let store = seed_db();
        let conn = store.conn();
        seed_rule(
            conn, "r-1", "Boletos", 10,
            r#"{"sender_domain":"utfpr.edu.br"}"#, "folder", "Arquivo",
        );
        let mid = mid_for_uid(conn, 1);
        queries::upsert_label(conn, mid, "academico", None, 0.85, 0.6, false).unwrap();
        queries::upsert_rule_suggestion(
            conn,
            &RuleSuggestionRow {
                message_id: mid,
                rule_id: "r-1".to_string(),
                dest_kind: "folder".to_string(),
                destination: "Arquivo".to_string(),
            },
        )
        .unwrap();
        // Dismiss path (same writes `dismiss_suggestion` performs).
        queries::dismiss_label(conn, mid).unwrap();
        queries::delete_rule_suggestion(conn, mid).unwrap();
        // Mail untouched: row present, not hidden, nothing queued.
        let loc = queries::message_location(conn, mid).unwrap();
        assert!(loc.is_some(), "dismissal must keep mail in place");
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| r.get(0))
            .unwrap();
        assert!(!queries::is_pending_delete(conn, mb as u64, 1).unwrap());
        assert_eq!(queries::imap_outbox_count(conn, mb as u64).unwrap(), 0);
        assert_eq!(queries::outbox_count(conn, mb as u64).unwrap(), 0);
    }
}

#[cfg(test)]
mod rule_evolv_tests {
    use super::*;
    use crate::classify::taxonomy::{Category, Taxonomy};
    use crate::store::Store;

    fn two_cat_tax() -> Taxonomy {
        Taxonomy {
            version: 1,
            name: "t".to_string(),
            categories: vec![
                Category {
                    id: "cat-a".to_string(),
                    name: "A".to_string(),
                    parent: None,
                    keywords: vec![],
                    rule: String::new(),
                },
                Category {
                    id: "cat-b".to_string(),
                    name: "B".to_string(),
                    parent: None,
                    keywords: vec![],
                    rule: String::new(),
                },
            ],
        }
    }

    fn seed_taxonomy_db() -> Store {
        let store = Store::open_in_memory().unwrap();
        let tax = two_cat_tax();
        let json = serde_json::to_string(&tax).unwrap();
        queries::install_taxonomy(store.conn(), 1, "t", &json).unwrap();
        store
    }

    fn seed_cat_rule(conn: &rusqlite::Connection, id: &str, name: &str, dest: &str) {
        queries::insert_rule(
            conn,
            &RuleRow {
                id: id.to_string(),
                name: name.to_string(),
                priority: 10,
                conditions_json: r#"{"sender_domain":"x.com"}"#.to_string(),
                dest_kind: "category".to_string(),
                destination: dest.to_string(),
                scope_json: "[]".to_string(),
                enabled: true,
            },
        )
        .unwrap();
    }

    #[test]
    fn rule_evolv_merge_remaps_rule_destination_ids() {
        let store = seed_taxonomy_db();
        let conn = store.conn();
        seed_cat_rule(conn, "r-1", "R", "cat-a");
        // Same migration as the label remap: merge cat-a into cat-b.
        let n = queries::remap_rule_destinations(conn, "cat-a", "cat-b").unwrap();
        assert_eq!(n, 1);
        let row = queries::get_rule(conn, "r-1").unwrap().unwrap();
        assert_eq!(row.destination, "cat-b");
        assert!(row.enabled, "merge remap must not disable the rule");
    }

    #[test]
    fn rule_evolv_delete_reassigns_or_disables_with_notice() {
        let store = seed_taxonomy_db();
        let conn = store.conn();
        seed_cat_rule(conn, "r-1", "VaiParaB", "cat-a");
        // Persist the deletion (bump revalidation disables the orphan)…
        let mut tax = read_taxonomy(conn).unwrap();
        taxedit::apply_delete(&mut tax, "cat-a").unwrap();
        persist_taxonomy(conn, &tax).unwrap();
        let disabled_row = queries::get_rule(conn, "r-1").unwrap().unwrap();
        assert!(!disabled_row.enabled, "bump must stale-disable the orphan");
        // …then the orphan-prompt reassignment revives it at the new id.
        let (remapped, notices) = reassign_deleted_rules(conn, "cat-a", "cat-b").unwrap();
        assert_eq!(remapped, 1);
        assert!(notices.is_empty());
        let row = queries::get_rule(conn, "r-1").unwrap().unwrap();
        assert_eq!(row.destination, "cat-b");
        assert!(row.enabled, "reassigned rule stays alive");
        // Unknown target: disable with notice, NEVER drop the row.
        seed_cat_rule(conn, "r-2", "Orfa", "cat-a");
        let (remapped2, notices2) = reassign_deleted_rules(conn, "cat-a", "cat-ghost").unwrap();
        assert_eq!(remapped2, 0);
        assert_eq!(notices2.len(), 1);
        assert!(notices2[0].contains("Orfa"), "notice names the rule: {}", notices2[0]);
        assert!(queries::get_rule(conn, "r-2").unwrap().is_some(), "disabled, never dropped");
    }

    #[test]
    fn rule_evolv_rename_never_breaks_rules() {
        let store = seed_taxonomy_db();
        let conn = store.conn();
        seed_cat_rule(conn, "r-1", "R", "cat-a");
        let mut tax = read_taxonomy(conn).unwrap();
        taxedit::apply_rename(&mut tax, "cat-a", "A Renomeada").unwrap();
        persist_taxonomy(conn, &tax).unwrap();
        // ID-stable: the rule still resolves to the renamed display path.
        let tax2 = read_taxonomy(conn).unwrap();
        let (display, _, _) = filing::auto_path_for_id(&tax2, "cat-a").unwrap();
        assert!(display.contains("A Renomeada"), "got {display}");
        let row = queries::get_rule(conn, "r-1").unwrap().unwrap();
        assert!(row.enabled, "rename must not touch rules");
        assert!(revalidate_rules(conn, &tax2).unwrap().is_empty());
    }

    #[test]
    fn rule_evolv_bump_revalidates_never_drops() {
        let store = seed_taxonomy_db();
        let conn = store.conn();
        seed_cat_rule(conn, "r-ok", "Boa", "cat-a");
        seed_cat_rule(conn, "r-bad", "Fantasma", "cat-ghost");
        let tax = read_taxonomy(conn).unwrap();
        let notices = revalidate_rules(conn, &tax).unwrap();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("Fantasma"));
        // Invalid rule disabled AND kept; valid rule untouched.
        let bad = queries::get_rule(conn, "r-bad").unwrap().unwrap();
        assert!(!bad.enabled);
        let ok = queries::get_rule(conn, "r-ok").unwrap().unwrap();
        assert!(ok.enabled);
        assert_eq!(queries::list_rules(conn).unwrap().len(), 2, "revalidate never drops rows");
    }
}

// ── Phase 25: journaled bulk apply_rules (25-02, RULE-02 bulk depth) ──
//
// Reuse-depth decision (CONTEXT agent discretion, prescribed here):
// - THIN loop = `apply_rules` (above): few hits, one shot, dialog ON per
//   hit through `confirm_suggestion`. No journal, no undo.
// - JOURNALED bulk = `apply_rules_bulk` (below): whole-account (or folder)
//   runs with progress events + undo. Reuses the Phase 20 machinery
//   wholesale — `batch_runs`/`batch_items` journal, `journal_intent`/
//   `journal_moved`/`journaled_ids`, per-chunk UIDVALIDITY re-check,
//   cooperative cancel, and the EXISTING `undo_batch`/`batch_report`
//   commands (both are generic over `batch_items`; `label_id` carries the
//   rule id). No duplicate journal table, no new undo path.
// Cutoff: the UI offers the thin path for scoped runs and the journaled
// path for whole-account runs; both funnel hits through the same
// `evaluate_rules` core and the same epoch-gated `move_one_uid`.

use crate::classify::batch as rule_batch;

/// Bulk progress event (`rule-progress` channel).
#[derive(Debug, Clone, Serialize)]
pub struct RuleBulkProgress {
    pub run_id: i64,
    pub done: usize,
    pub total: usize,
    pub moved: usize,
    pub skipped: usize,
    pub state: String,
}

/// Plannable bulk hit: rule hit + source wire for journaling. Pure —
/// snapshot candidates, evaluate, attach current wires. Skips Auto/-sourced
/// mail (already filed — same spirit as the batch collector).
pub struct PlannedRuleHit {
    pub hit: RuleHit,
    pub from_folder: String,
}

pub fn plan_rule_bulk(
    conn: &rusqlite::Connection,
    mailbox: Option<&str>,
) -> Result<Vec<PlannedRuleHit>, String> {
    let cands = queries::rule_candidate_rows(conn, mailbox).map_err(|e| e.to_string())?;
    let hits = evaluate_rules(conn, &cands);
    let mut out = Vec::new();
    for h in hits {
        let loc = queries::message_location(conn, h.message_id)
            .map_err(|e| e.to_string())?;
        if let Some((mb, _)) = loc {
            if mb == "Auto" || mb.starts_with("Auto/") || mb.starts_with("Auto.") {
                continue;
            }
            out.push(PlannedRuleHit { hit: h, from_folder: mb });
        }
    }
    Ok(out)
}

/// Journaled whole-account (or per-folder) rule run. One explicit op with
/// progress + report + undo (via `batch_report`/`undo_batch`). Dialog OFF
/// by design — the journal IS the backstop. Destinations are pre-ensured
/// up front (dialog-off means no mid-run asks); a destination that needs
/// an `Auto`-collision confirm is SKIPPED with its hits counted, never
/// forced. Sent/Drafts auto-excluded from NOTHING here — bulk is manual
/// intent (`apply_rules` semantics), served on demand. Returns the run id.
#[tauri::command]
pub async fn apply_rules_bulk(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    mailbox: Option<String>,
) -> Result<i64, String> {
    let store = state.store.clone();
    let account_cfg = load_account_config(state.clone()).await?;
    let manager = manager_for(&state, &account_cfg);
    let cancel = state.batch_cancel.clone();
    cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    // Snapshot the plan (short lock, no awaits).
    let planned = {
        let guard = store.lock().unwrap();
        plan_rule_bulk(guard.conn(), mailbox.as_deref())?
    };
    // Group by destination for one ensure per folder.
    use std::collections::BTreeMap;
    let mut by_dest: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, p) in planned.iter().enumerate() {
        by_dest
            .entry((p.hit.dest_kind.clone(), p.hit.destination.clone()))
            .or_default()
            .push(i);
    }
    let tax = load_default().map_err(|e| e.to_string())?;
    // Pre-ensure every destination (verbs here, never mid-run).
    let mut wires: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut skipped_dest = 0usize;
    for (key, _) in &by_dest {
        let (kind, dest) = key;
        let wire: Result<String, String> = if kind == "category" {
            let (display, top, child) =
                filing::auto_path_for_id(&tax, dest).map_err(|e| e.to_string())?;
            let probe = filing::FilePlan {
                message_id: 0,
                mailbox: String::new(),
                uid: 0,
                dest_display: display,
                top_name: top,
                child_name: child,
                threshold_used: 0.0,
                confidence: 0.0,
                needs_review: false,
            };
            filing::ensure_auto_tree(&manager, &store, &probe, true)
                .await
                .map_err(|e| e.to_string())
        } else if let Some((top, child)) = split_auto_nest(dest) {
            let probe = filing::FilePlan {
                message_id: 0,
                mailbox: String::new(),
                uid: 0,
                dest_display: dest.clone(),
                top_name: top,
                child_name: child,
                threshold_used: 0.0,
                confidence: 0.0,
                needs_review: false,
            };
            filing::ensure_auto_tree(&manager, &store, &probe, true)
                .await
                .map_err(|e| e.to_string())
        } else {
            ensure_rule_folder(&manager, &store, dest).await
        };
        match wire {
            Ok(w) => {
                wires.insert(key.clone(), w);
            }
            Err(e) => {
                // Collision-or-offline: skip the destination's hits loudly
                // (counted, never forced, never half-ensured).
                eprintln!("[SGE rules] bulk: destino ignorado ({dest}: {e})");
                skipped_dest += by_dest[key].len();
            }
        }
    }
    // Journal intents BEFORE any verb (crash safety) + capture epochs.
    let run_id = {
        let guard = store.lock().unwrap();
        crate::store::queries::create_batch_run(guard.conn()).map_err(|e| e.to_string())?
    };
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
    let total = planned.len();
    // Keep only ensurable hits for the loop.
    let mut jobs: Vec<(u64, String, String, String)> = Vec::new();
    for p in &planned {
        if let Some(w) = wires.get(&(p.hit.dest_kind.clone(), p.hit.destination.clone())) {
            jobs.push((p.hit.message_id, p.from_folder.clone(), w.clone(), p.hit.rule_id.clone()));
        }
    }
    {
        let guard = store.lock().unwrap();
        for (chunk_no, chunk) in jobs.chunks(rule_batch::BATCH_CHUNK).enumerate() {
            for (mid, from_folder, to_wire, rule_id) in chunk {
                let _ = rule_batch::journal_intent(
                    guard.conn(),
                    run_id,
                    *mid,
                    from_folder,
                    to_wire,
                    rule_id,
                    chunk_no,
                );
            }
        }
    }
    let app_h = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut done = 0usize;
        let mut moved = 0usize;
        let mut skipped = skipped_dest;
        for chunk in jobs.chunks(rule_batch::BATCH_CHUNK) {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            for (mid, _from_folder, to_wire, rule_id) in chunk {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // Current location (uid may have shifted) + per-chunk
                // UIDVALIDITY re-check: abort-to-resync on shift.
                let loc = {
                    let guard = store.lock().unwrap();
                    queries::message_location(guard.conn(), *mid).ok().flatten()
                };
                let Some((mb_now, uid_now)) = loc else {
                    done += 1;
                    skipped += 1;
                    continue;
                };
                let epoch_now: u32 = {
                    let guard = store.lock().unwrap();
                    guard
                        .conn()
                        .query_row(
                            "SELECT uid_validity FROM mailboxes WHERE name = ?1",
                            rusqlite::params![mb_now],
                            |r| r.get(0),
                        )
                        .unwrap_or(0)
                };
                if epochs.get(&mb_now).copied().unwrap_or(0) != epoch_now {
                    done += 1;
                    skipped += 1;
                    continue;
                }
                if mb_now == *to_wire {
                    let guard = store.lock().unwrap();
                    let _ = rule_batch::journal_moved(guard.conn(), run_id, *mid);
                    moved += 1;
                    done += 1;
                    continue;
                }
                match filing::move_one_uid(
                    &manager,
                    &store,
                    &mb_now,
                    uid_now,
                    to_wire,
                    "Arquivado por regra",
                )
                .await
                {
                    Ok(_) => {
                        let guard = store.lock().unwrap();
                        let _ = rule_batch::journal_moved(guard.conn(), run_id, *mid);
                        let _ = queries::delete_rule_suggestion(guard.conn(), *mid);
                        // Pin the outcome against re-fire (same as single).
                        let dest = to_wire.clone();
                        let _ = queries::log_override(guard.conn(), *mid, rule_id, &dest);
                        moved += 1;
                    }
                    Err(_) => {
                        skipped += 1;
                    }
                }
                done += 1;
            }
            let _ = app_h.emit(
                "rule-progress",
                RuleBulkProgress {
                    run_id,
                    done,
                    total,
                    moved,
                    skipped,
                    state: "running".to_string(),
                },
            );
        }
        let interrupted = cancel.load(std::sync::atomic::Ordering::SeqCst);
        let totals = serde_json::json!({
            "kind": "rule-bulk", "done": done, "moved": moved, "skipped": skipped,
        })
        .to_string();
        {
            let guard = store.lock().unwrap();
            let _ = crate::store::queries::finish_batch_run(
                guard.conn(),
                run_id,
                if interrupted { "interrupted" } else { "done" },
                &totals,
            );
        }
        let _ = app_h.emit(
            "rule-progress",
            RuleBulkProgress {
                run_id,
                done,
                total,
                moved,
                skipped,
                state: if interrupted { "interrupted".to_string() } else { "done".to_string() },
            },
        );
    });
    Ok(run_id)
}

#[cfg(test)]
mod rule_bulk_tests {
    use super::*;
    use crate::store::Store;

    fn seed_db() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter) \
             VALUES ('INBOX', 77, 10, '/'), ('Arquivo', 77, 10, '/'), \
                     ('Auto/Finanças', 77, 10, '/')",
            [],
        )
        .unwrap();
        let inbox: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| r.get(0))
            .unwrap();
        let filed: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'Auto/Finanças'", [], |r| {
                r.get(0)
            })
            .unwrap();
        for (mb, uid, subject, from, preview) in [
            (inbox, 1u32, "Fatura de energia", "contas@utfpr.edu.br", "boleto de outubro"),
            (inbox, 2, "Reunião quinta", "chefe@empresa.com", "pauta"),
            (filed, 1u32, "Fatura antiga", "contas@utfpr.edu.br", "boleto antigo"),
        ] {
            queries::upsert_message(
                conn, mb as u64, uid, None, subject, from, "[]", "[]",
                "2024-01-01T00:00:00Z", "[]", false, preview,
            )
            .unwrap();
        }
        queries::insert_rule(
            conn,
            &RuleRow {
                id: "r-1".to_string(),
                name: "Boletos".to_string(),
                priority: 10,
                conditions_json: r#"{"sender_domain":"utfpr.edu.br"}"#.to_string(),
                dest_kind: "folder".to_string(),
                destination: "Arquivo".to_string(),
                scope_json: "[]".to_string(),
                enabled: true,
            },
        )
        .unwrap();
        store
    }

    #[test]
    fn rule_bulk_plan_skips_filed_pinned_labeled() {
        let store = seed_db();
        let conn = store.conn();
        // Filed mail under Auto/ never bulk-moves (already filed).
        let planned = plan_rule_bulk(conn, None).unwrap();
        assert_eq!(planned.len(), 1, "only the INBOX hit plans, got {}", planned.len());
        assert_eq!(planned[0].from_folder, "INBOX");
        assert_eq!(planned[0].hit.rule_id, "r-1");
        // Pin it: plan empties (override wins even in bulk).
        let mid = planned[0].hit.message_id;
        queries::log_override(conn, mid, "r-1", "Arquivo").unwrap();
        assert!(plan_rule_bulk(conn, None).unwrap().is_empty());
    }

    #[test]
    fn rule_bulk_journal_reuses_batch_machinery_with_undo() {
        // The bulk journal IS batch_items (label_id carries the rule id):
        // intent → moved → journaled_ids → undo_batch reverses via the
        // same rows `batch_report` reads. Proves reuse, no duplicate table.
        let store = seed_db();
        let conn = store.conn();
        let planned = plan_rule_bulk(conn, None).unwrap();
        assert_eq!(planned.len(), 1);
        let run = queries::create_batch_run(conn).unwrap();
        let p = &planned[0];
        rule_batch::journal_intent(conn, run, p.hit.message_id, &p.from_folder, "Arquivo", "r-1", 0)
            .unwrap();
        rule_batch::journal_moved(conn, run, p.hit.message_id).unwrap();
        assert_eq!(rule_batch::journaled_ids(conn, run).unwrap(), vec![p.hit.message_id]);
        // Report reads the same journal generically.
        let per: Vec<(String, usize)> = conn
            .prepare(
                "SELECT label_id, count(*) FROM batch_items WHERE run_id = ?1 AND moved = 1 GROUP BY label_id",
            )
            .unwrap()
            .query_map(rusqlite::params![run], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(per, vec![("r-1".to_string(), 1)]);
        queries::finish_batch_run(conn, run, "done", r#"{"kind":"rule-bulk"}"#).unwrap();
    }
}
