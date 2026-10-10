//! Batch reorganization engine (Phase 20, Plan 20-01).
//!
//! Whole-account classify as a loop over the PROVEN confirm path with the
//! dialog off and progress on: pre-created tree, chunked drain with
//! per-chunk UIDVALIDITY re-checks, journal-before-move, resume, undo.
//!
//! Explicitly UNCONFIRMED by design (one account-level decision with a
//! persisted report + undo) — distinct from single-email always-confirmed.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use super::filing::move_one_uid;
use super::suggest::DEFAULT_THRESHOLD;
use super::taxonomy::{children_of, load_default, top_level};
use super::worker::suggest_label;
use crate::commands::sync::prepare_create_wire;
use crate::imap::manager::SessionManager;
use crate::store::queries;
use crate::store::Store;

/// Batch chunk size (25–50 per roadmap; start low against the throttling
/// server, tuned live in 20-01 verification).
pub const BATCH_CHUNK: usize = 25;

#[derive(Debug, Clone, Serialize)]
pub struct BatchProgress {
    pub run_id: i64,
    pub done: usize,
    pub total: usize,
    pub moved: usize,
    pub review: usize,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchReport {
    pub run_id: i64,
    pub state: String,
    pub per_category: Vec<(String, usize)>,
    pub remainder: usize,
    pub undone: usize,
}

/// Pre-create the FULL `Auto/` tree up front (all tops + children).
/// Returns the number of folders created.
pub async fn ensure_full_tree(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
) -> Result<usize, String> {
    let tax = load_default().map_err(|e| e.to_string())?;
    let cached = {
        let guard = store.lock().unwrap();
        queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
    };
    let delimiter = cached.first().map(|r| r.delimiter.clone()).unwrap_or_default();
    let join = |parts: &[&str]| {
        if delimiter.is_empty() {
            parts.join("/")
        } else {
            parts.join(&delimiter)
        }
    };
    let root = join(&["Auto"]);
    let mut created = 0usize;
    // Re-read liveness once (server truth); create missing level by level.
    let live = manager.list_mailboxes().await.map_err(|e| e.to_string())?;
    let mut known: Vec<String> = live.into_iter().map(|m| m.name).collect();
    for row in &cached {
        if !known.contains(&row.name) {
            known.push(row.name.clone());
        }
    }
    // Root first.
    if !known.iter().any(|n| n == &root || n == "Auto") {
        let wire = prepare_create_wire(&cached, None, "Auto", &delimiter)
            .map_err(|e| e.to_string())?;
        manager.create_mailbox_in(&wire).await.map_err(|e| e.to_string())?;
        known.push(wire);
        created += 1;
    }
    let root_wire = if known.iter().any(|n| n == &root) {
        root.clone()
    } else {
        "Auto".to_string()
    };
    for top in top_level(&tax) {
        let top_wire = join(&[&root_wire, &top.name]);
        if !known.contains(&top_wire) {
            let cached_now = {
                let guard = store.lock().unwrap();
                queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
            };
            let wire = prepare_create_wire(&cached_now, Some(&root_wire), &top.name, &delimiter)
                .map_err(|e| e.to_string())?;
            manager.create_mailbox_in(&wire).await.map_err(|e| e.to_string())?;
            known.push(wire.clone());
            created += 1;
        }
        let top_wire = known
            .iter()
            .find(|n| n.ends_with(top.name.as_str()))
            .cloned()
            .unwrap_or(top_wire);
        for child in children_of(&tax, &top.id) {
            let child_wire = join(&[&top_wire, &child.name]);
            if !known.contains(&child_wire) {
                let cached_now = {
                    let guard = store.lock().unwrap();
                    queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
                };
                let wire = prepare_create_wire(&cached_now, Some(&top_wire), &child.name, &delimiter)
                    .map_err(|e| e.to_string())?;
                manager.create_mailbox_in(&wire).await.map_err(|e| e.to_string())?;
                known.push(wire);
                created += 1;
            }
        }
    }
    Ok(created)
}

/// Candidate rows for a batch run: (message_id, mailbox, uid) for mail in
/// scope folders that is NOT already under the `Auto/` tree and NOT in an
/// excluded folder. Pinned-override mail is skipped (corrections stand).
pub fn collect_candidates(
    conn: &rusqlite::Connection,
    scope_folder: Option<&str>,
) -> Result<Vec<(u64, String, u32)>, String> {
    let excluded = queries::excluded_folders(conn).map_err(|e| e.to_string())?;
    let mailboxes: Vec<(String, u32)> = conn
        .prepare("SELECT name, uid_validity FROM mailboxes")
        .map_err(|e| e.to_string())?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (mb, _uidv) in mailboxes {
        if mb == "Auto" || mb.starts_with("Auto/") || mb.starts_with("Auto.") {
            continue;
        }
        if excluded.iter().any(|e| e == &mb) {
            continue;
        }
        if let Some(scope) = scope_folder {
            if scope != mb {
                continue;
            }
        }
        let rows: Vec<(u64, u32)> = conn
            .prepare(
                "SELECT m.id, m.uid FROM messages m JOIN mailboxes mb ON m.mailbox_id = mb.id
                 WHERE mb.name = ?1",
            )
            .map_err(|e| e.to_string())?
            .query_map(rusqlite::params![mb], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for (mid, uid) in rows {
            // Skip pinned corrections + already-labeled-and-filed mail is
            // impossible here (nothing under Auto/ is in scope) — but skip
            // dismissed suggestions? NO: batch re-processes dismissed
            // (explicit whole-account op beats per-message dismiss).
            let pinned = queries::latest_override(conn, mid).map_err(|e| e.to_string())?;
            if pinned.is_some() {
                continue;
            }
            out.push((mid, mb.clone(), uid));
        }
    }
    Ok(out)
}

/// Journal a move intent BEFORE the verb fires (crash safety).
pub fn journal_intent(
    conn: &rusqlite::Connection,
    run_id: i64,
    message_id: u64,
    from_folder: &str,
    to_folder: &str,
    label_id: &str,
    chunk: usize,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO batch_items (run_id, message_id, from_folder, to_folder, label_id, chunk)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![run_id, message_id, from_folder, to_folder, label_id, chunk as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn journal_moved(conn: &rusqlite::Connection, run_id: i64, message_id: u64) -> Result<(), String> {
    conn.execute(
        "UPDATE batch_items SET moved = 1 WHERE run_id = ?1 AND message_id = ?2",
        rusqlite::params![run_id, message_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Already-journaled message ids for a run (resume skips them).
pub fn journaled_ids(conn: &rusqlite::Connection, run_id: i64) -> Result<Vec<u64>, String> {
    conn.prepare("SELECT message_id FROM batch_items WHERE run_id = ?1")
        .map_err(|e| e.to_string())?
        .query_map(rusqlite::params![run_id], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Undo-batch: reverse every `moved` row (newest first), mark undone.
pub async fn undo_run(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    run_id: i64,
) -> Result<usize, String> {
    let rows: Vec<(u64, String, String)> = {
        let guard = store.lock().unwrap();
        let conn: &rusqlite::Connection = guard.conn();
        let mut stmt = conn
            .prepare(
                "SELECT message_id, from_folder, to_folder FROM batch_items
                 WHERE run_id = ?1 AND moved = 1 AND undone = 0 ORDER BY id DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(u64, String, String)> = stmt
            .query_map(rusqlite::params![run_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let mut restored = 0usize;
    for (mid, from_folder, _to_folder) in &rows {
        // Locate current uid: the message now lives in to_folder.
        let current: Option<(String, u32)> = {
            let guard = store.lock().unwrap();
            queries::message_location(guard.conn(), *mid)
                .map_err(|e| e.to_string())?
        };
        if let Some((mailbox, uid)) = current {
            move_one_uid(
                manager,
                store,
                &mailbox,
                uid,
                from_folder,
                "Restaurado pelo desfazer",
            )
            .await
            .map_err(|e| e.to_string())?;
            restored += 1;
        }
    }
    {
        let guard = store.lock().unwrap();
        guard
            .conn()
            .execute(
                "UPDATE batch_items SET undone = 1 WHERE run_id = ?1 AND moved = 1",
                rusqlite::params![run_id],
            )
            .map_err(|e| e.to_string())?;
        queries::finish_batch_run(guard.conn(), run_id, "undone", "{}")
            .map_err(|e| e.to_string())?;
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::queries::*;
    use crate::store::Store;

    fn seeded() -> (Store, u64, u64) {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let inbox = ensure_mailbox(conn, "INBOX").unwrap();
        let sent = ensure_mailbox(conn, "Sent").unwrap();
        upsert_message(conn, inbox, 1, None, "Boleto", "f@x", "[]", "[]",
            "2026-10-10T00:00:00Z", "[]", false, "").unwrap();
        upsert_message(conn, sent, 1, None, "Oi", "g@x", "[]", "[]",
            "2026-10-10T00:00:00Z", "[]", false, "").unwrap();
        let a = find_message_id(conn, inbox, 1).unwrap().unwrap();
        let b = find_message_id(conn, sent, 1).unwrap().unwrap();
        // Sent is excluded by default in production (seed); emulate here.
        exclude_folder(conn, "Sent", "test").unwrap();
        (store, a, b)
    }

    #[test]
    fn candidates_skip_auto_tree_and_excluded() {
        let (store, a, _b) = seeded();
        let conn = store.conn();
        let cands = collect_candidates(conn, None).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].0, a);
        assert_eq!(cands[0].1, "INBOX");
        // Scope filter narrows further.
        assert!(collect_candidates(conn, Some("Sent")).unwrap().is_empty());
    }

    #[test]
    fn journal_roundtrip_enables_resume_skip() {
        let (store, a, _b) = seeded();
        let conn = store.conn();
        let run = create_batch_run(conn).unwrap();
        journal_intent(conn, run, a, "INBOX", "Auto/Financeiro", "financeiro", 0).unwrap();
        assert_eq!(journaled_ids(conn, run).unwrap(), vec![a]);
        journal_moved(conn, run, a).unwrap();
        // Resume view: journaled rows are skipped by the caller.
        let cands = collect_candidates(conn, None).unwrap();
        let done = journaled_ids(conn, run).unwrap();
        let remaining: Vec<_> = cands.into_iter().filter(|(m, _, _)| !done.contains(m)).collect();
        assert!(remaining.is_empty());
    }

    #[test]
    fn pinned_corrections_never_enter_batch() {
        let (store, a, _b) = seeded();
        let conn = store.conn();
        log_override(conn, a, "academico", "financeiro").unwrap();
        let cands = collect_candidates(conn, None).unwrap();
        assert!(cands.is_empty(), "pinned mail must not enter batch");
    }
}
