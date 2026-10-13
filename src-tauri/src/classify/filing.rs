//! Confirm-gated filing (Phase 18, Plan 18-01): suggest → confirm → MOVE.
//!
//! Backend-enforced trust: `confirm_suggestion` moves mail ONLY from an
//! existing `labels` row into `Auto/<Top>/<Child>` via the proven Phase 10
//! path (`move_message_in` + `imap_outbox`). No label row ⇒ refuse. Direct
//! IPC with forged args cannot file unclassified mail — the gate is here,
//! not in the UI.
//!
//! Zero new IMAP verbs: CREATE-when-missing (Phase 11 guards) + MOVE
//! (Phase 10 machinery, offline → outbox replay).

use std::sync::{Arc, Mutex};

use serde::Serialize;
use thiserror::Error;

use super::evidence::build_evidence;
use super::taxonomy::{children_of, match_keywords, Taxonomy};
use crate::commands::sync::{prepare_create_wire, refresh_mailbox_tree};
use crate::imap::manager::SessionManager;
use crate::store::queries;
use crate::store::Store;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FilingError {
    #[error("sem sugestão para este e-mail — classifique antes de mover")]
    NoSuggestion,
    #[error("sugestão desatualizada (a taxonomia mudou) — reclassifique antes de mover")]
    StaleSuggestion,
    #[error("este e-mail já está na pasta de destino")]
    AlreadyThere,
    #[error("categoria desconhecida: {0}")]
    UnknownCategory(String),
    #[error("COLLISION")]
    CollisionNeedsConfirm,
    #[error("{0}")]
    Store(String),
    #[error("{0}")]
    Imap(String),
}

/// Filing plan: pure reads, no verbs. The command awaits tree/moves after.
#[derive(Debug, Clone)]
pub struct FilePlan {
    pub message_id: u64,
    pub mailbox: String,
    pub uid: u32,
    pub dest_display: String,
    pub top_name: String,
    pub child_name: Option<String>,
    pub threshold_used: f64,
    pub confidence: f64,
    pub needs_review: bool,
}

/// Confirm outcome surfaced to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ConfirmResult {
    pub dest: String,
    pub acked: bool,
    pub pending: bool,
    pub detail: String,
}

/// Build the filing plan from the label row + cached fields. Refuses when:
/// no label (nothing suggested), stale (taxonomy changed underneath),
/// unknown category, or already in the destination.
pub fn plan_filing(
    conn: &rusqlite::Connection,
    message_id: u64,
    tax: &Taxonomy,
) -> Result<FilePlan, FilingError> {
    let label = queries::get_label(conn, message_id)
        .map_err(|e| FilingError::Store(e.to_string()))?
        .ok_or(FilingError::NoSuggestion)?;
    if label.stale {
        return Err(FilingError::StaleSuggestion);
    }
    let top = tax
        .categories
        .iter()
        .find(|c| c.id == label.primary_id && c.parent.is_none())
        .ok_or_else(|| FilingError::UnknownCategory(label.primary_id.clone()))?;
    // Recompute the child from cached fields (deterministic given the same
    // taxonomy version; stale rows refused above).
    let fields = queries::message_classify_fields(conn, message_id)
        .map_err(|e| FilingError::Store(e.to_string()))?
        .ok_or(FilingError::NoSuggestion)?;
    let (subject, snippet, from_addr, has_attachments) = fields;
    let domain = from_addr.rsplit('@').next().unwrap_or("");
    let is_reply = subject.to_lowercase().starts_with("re:");
    let evidence = build_evidence(&subject, &snippet, domain, has_attachments, is_reply);
    let kids: Vec<&super::taxonomy::Category> = children_of(tax, &top.id);
    let child = match_keywords(&evidence, &kids).into_iter().next().map(|(id, _)| {
        tax.categories
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or(id)
    });
    let dest_display = match &child {
        Some(c) => format!("Auto/{}/{}", top.name, c),
        None => format!("Auto/{}", top.name),
    };
    let (mailbox, uid) = queries::message_location(conn, message_id)
        .map_err(|e| FilingError::Store(e.to_string()))?
        .ok_or(FilingError::NoSuggestion)?;
    Ok(FilePlan {
        message_id,
        mailbox,
        uid,
        dest_display,
        top_name: top.name.clone(),
        child_name: child,
        threshold_used: label.threshold,
        confidence: label.confidence,
        needs_review: label.needs_review,
    })
}

/// Ensure the `Auto/` tree levels exist. Returns the destination WIRE name.
///
/// Levels are created via `prepare_create_wire` (leaf validation + mutf7 +
/// exists pre-check) + `create_mailbox_in`, then the tree refreshes.
/// Collision: a user-owned `Auto` without our marker refuses with
/// `CollisionNeedsConfirm` unless `allow_nest` (user approved the dialog) —
/// approval records the marker and nests under the existing `Auto`.
pub async fn ensure_auto_tree(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    plan: &FilePlan,
    allow_nest: bool,
) -> Result<String, FilingError> {
    // Brief lock: cached tree + delimiter + marker.
    let (cached, delimiter, marked) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let cached = queries::list_mailboxes(conn)
            .map_err(|e| FilingError::Store(e.to_string()))?;
        let delimiter = cached
            .iter()
            .find(|r| r.name == plan.mailbox)
            .map(|r| r.delimiter.clone())
            .or_else(|| cached.first().map(|r| r.delimiter.clone()))
            .unwrap_or_default();
        let marked = queries::get_setting(conn, "auto_root")
            .map_err(|e| FilingError::Store(e.to_string()))?
            .as_deref()
            == Some("ours");
        (cached, delimiter, marked)
    };
    let join = |parts: &[&str]| {
        if delimiter.is_empty() {
            parts.join("/")
        } else {
            parts.join(&delimiter)
        }
    };
    let root_wire = join(&["Auto"]);
    // Server truth for the collision check.
    let live = manager
        .list_mailboxes()
        .await
        .map_err(|e| FilingError::Imap(e.to_string()))?;
    let auto_exists = live.iter().any(|m| m.name == root_wire || m.name == "Auto");
    if auto_exists && !marked {
        if !allow_nest {
            return Err(FilingError::CollisionNeedsConfirm);
        }
        let guard = store.lock().unwrap();
        queries::set_setting(guard.conn(), "auto_root", "ours")
            .map_err(|e| FilingError::Store(e.to_string()))?;
    }
    // Level-by-level: Auto → Top → Child?.
    let mut parent_wire = root_wire.clone();
    if !live.iter().any(|m| m.name == parent_wire)
        && !cached.iter().any(|r| r.name == parent_wire)
    {
        let wire = prepare_create_wire(&cached, None, "Auto", &delimiter)
            .map_err(FilingError::Imap)?;
        manager
            .create_mailbox_in(&wire)
            .await
            .map_err(|e| FilingError::Imap(format!("não foi possível criar {wire}: {e}")))?;
        parent_wire = wire;
    }
    for level in std::iter::once(&plan.top_name).chain(plan.child_name.as_ref()) {
        let cached_now = {
            let guard = store.lock().unwrap();
            queries::list_mailboxes(guard.conn())
                .map_err(|e| FilingError::Store(e.to_string()))?
        };
        if cached_now.iter().any(|r| r.name == join(&[&parent_wire, level])) {
            parent_wire = join(&[&parent_wire, level]);
            continue;
        }
        let wire = prepare_create_wire(&cached_now, Some(&parent_wire), level, &delimiter)
            .map_err(FilingError::Imap)?;
        manager
            .create_mailbox_in(&wire)
            .await
            .map_err(|e| FilingError::Imap(format!("não foi possível criar {wire}: {e}")))?;
        parent_wire = wire;
    }
    // Refresh the cached tree (shared body) + record the marker.
    let _ = refresh_mailbox_tree(manager, store)
        .await
        .map_err(FilingError::Imap)?;
    {
        let guard = store.lock().unwrap();
        let _ = queries::set_setting(guard.conn(), "auto_root", "ours");
    }
    Ok(parent_wire)
}

/// Execute the gated MOVE: optimistic hide + durable outbox enqueue, then
/// immediate `move_message_in`; ack dequeues, failure stays queued for the
/// reconnect replay (mirrors `move_message` exactly).
pub async fn execute_filing_move(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    plan: &FilePlan,
    dest_wire: &str,
) -> Result<ConfirmResult, FilingError> {
    if plan.mailbox == *dest_wire {
        return Err(FilingError::AlreadyThere);
    }
    move_one_uid(
        manager,
        store,
        &plan.mailbox,
        plan.uid,
        dest_wire,
        &format!("Arquivado em {}", plan.dest_display),
    )
    .await
}

/// Single-UID optimistic move shared by confirm/override/merge/delete.
/// `ok_detail` is shown on ack; offline stays queued with a replay note.
pub async fn move_one_uid(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    mailbox: &str,
    uid: u32,
    dest_wire: &str,
    ok_detail: &str,
) -> Result<ConfirmResult, FilingError> {
    let mailbox_id = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let mb = queries::ensure_mailbox(conn, mailbox)
            .map_err(|e| FilingError::Store(e.to_string()))?;
        let epoch = queries::get_sync_state(conn, mailbox)
            .map_err(|e| FilingError::Store(e.to_string()))?
            .map(|(v, _)| v)
            .unwrap_or(0);
        queries::set_pending_delete(conn, mb, uid, true)
            .map_err(|e| FilingError::Store(e.to_string()))?;
        queries::enqueue_imap_outbox(
            conn,
            mb,
            uid,
            queries::IMAP_OP_MOVE,
            Some(dest_wire),
            epoch,
        )
        .map_err(|e| FilingError::Store(e.to_string()))?;
        mb
    };
    match manager
        .move_message_in(mailbox, &uid.to_string(), dest_wire)
        .await
    {
        Ok(_) => {
            let guard = store.lock().unwrap();
            let _ = queries::delete_imap_outbox_op(guard.conn(), mailbox_id, uid);
            Ok(ConfirmResult {
                dest: dest_wire.to_string(),
                acked: true,
                pending: false,
                detail: ok_detail.to_string(),
            })
        }
        Err(crate::imap::SyncError::Refused(msg)) => {
            let guard = store.lock().unwrap();
            let _ = queries::delete_imap_outbox_op(guard.conn(), mailbox_id, uid);
            let _ = queries::set_pending_delete(guard.conn(), mailbox_id, uid, false);
            Err(FilingError::Imap(format!("IMAP recusou: {msg}")))
        }
        Err(e) => {
            let guard = store.lock().unwrap();
            let _ =
                queries::record_imap_outbox_error(guard.conn(), mailbox_id, uid, &e.to_string());
            Ok(ConfirmResult {
                dest: dest_wire.to_string(),
                acked: false,
                pending: true,
                detail: format!("Sem conexão — será arquivado no próximo sync ({e})"),
            })
        }
    }
}

/// Override destination: `Auto/<Top>` or `Auto/<Top>/<Child>` display
/// path for any category id (top or child).
pub fn auto_path_for_id(
    tax: &Taxonomy,
    category_id: &str,
) -> Result<(String, String, Option<String>), FilingError> {
    let cat = tax
        .categories
        .iter()
        .find(|c| c.id == category_id)
        .ok_or_else(|| FilingError::UnknownCategory(category_id.to_string()))?;
    let (top_name, child_name) = match &cat.parent {
        None => (cat.name.clone(), None),
        Some(pid) => {
            let top = tax
                .categories
                .iter()
                .find(|c| &c.id == pid)
                .ok_or_else(|| FilingError::UnknownCategory(pid.clone()))?;
            (top.name.clone(), Some(cat.name.clone()))
        }
    };
    let dest = match &child_name {
        Some(c) => format!("Auto/{top_name}/{c}"),
        None => format!("Auto/{top_name}"),
    };
    Ok((dest, top_name, child_name))
}

#[cfg(test)]
mod tests {
    use super::super::taxonomy::load_default;
    use super::*;

    #[test]
    fn path_shapes_top_and_child() {
        let tax = load_default().unwrap();
        let (dest, top, child) = auto_path_for_id(&tax, "financeiro.cobrancas").unwrap();
        assert_eq!(dest, "Auto/Financeiro/Boletos e Cobranças");
        assert_eq!((top.as_str(), child.as_deref()), ("Financeiro", Some("Boletos e Cobranças")));
        let (dest2, _, child2) = auto_path_for_id(&tax, "comunidade").unwrap();
        assert_eq!(dest2, "Auto/Comunidade");
        assert_eq!(child2, None);
    }

    #[test]
    fn unknown_category_refused() {
        let tax = load_default().unwrap();
        assert!(matches!(
            auto_path_for_id(&tax, "inexistente.xyz"),
            Err(FilingError::UnknownCategory(_))
        ));
    }

    #[test]
    fn collision_error_is_distinct() {
        assert_eq!(
            FilingError::CollisionNeedsConfirm.to_string(),
            "COLLISION"
        );
    }
}

#[cfg(test)]
mod plan_tests {
    use super::super::taxonomy::load_default;
    use super::*;
    use crate::store::queries::*;
    use crate::store::Store;

    fn seeded() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb = ensure_mailbox(conn, "INBOX").unwrap();
        upsert_message(conn, mb, 7, None, "Boleto", "fin@utfpr.edu.br", "[]", "[]",
            "2026-10-10T00:00:00Z", "[]", false, "fatura e pagamento do boleto").unwrap();
        store
    }

    fn msg_id(store: &Store) -> u64 {
        let conn = store.conn();
        let mb = mailbox_id(conn, "INBOX").unwrap().unwrap();
        find_message_id(conn, mb, 7).unwrap().unwrap()
    }

    #[test]
    fn refuses_without_label() {
        let store = seeded();
        let tax = load_default().unwrap();
        assert!(matches!(
            plan_filing(store.conn(), msg_id(&store), &tax),
            Err(FilingError::NoSuggestion)
        ));
    }

    #[test]
    fn plans_top_and_child_destination() {
        let store = seeded();
        let mid = msg_id(&store);
        {
            let conn = store.conn();
            upsert_label(conn, mid, "financeiro", Some("academico"), 0.9, 0.6, false).unwrap();
        }
        let tax = load_default().unwrap();
        let plan = plan_filing(store.conn(), mid, &tax).unwrap();
        assert_eq!(plan.dest_display, "Auto/Financeiro/Boletos e Cobranças");
        assert_eq!(plan.top_name, "Financeiro");
        assert_eq!(plan.child_name.as_deref(), Some("Boletos e Cobranças"));
        assert_eq!(plan.mailbox.as_str(), "INBOX");
        assert_eq!(plan.uid, 7);
    }

    #[test]
    fn refuses_stale_and_unknown() {
        let store = seeded();
        let mid = msg_id(&store);
        {
            let conn = store.conn();
            upsert_label(conn, mid, "financeiro", None, 0.9, 0.6, false).unwrap();
            conn.execute("UPDATE labels SET stale = 1", []).unwrap();
        }
        let tax = load_default().unwrap();
        assert!(matches!(
            plan_filing(store.conn(), mid, &tax),
            Err(FilingError::StaleSuggestion)
        ));
        {
            let conn = store.conn();
            upsert_label(conn, mid, "fantasma.x", None, 0.9, 0.6, false).unwrap();
        }
        assert!(matches!(
            plan_filing(store.conn(), mid, &tax),
            Err(FilingError::UnknownCategory(_))
        ));
    }
}
