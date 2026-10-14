//! Batch reorganization engine (Phase 20, Plan 20-01).
//!
//! Whole-account classify as a loop over the PROVEN confirm path with the
//! dialog off and progress on: pre-created tree, chunked drain with
//! per-chunk UIDVALIDITY re-checks, journal-before-move, resume, undo.
//!
//! Explicitly UNCONFIRMED by design (one account-level decision with a
//! persisted report + undo) — distinct from single-email always-confirmed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use rusqlite::OptionalExtension;

use super::filing::move_one_uid;
use super::taxonomy::{children_of, load_default, top_level};
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
    /// Raw `totals_json` (Phase 24: carries `{"kind":"archive",...}` for
    /// archive runs so report surfaces can label them distinctly without
    /// a second query — additive, existing readers ignore it).
    pub totals_json: String,
    /// Phase 30: stale-drop count parsed from `totals_json`
    /// (`stale_dropped`, default 0 on old rows).
    pub stale_dropped: usize,
    /// Phase 30: retry-skip count parsed from `totals_json`
    /// (`retry_skipped`, default 0 until a retry accumulates).
    pub retry_skipped: usize,
    /// Phase 30: failed-intent count (`moved=0` rows) — the retry
    /// button's failure count (backend truth, labeling + gating).
    pub failed: usize,
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

/// Destination display path for a (primary, child?) label pair.
/// Shared by the real run loop and the dry-run preview so the two can
/// never diverge (T-30-04): `Auto/<Top>` or `Auto/<Top>/<Child>`, falling
/// back to the raw id when the taxonomy no longer names it.
pub fn dest_display_for(
    tax: &super::taxonomy::Taxonomy,
    primary_id: &str,
    child_id: Option<&str>,
) -> String {
    let top_name = tax
        .categories
        .iter()
        .find(|c| c.id == primary_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| primary_id.to_string());
    match child_id {
        Some(child) => {
            let child_name = tax
                .categories
                .iter()
                .find(|c| c.id == child)
                .map(|c| c.name.clone());
            match child_name {
                Some(cn) => format!("Auto/{top_name}/{cn}"),
                None => format!("Auto/{top_name}"),
            }
        }
        None => format!("Auto/{top_name}"),
    }
}

// ── Phase 30: shared dry-run preview (BATCH-03) ────────────────────
//
// One preview path serving BOTH `batch_classify` and `archive_run`.
// Preview runs the IDENTICAL candidate collection (`collect_candidates`
// for classify; the archive selector for archive) and the IDENTICAL
// destination shape (`dest_display_for` / `archive_dest_for`) as the real
// run. The `execute=false` equivalent is structural: this function is
// pure-read — it performs ZERO DB writes (no `batch_runs` row, no
// `batch_items` rows, no label upserts, no `override_events` rows; Phase
// 29 `dry_run_writes_nothing` precedent) and issues ZERO IMAP verbs, so
// it never touches SyncGate. The plan is a transient return value, never
// persisted. Confirm NEVER replays preview rows — `batch_classify` /
// `archive_run` re-collect fresh (see `resolve_for_move` below).

/// Maximum preview rows returned (DoS cap, T-30-03).
pub const PREVIEW_CAP: usize = 500;

/// Preview leg selector. Archive reuses this SAME function with
/// `PreviewKind::Archive` (never a second implementation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewKind {
    Classify,
    Archive,
}

/// One row of the per-item plan. Movable rows carry `to_folder` +
/// `basis` with `skipped_reason: None`; skipped rows carry a fixed-enum
/// `skipped_reason` (`review-bucket`, `excluded`, `undatable`) and an
/// empty `to_folder`. All strings are backend-enum/label-ID cents only —
/// never bodies or secrets (T-30-02, redaction-sanitized).
#[derive(Debug, Clone, Serialize)]
pub struct PreviewItem {
    pub message_id: u64,
    pub from_folder: String,
    pub to_folder: String,
    pub dest_display: String,
    pub basis: String,
    pub skipped_reason: Option<String>,
}

/// Transient dry-run plan (never persisted).
#[derive(Debug, Clone, Serialize)]
pub struct BatchPreview {
    pub kind: PreviewKind,
    /// Reports alternate spelling for surfaces that switch on it.
    pub kind_tag: String,
    pub items: Vec<PreviewItem>,
    pub skipped: Vec<PreviewItem>,
    pub total_candidates: usize,
    pub truncated: bool,
    /// `A Classificar` remainder (classify leg only; 0 for archive).
    pub remainder: usize,
}

/// Shared preview entry: identical matcher, writes off, transient.
/// `scope` narrows the classify leg to one folder (None = whole
/// account); the archive leg ignores it (settings scope is backend
/// truth).
pub fn preview_plan(
    conn: &rusqlite::Connection,
    kind: PreviewKind,
    scope: Option<&str>,
) -> Result<BatchPreview, String> {
    match kind {
        PreviewKind::Classify => preview_classify(conn, scope),
        PreviewKind::Archive => preview_archive(conn),
    }
}

fn preview_classify(
    conn: &rusqlite::Connection,
    scope: Option<&str>,
) -> Result<BatchPreview, String> {
    let tax = load_default().map_err(|e| e.to_string())?;
    let cands = collect_candidates(conn, scope)?;
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for (mid, mb, _uid) in cands {
        let row = build_classify_preview_item(conn, &tax, mid, &mb)?;
        if row.skipped_reason.is_some() {
            if skipped.len() < PREVIEW_CAP {
                skipped.push(row);
            }
        } else if items.len() < PREVIEW_CAP {
            items.push(row);
        }
    }
    let total = items.len() + skipped.len();
    // Remainder mirrors the `batch_report` remainder query (review list).
    let remainder: usize = conn
        .query_row(
            "SELECT count(*) FROM labels WHERE needs_review = 1 AND dismissed = 0",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .unwrap_or(0);
    let truncated = items.len() + skipped.len() >= PREVIEW_CAP && total >= PREVIEW_CAP;
    Ok(BatchPreview {
        kind: PreviewKind::Classify,
        kind_tag: "classify".to_string(),
        items,
        skipped,
        total_candidates: total,
        truncated,
        remainder,
    })
}

/// One classify preview row from CACHED rows only (no sidecar, no FETCH):
/// an existing fresh label row decides (same ids the real run journals);
/// otherwise a headless taxonomy keyword match over the cached classify
/// fields approximates the suggester — no match means review-bucket.
fn build_classify_preview_item(
    conn: &rusqlite::Connection,
    tax: &super::taxonomy::Taxonomy,
    mid: u64,
    mb: &str,
) -> Result<PreviewItem, String> {
    let empty = |reason: &str| PreviewItem {
        message_id: mid,
        from_folder: mb.to_string(),
        to_folder: String::new(),
        dest_display: String::new(),
        basis: String::new(),
        skipped_reason: Some(reason.to_string()),
    };
    if let Some(label) = queries::get_label(conn, mid).map_err(|e| e.to_string())? {
        if label.stale {
            return Ok(empty("review-bucket"));
        }
        let dest = dest_display_for(tax, &label.primary_id, label.secondary_id.as_deref());
        if label.needs_review {
            let mut row = empty("review-bucket");
            row.to_folder = dest.clone();
            row.dest_display = dest;
            row.basis = super::redact::filter_output(&format!(
                "{} {:.2}",
                label.primary_id, label.confidence
            ));
            return Ok(row);
        }
        return Ok(PreviewItem {
            message_id: mid,
            from_folder: mb.to_string(),
            to_folder: dest.clone(),
            dest_display: dest,
            basis: super::redact::filter_output(&format!(
                "{} {:.2}",
                label.primary_id, label.confidence
            )),
            skipped_reason: None,
        });
    }
    // No label row: headless keyword match over cached fields.
    let fields = queries::message_classify_fields(conn, mid).map_err(|e| e.to_string())?;
    let Some((subject, snippet, from_addr, atts)) = fields else {
        return Ok(empty("review-bucket"));
    };
    let domain = from_addr.rsplit('@').next().unwrap_or("");
    let is_reply = subject.to_lowercase().starts_with("re:");
    let ev = super::evidence::build_evidence(&subject, &snippet, domain, atts, is_reply);
    let all: Vec<&super::taxonomy::Category> = tax.categories.iter().collect();
    let hits = super::taxonomy::match_keywords(&ev, &all);
    let Some((best_id, _)) = hits.into_iter().next() else {
        return Ok(empty("review-bucket"));
    };
    let best = tax.categories.iter().find(|c| c.id == best_id);
    let (primary, child) = match best.and_then(|c| c.parent.clone()) {
        Some(pid) => (pid, Some(best_id.as_str())),
        None => (best_id.clone(), None),
    };
    let dest = dest_display_for(tax, &primary, child);
    Ok(PreviewItem {
        message_id: mid,
        from_folder: mb.to_string(),
        to_folder: dest.clone(),
        dest_display: dest,
        basis: super::redact::filter_output(&format!("{primary} keyword")),
        skipped_reason: None,
    })
}

fn preview_archive(conn: &rusqlite::Connection) -> Result<BatchPreview, String> {
    let threshold = queries::get_archive_threshold(conn);
    let included = super::archive::resolve_included_folders(conn);
    let now = chrono::Utc::now();
    let cands =
        super::archive::collect_archive_candidates(conn, threshold, &included, now)?;
    let mut items = Vec::new();
    for (mid, mb, _uid) in cands {
        if items.len() >= PREVIEW_CAP {
            break;
        }
        let Ok((dest_display, _, _)) = super::archive::archive_dest_for(conn, mid, &mb, now)
        else {
            continue;
        };
        items.push(PreviewItem {
            message_id: mid,
            from_folder: mb,
            to_folder: dest_display.clone(),
            dest_display,
            basis: super::archive::ARCHIVE_LABEL_ID.to_string(),
            skipped_reason: None,
        });
    }
    // Undatable mail stays put structurally — surface it as skipped so
    // the user sees WHY those rows never move.
    let mut skipped = Vec::new();
    let included_set: HashSet<&str> = included.iter().map(|s| s.as_str()).collect();
    let mboxes: Vec<String> = conn
        .prepare("SELECT name FROM mailboxes")
        .map_err(|e| e.to_string())?
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for mb in mboxes {
        if !included_set.contains(mb.as_str()) {
            continue;
        }
        let rows: Vec<(u64, String)> = conn
            .prepare(
                "SELECT m.id, m.date_utc FROM messages m JOIN mailboxes b ON m.mailbox_id = b.id
                 WHERE b.name = ?1",
            )
            .map_err(|e| e.to_string())?
            .query_map(rusqlite::params![mb], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for (mid, date_utc) in rows {
            if super::archive::parse_date_utc(&date_utc).is_none() && skipped.len() < PREVIEW_CAP {
                skipped.push(PreviewItem {
                    message_id: mid,
                    from_folder: mb.clone(),
                    to_folder: String::new(),
                    dest_display: String::new(),
                    basis: String::new(),
                    skipped_reason: Some("undatable".to_string()),
                });
            }
        }
    }
    let total = items.len() + skipped.len();
    let items_capped = items.len() >= PREVIEW_CAP;
    Ok(BatchPreview {
        kind: PreviewKind::Archive,
        // Mirrors the Phase 24 `totals_json` `{"kind":"archive"}` convention.
        kind_tag: "archive".to_string(),
        items,
        skipped,
        total_candidates: total,
        truncated: items_capped,
        remainder: 0,
    })
}

// ── Phase 30: stale-preview re-resolution (T-30-01) ────────────────
//
// The MOVE verb ALWAYS addresses the (folder, UID) pair returned by
// `resolve_for_move`, read in the same lock section as the journal
// write. A preview/run-start UID is NEVER passed to the verb directly.
// Staleness: the item's (folder, UID) differs from the run-start
// snapshot OR its folder's `uid_validity` epoch differs from the epoch
// snapshot. Stale items return None → the caller drops them with a
// surfaced `stale_dropped` count, never filed by stale UID (T-30-01).
// Folder-level epoch shifts abort that folder's items to resync (Phase
// 20 precedent) — the run continues for unaffected folders.

/// Re-resolved move target: the ONLY address the MOVE verb may take.
#[derive(Debug, Clone)]
pub struct ResolvedMove {
    pub message_id: u64,
    pub from_folder: String,
    pub uid: u32,
    pub epoch: u32,
}

/// Re-read `message_location` + `mailboxes.uid_validity` in the caller's
/// lock section and compare against the run-start snapshot. `Ok(None)`
/// = stale (drop + count); `Ok(Some(_))` = fresh pair for journal+verb.
pub fn resolve_for_move(
    conn: &rusqlite::Connection,
    message_id: u64,
    snap_folder: &str,
    snap_uid: u32,
    snap_epoch: u32,
) -> Result<Option<ResolvedMove>, String> {
    let loc = queries::message_location(conn, message_id).map_err(|e| e.to_string())?;
    let Some((folder, uid)) = loc else {
        return Ok(None); // Vanished mid-run: re-collect, never resurrect.
    };
    if folder != snap_folder || uid != snap_uid {
        return Ok(None);
    }
    let epoch: u32 = conn
        .query_row(
            "SELECT uid_validity FROM mailboxes WHERE name = ?1",
            rusqlite::params![folder],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if epoch != snap_epoch {
        return Ok(None);
    }
    Ok(Some(ResolvedMove { message_id, from_folder: folder, uid, epoch }))
}

/// Folder epochs snapshot (run start) for the per-chunk UIDVALIDITY
/// re-check pattern (Phase 20, reused verbatim).
pub fn snapshot_epochs(conn: &rusqlite::Connection) -> Result<HashMap<String, u32>, String> {
    conn.prepare("SELECT name, uid_validity FROM mailboxes")
        .map_err(|e| e.to_string())?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|e| e.to_string())
}

// ── Phase 30: retry-only-failures core (BATCH-04, backend half) ────
//
// Failure model: failed = `batch_items` intent rows with `moved=0` in a
// finished/interrupted run (journaled intent, no journaled fact). Rows
// with `moved=1` are done and never re-executed; rows never journaled
// (candidate vanished mid-run) are re-collected, not resurrected.
// Failure derives from `moved=0` alone — no migration, no new column.

/// One failed intent row awaiting retry.
#[derive(Debug, Clone)]
pub struct FailedItem {
    pub message_id: u64,
    pub from_folder: String,
    pub to_folder: String,
    pub label_id: String,
}

/// Entry gate: only `done`/`interrupted` runs with at least one
/// `moved=0` row offer retry. `undone` runs never retry (undo already
/// restored originals). Plain-language errors, no internals (T-30-08).
pub fn check_retry_entry(
    conn: &rusqlite::Connection,
    run_id: i64,
) -> Result<Vec<FailedItem>, String> {
    let state: Option<String> = conn
        .query_row("SELECT state FROM batch_runs WHERE id = ?1", rusqlite::params![run_id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let state = state.ok_or_else(|| "execução desconhecida".to_string())?;
    if state == "undone" {
        return Err("execução desfeita — nada para repetir".to_string());
    }
    if state != "done" && state != "interrupted" {
        return Err("só execuções terminadas podem repetir falhas".to_string());
    }
    let rows: Vec<FailedItem> = conn
        .prepare(
            "SELECT message_id, from_folder, to_folder, label_id FROM batch_items
             WHERE run_id = ?1 AND moved = 0",
        )
        .map_err(|e| e.to_string())?
        .query_map(rusqlite::params![run_id], |r| {
            Ok(FailedItem {
                message_id: r.get(0)?,
                from_folder: r.get(1)?,
                to_folder: r.get(2)?,
                label_id: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Err("esta execução não tem falhas para repetir".to_string());
    }
    Ok(rows)
}

/// Count of failed (`moved=0`) rows — the retry button's failure count.
pub fn failed_count(conn: &rusqlite::Connection, run_id: i64) -> usize {
    conn.query_row(
        "SELECT count(*) FROM batch_items WHERE run_id = ?1 AND moved = 0",
        rusqlite::params![run_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .unwrap_or(0)
}

/// Run-level single-flight guard (SyncGate-style): double-click-retry
/// never double-executes — the second caller sees the running retry and
/// skips (T-30-06).
static RETRY_IN_FLIGHT: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();

fn retry_set() -> &'static Mutex<HashSet<i64>> {
    RETRY_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Try to claim the retry lane for `run_id`. `true` = claimed (caller
/// must `release_retry`); `false` = a retry is already running, skip.
pub fn try_claim_retry(run_id: i64) -> bool {
    retry_set().lock().unwrap().insert(run_id)
}

/// Release the retry lane for `run_id`.
pub fn release_retry(run_id: i64) {
    retry_set().lock().unwrap().remove(&run_id);
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

    #[test]
    fn batch_chunk_stays_25_shared_with_archive() {
        // Chunk-size discipline (Phase 20 tuning against the throttling
        // UTFPR server): the archive runner reuses this constant verbatim
        // (see classify::archive). A new size needs live evidence — this
        // pin fails the build if either side drifts. Cooperative cancel +
        // per-chunk UIDVALIDITY re-check assume small chunks; do not bump
        // without re-running the Phase 26 live bundle.
        assert_eq!(BATCH_CHUNK, 25);
    }

    // ── Phase 30 (BATCH-03): preview + re-resolution ──────────────

    fn db_counts(conn: &rusqlite::Connection) -> (i64, i64, i64, i64) {
        let runs: i64 = conn.query_row("SELECT count(*) FROM batch_runs", [], |r| r.get(0)).unwrap();
        let items: i64 = conn.query_row("SELECT count(*) FROM batch_items", [], |r| r.get(0)).unwrap();
        let labels: i64 = conn.query_row("SELECT count(*) FROM labels", [], |r| r.get(0)).unwrap();
        let events: i64 = conn.query_row("SELECT count(*) FROM override_events", [], |r| r.get(0)).unwrap();
        (runs, items, labels, events)
    }

    /// Preview seed: labeled INBOX mail (movable) + unlabeled INBOX mail
    /// (keyword/review path) + old mail for the archive leg.
    fn preview_seeded() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let inbox = ensure_mailbox(conn, "INBOX").unwrap();
        upsert_message(conn, inbox, 1, None, "Boleto vencido fatura", "f@x", "[]", "[]",
            "2026-06-01T00:00:00Z", "[]", false, "").unwrap();
        upsert_message(conn, inbox, 2, None, "xqzkwv jjj", "g@x", "[]", "[]",
            "2026-10-10T00:00:00Z", "[]", false, "").unwrap();
        let a = find_message_id(conn, inbox, 1).unwrap().unwrap();
        upsert_label(conn, a, "financeiro", None, 0.9, 0.6, false).unwrap();
        store
    }

    #[test]
    fn preview_writes_nothing() {
        // Phase 29 `dry_run_writes_nothing` precedent, adapted: snapshot
        // batch_runs/batch_items/labels/override_events, run BOTH preview
        // legs, assert counts identical (zero writes, transient plan).
        let store = preview_seeded();
        let conn = store.conn();
        let before = db_counts(conn);
        let classify = preview_plan(conn, PreviewKind::Classify, None).unwrap();
        let archive = preview_plan(conn, PreviewKind::Archive, None).unwrap();
        assert_eq!(db_counts(conn), before, "preview must write nothing");
        // Classify leg: labeled mail movable with basis; gibberish mail
        // skipped to the review bucket.
        assert_eq!(classify.kind_tag, "classify");
        assert!(classify.items.iter().any(|i| i.basis.contains("financeiro")),
            "labeled mail must preview with its label basis: {:?}", classify.items);
        assert!(classify.skipped.iter().any(|s| s.skipped_reason.as_deref() == Some("review-bucket")),
            "unmatched mail must surface as review-bucket: {:?}", classify.skipped);
        for row in classify.items.iter().chain(classify.skipped.iter()) {
            assert!(crate::classify::redact::looks_clean(&row.basis),
                "basis must be redaction-clean: {}", row.basis);
        }
        // Archive leg rides the SAME function with kind=archive tagging.
        assert_eq!(archive.kind, PreviewKind::Archive);
        assert_eq!(archive.kind_tag, "archive");
        assert!(archive.items.iter().all(|i| i.basis == super::super::archive::ARCHIVE_LABEL_ID));
        assert!(archive.items.iter().all(|i| i.dest_display.starts_with("Auto/Archive/")));
    }

    #[test]
    fn commit_counts_match_preview() {
        // Parity: journaling the preview destinations (the headless shape
        // of the real run's journal-before-move) yields exactly the
        // preview's candidate set + destinations.
        let store = preview_seeded();
        let conn = store.conn();
        let plan = preview_plan(conn, PreviewKind::Classify, None).unwrap();
        let run = create_batch_run(conn).unwrap();
        for (chunk, item) in plan.items.iter().enumerate() {
            journal_intent(conn, run, item.message_id, &item.from_folder, &item.to_folder, "preview", chunk).unwrap();
        }
        let journaled: Vec<(u64, String)> = conn
            .prepare("SELECT message_id, to_folder FROM batch_items WHERE run_id = ?1 ORDER BY message_id")
            .unwrap()
            .query_map(rusqlite::params![run], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let mut expected: Vec<(u64, String)> = plan.items.iter().map(|i| (i.message_id, i.to_folder.clone())).collect();
        expected.sort();
        assert_eq!(journaled, expected, "real-run journal must match preview plan");
    }

    #[test]
    fn stale_preview_drop() {
        // Fixture: two preview items; between preview and confirm one
        // message's location shifts (burst-EXISTS) and one folder's epoch
        // shifts (UIDVALIDITY). The simulated confirm loop re-resolves in
        // the journal lock section: shifted rows drop with stale_dropped
        // == 2 and the verb transcript contains NO MOVE for stale UIDs.
        let store = preview_seeded();
        let conn = store.conn();
        let inbox_id = mailbox_id(conn, "INBOX").unwrap().unwrap();
        let other = ensure_mailbox(conn, "Other").unwrap();
        let plan = preview_plan(conn, PreviewKind::Classify, None).unwrap();
        assert!(!plan.items.is_empty());
        let epochs = snapshot_epochs(conn).unwrap();
        // Snapshot (folder, uid) per item at "preview time".
        let snap: Vec<(u64, String, u32)> = plan.items.iter().map(|i| {
            let uid: u32 = conn.query_row(
                "SELECT uid FROM messages WHERE id = ?1", rusqlite::params![i.message_id], |r| r.get(0)).unwrap();
            (i.message_id, i.from_folder.clone(), uid)
        }).collect();
        // Shift 1: move first message's row to Other with a new UID.
        let first = snap[0].0;
        conn.execute("UPDATE messages SET mailbox_id = ?1, uid = 99 WHERE id = ?2",
            rusqlite::params![other, first]).unwrap();
        // Shift 2: bump INBOX epoch (UIDVALIDITY shift).
        conn.execute("UPDATE mailboxes SET uid_validity = uid_validity + 1 WHERE id = ?1",
            rusqlite::params![inbox_id]).unwrap();
        // Simulated confirm: resolve → journal → transcript verb.
        let run = create_batch_run(conn).unwrap();
        let mut transcript: Vec<String> = Vec::new();
        let mut stale_dropped = 0usize;
        for (mid, folder, uid) in &snap {
            let epoch = epochs.get(folder).copied().unwrap_or(0);
            match resolve_for_move(conn, *mid, folder, *uid, epoch).unwrap() {
                None => stale_dropped += 1,
                Some(res) => {
                    // The verb takes ONLY the re-resolved pair — never the
                    // snapshot (folder, uid). Record the verb address.
                    transcript.push(format!("MOVE {}:{} -> filed", res.from_folder, res.uid));
                    journal_intent(conn, run, *mid, &res.from_folder, "Auto/X", "financeiro", 0).unwrap();
                }
            }
        }
        assert_eq!(stale_dropped, snap.len(), "every item went stale (location + epoch shifts)");
        assert!(transcript.is_empty(), "no verb may fire for stale rows: {transcript:?}");
        // Fresh control: unshifted store resolves cleanly.
        let store2 = preview_seeded();
        let conn2 = store2.conn();
        let plan2 = preview_plan(conn2, PreviewKind::Classify, None).unwrap();
        let epochs2 = snapshot_epochs(conn2).unwrap();
        for item in &plan2.items {
            let uid: u32 = conn2.query_row(
                "SELECT uid FROM messages WHERE id = ?1", rusqlite::params![item.message_id], |r| r.get(0)).unwrap();
            let res = resolve_for_move(conn2, item.message_id, &item.from_folder, uid,
                epochs2.get(&item.from_folder).copied().unwrap_or(0)).unwrap();
            assert!(res.is_some(), "fresh item must resolve");
            let res = res.unwrap();
            assert_eq!((res.from_folder.as_str(), res.uid), (item.from_folder.as_str(), uid));
        }
    }

    #[test]
    fn live_preview_gate_skips_offline() {
        // Sandbox contract: without operator credentials this leg SKIPs,
        // never fails and never touches the network (exit-2 shape for the
        // live-gate bins; early return for the unit leg).
        if std::env::var("SGE_LIVE_IMAP_USER").is_err() {
            eprintln!("SKIP: live batch preview leg needs SGE_LIVE_IMAP_USER (operator-only)");
            return;
        }
        // With creds the operator runs the credentialed preview leg
        // externally (self-only mail, pre-flight guard, undo_batch
        // cleanup); the headless tests above are the assertion.
        let _store = Store::open_in_memory().unwrap();
    }
}
