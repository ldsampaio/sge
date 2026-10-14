//! Auto-archive engine (Phase 24, Plan 24-01).
//!
//! Age-based retirement of old mail into `Auto/Archive/<YYYY>/<Top>` as
//! an explicit journaled batch (ARCH-01/02): settings persistence, an
//! age-based candidate selector over the local `date_utc` column, archive
//! destination tree-ensure over the proven `ensure_auto_tree` CREATE path,
//! and per-message MOVE through the proven `move_one_uid` + `imap_outbox`
//! replay verbs.
//!
//! Hard invariants (STRUCTURAL, test-asserted below — never UI copy):
//! - MOVE-only: this module touches no removal verb. The only IMAP verbs
//!   reachable from here are CREATE (tree-ensure) and MOVE (filing) plus
//!   the read-only LIST/SELECT/STATUS inside the manager lease.
//! - Manual trigger only: no timer, no mount hook, no background poll
//!   invokes the archive runner. Scheduling belongs to Phase 25+ rules.
//! - Age comes from the local `date_utc` column (the Date-header clock,
//!   same clock as the year bucket) — never a fresh FETCH. The archive
//!   path issues zero new FETCH/download code paths (BODY.PEEK-only).
//! - `A Classificar`, Sent, Drafts, and `Auto/Archive` itself (plus the
//!   whole archive subtree) are never sourced from — structurally
//!   excluded in the selector, not merely unchecked in the UI.
//! - UIDVALIDITY per-chunk re-check + UID-only addressing + cooperative
//!   cancel + journal-before-move reuse the Phase 20 batch machinery
//!   verbatim (single journal invariant: same `batch_runs` table, same
//!   `journal_intent` / `journal_moved` / `journaled_ids` rows, same
//!   reverse-order `undo_batch`). Archive runs ARE batch runs with an
//!   archive selector; `totals_json` carries `{"kind":"archive", ...}` so
//!   report/list surfaces can label them distinctly.
//!
//! Chunk sizing: BATCH_CHUNK=25 is reused unchanged (tuned against the
//! throttling UTFPR server in Phase 20 — no new size without live
//! evidence; pinned by `archive_reuses_batch_chunk_25` below).
//!
//! Sandbox live-run checklist (Phase 26 live bundle executes this; the
//! offline suite below is the CI gate):
//! - Scope every live archive probe to `SGE-Test-<run-id>` (dedicated
//!   folder per run); existing mail/folders never touched.
//! - Test mail to the operator's own address only; pre-flight guard
//!   aborts on out-of-sandbox targets (`scripts/live-*.sh` pattern).
//! - Post-run: `undo_batch` the probe run, then full cleanup of the
//!   sandbox folder; secret scan over the temp DB before cleanup.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use super::filing::{self, ConfirmResult};
use super::redact;
use super::taxonomy::{load_default, top_level};
use crate::imap::headers::MessageHeader;
use crate::commands::sync::{prepare_create_wire, refresh_mailbox_tree};
use crate::imap::manager::SessionManager;
use crate::store::queries;
use crate::store::Store;

/// Journal `label_id` for archive moves (the per-category report groups
/// by this id; the run KIND lives in `totals_json`, not here).
pub const ARCHIVE_LABEL_ID: &str = "archive";

/// Folders the archive selector never sources from, no matter what the
/// include list says (burying unreviewed mail would hide it from review;
/// Sent/Drafts mirrors the Phase 17 SIDE-03 seed).
const ALWAYS_EXCLUDED: [&str; 3] = ["A Classificar", "Sent", "Drafts"];

/// Default include scope: INBOX plus every filed `Auto/<Top>` top.
pub fn default_included_folders() -> Vec<String> {
    let mut out = vec!["INBOX".to_string()];
    if let Ok(tax) = load_default() {
        for top in top_level(&tax) {
            out.push(format!("Auto/{}", top.name));
        }
    }
    out
}

/// Effective include scope: stored JSON when valid, otherwise the INBOX +
/// Auto-tops default. Hardcoded exclusions are applied by the SELECTOR,
/// not here — a stored list naming an excluded folder stays inert.
pub fn resolve_included_folders(conn: &rusqlite::Connection) -> Vec<String> {
    match queries::get_archive_included_json(conn) {
        Some(json) => serde_json::from_str::<Vec<String>>(&json).unwrap_or_else(|_| default_included_folders()),
        None => default_included_folders(),
    }
}

/// True for the archive subtree in either hierarchy spelling
/// (`Auto/Archive/...` with `/` delimiter, `Auto.Archive...` with `.`).
pub fn is_archive_tree(name: &str) -> bool {
    if name == "Auto/Archive" || name == "Auto.Archive" {
        return true;
    }
    name.starts_with("Auto/Archive/") || name.starts_with("Auto.Archive.")
}

/// Structural exclusion: hardcoded always-excluded folders, the archive
/// subtree itself, the bare `Auto` root placeholder, plus the shared
/// `classify_excluded_folders` table (Sent/Drafts seed included).
fn is_source_excluded(conn: &rusqlite::Connection, mailbox: &str) -> bool {
    if ALWAYS_EXCLUDED.contains(&mailbox) {
        return true;
    }
    if is_archive_tree(mailbox) {
        return true;
    }
    if mailbox == "Auto" {
        return true;
    }
    queries::is_excluded(conn, mailbox).unwrap_or(false)
}

/// Parse a stored `date_utc` value. The sync layer writes RFC3339
/// (`2026-10-10T00:00:00Z`); unparseable values return None and the
/// message is NEVER archived (conservative: undatable mail stays put).
pub fn parse_date_utc(date_utc: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(date_utc.trim()) {
        return Some(dt.with_timezone(&Utc));
    }
    // Lenient fallback: date-only prefix `YYYY-MM-DD`.
    if date_utc.len() >= 10 {
        let day: String = date_utc.chars().take(10).collect();
        if let Ok(naive) = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d") {
            let dt = naive.and_hms_opt(0, 0, 0)?;
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }
    None
}

/// Whole days between `date_utc` and `now`. None when undatable.
pub fn message_age_days(date_utc: &str, now: DateTime<Utc>) -> Option<i64> {
    parse_date_utc(date_utc).map(|then| (now - then).num_days())
}

/// Year bucket from the message Date clock (same clock as the age rule).
/// Falls back to `now`'s year only when the stored date is undatable —
/// which the selector never passes through (undatable mail is skipped).
pub fn archive_year(date_utc: &str, now: DateTime<Utc>) -> i32 {
    let dt = parse_date_utc(date_utc).unwrap_or(now);
    dt.format("%Y").to_string().parse::<i32>().unwrap_or(1970)
}

/// Archive top for one message: filed mail keeps its folder context (the
/// `Auto/<Top>` segment, decoded from wire form); unfiled mail keeps its
/// classified top name when a label row exists; otherwise the source
/// folder leaf (e.g. `INBOX`). Category IDs never appear in paths — the
/// destinations are taxonomy-version-independent by construction (a
/// rename/bump changes future destinations, never invalidates old ones).
pub fn archive_top_for(
    conn: &rusqlite::Connection,
    message_id: u64,
    mailbox: &str,
) -> Result<String, String> {
    // Filed mail: folder segment is the category context (folder truth
    // wins over possibly-stale label rows after a taxonomy rename).
    let stripped = mailbox
        .strip_prefix("Auto/")
        .or_else(|| mailbox.strip_prefix("Auto."));
    if let Some(rest) = stripped {
        let top_wire = rest.split(['/', '.']).next().unwrap_or(rest);
        let top = crate::imap::mutf7::decode_modified_utf7(top_wire);
        if !top.is_empty() && top != "Archive" {
            return Ok(top);
        }
    }
    // Unfiled mail with a label: resolved top DISPLAY name (not the id).
    if let Ok(Some(label)) = queries::get_label(conn, message_id) {
        if let Ok(tax) = load_default() {
            if let Some(top) = tax.categories.iter().find(|c| c.id == label.primary_id && c.parent.is_none()) {
                return Ok(top.name.clone());
            }
            // Child id: resolve through its parent top.
            if let Some(cat) = tax.categories.iter().find(|c| c.id == label.primary_id) {
                if let Some(pid) = cat.parent.as_deref() {
                    if let Some(top) = tax.categories.iter().find(|c| c.id == pid) {
                        return Ok(top.name.clone());
                    }
                }
            }
        }
    }
    // Fallback: source folder leaf (INBOX and friends).
    let leaf = mailbox.rsplit(['/', '.']).next().unwrap_or(mailbox);
    Ok(crate::imap::mutf7::decode_modified_utf7(leaf))
}

/// `(display destination, year, top)` for one message. Display form always
/// uses `/`; the runner converts to wire form per the account delimiter.
pub fn archive_dest_for(
    conn: &rusqlite::Connection,
    message_id: u64,
    mailbox: &str,
    now: DateTime<Utc>,
) -> Result<(String, i32, String), String> {
    let date_utc: String = conn
        .query_row(
            "SELECT date_utc FROM messages WHERE id = ?1",
            rusqlite::params![message_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let year = archive_year(&date_utc, now);
    let top = archive_top_for(conn, message_id, mailbox)?;
    Ok((format!("Auto/Archive/{year}/{top}"), year, top))
}

/// Archive candidate rows: (message_id, mailbox, uid) for mail in the
/// include scope whose local `date_utc` age is >= `threshold_days`.
/// Reads the `date_utc` column only — never a FETCH, never a body
/// download. Undatable mail is skipped (stays put).
pub fn collect_archive_candidates(
    conn: &rusqlite::Connection,
    threshold_days: i64,
    included_folders: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<(u64, String, u32)>, String> {
    if included_folders.is_empty() {
        return Ok(vec![]);
    }
    let included: HashSet<&str> = included_folders.iter().map(|s| s.as_str()).collect();
    let mailboxes: Vec<String> = conn
        .prepare("SELECT name FROM mailboxes")
        .map_err(|e| e.to_string())?
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for mb in mailboxes {
        // Scope entries may be display names (`Auto/Acadêmico`) while the
        // disk holds wire names (`Auto/Acad&-mico`): match either form so
        // non-ASCII tops are actually archivable.
        let display = crate::imap::mutf7::decode_modified_utf7(&mb);
        if !included.contains(mb.as_str()) && !included.contains(display.as_str()) {
            continue;
        }
        if is_source_excluded(conn, &mb) {
            continue;
        }
        let rows: Vec<(u64, u32, String)> = conn
            .prepare(
                "SELECT m.id, m.uid, m.date_utc FROM messages m JOIN mailboxes mb ON m.mailbox_id = mb.id
                 WHERE mb.name = ?1",
            )
            .map_err(|e| e.to_string())?
            .query_map(rusqlite::params![mb], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for (mid, uid, date_utc) in rows {
            match message_age_days(&date_utc, now) {
                Some(age) if age >= threshold_days => out.push((mid, mb.clone(), uid)),
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Display (`Auto/Archive/2026/Top`) → wire form for the account
/// delimiter, encoding non-ASCII leaves exactly like `batch_classify`
/// does (same expression, one implementation would mean touching the
/// command — duplicated deliberately, one line each).
pub fn display_to_wire(display: &str, delimiter: &str) -> String {
    if delimiter.is_empty() || delimiter == "/" {
        return display.to_string();
    }
    display
        .split('/')
        .map(|p| crate::imap::mutf7::encode_modified_utf7(p))
        .collect::<Vec<_>>()
        .join(delimiter)
}

/// Redacted one-line detail for the move ("Arquivado em ..." passed
/// through the Phase 16 sanitizer: labels store pointers, never content,
/// and no secret survives in DB/logs).
pub fn archive_detail(dest_display: &str) -> String {
    redact::filter_output(&format!("Arquivado em {dest_display}"))
}

/// Pre-create the archive subtree for every destination display path
/// (`Auto → Archive → YYYY → Top`), level by level over the existing
/// `prepare_create_wire` (leaf validation + mutf7 + exists pre-check) +
/// `create_mailbox_in` path with the `auto_root` marker — zero new CREATE
/// logic. Dialog-off like `batch::ensure_full_tree`: a user-owned `Auto`
/// without our marker records the marker and nests (the Phase 18
/// confirm-then-nest dialog has no mid-run form; the run report + undo
/// are the backstop). Returns folders created.
pub async fn ensure_archive_tree(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    dests: &[String],
) -> Result<usize, String> {
    let (cached, delimiter, marked) = {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        let cached = queries::list_mailboxes(conn).map_err(|e| e.to_string())?;
        let raw = cached.first().map(|r| r.delimiter.clone()).unwrap_or_default();
        // Pre-first-sync stores carry an empty delimiter: joining with ""
        // would glue parent+leaf together ("AutoArchive"). The display
        // form uses "/", so normalize to it (same assumption the batch
        // runner's inline wire computation makes).
        let delimiter = if raw.is_empty() { "/".to_string() } else { raw };
        let marked =
            queries::get_setting(conn, "auto_root").map_err(|e| e.to_string())?.as_deref() == Some("ours");
        (cached, delimiter, marked)
    };
    let live = manager.list_mailboxes().await.map_err(|e| e.to_string())?;
    let mut known: Vec<String> = live.into_iter().map(|m| m.name).collect();
    for row in &cached {
        if !known.contains(&row.name) {
            known.push(row.name.clone());
        }
    }
    let root_wire = display_to_wire("Auto", &delimiter);
    if known.iter().any(|n| n == &root_wire || n == "Auto") && !marked {
        // Dialog-off batch precedent: record the marker and nest.
        let guard = store.lock().unwrap();
        queries::set_setting(guard.conn(), "auto_root", "ours").map_err(|e| e.to_string())?;
    }
    // Ordered levels, parents before children, deduped.
    let mut levels: Vec<String> = Vec::new();
    for dest in dests {
        let parts: Vec<&str> = dest.split('/').collect();
        for i in 1..=parts.len() {
            let level = parts[..i].join("/");
            if !levels.contains(&level) {
                levels.push(level);
            }
        }
    }
    let mut created = 0usize;
    for level in levels {
        let wire = display_to_wire(&level, &delimiter);
        if known.contains(&wire) {
            continue;
        }
        let (parent_display, leaf) = match level.rfind('/') {
            Some(idx) => (Some(level[..idx].to_string()), level[idx + 1..].to_string()),
            None => (None, level.clone()),
        };
        let parent_wire = parent_display.as_deref().map(|p| display_to_wire(p, &delimiter));
        let cached_now = {
            let guard = store.lock().unwrap();
            queries::list_mailboxes(guard.conn()).map_err(|e| e.to_string())?
        };
        match prepare_create_wire(&cached_now, parent_wire.as_deref(), &leaf, &delimiter) {
            Ok(wire_now) => {
                manager.create_mailbox_in(&wire_now).await.map_err(|e| e.to_string())?;
                known.push(wire_now);
                created += 1;
            }
            Err(e) if e == "Já existe uma pasta com esse nome." => {
                known.push(wire);
            }
            Err(e) => return Err(e),
        }
    }
    let _ = refresh_mailbox_tree(manager, store).await.map_err(|e| e.to_string());
    {
        let guard = store.lock().unwrap();
        let _ = queries::set_setting(guard.conn(), "auto_root", "ours");
    }
    Ok(created)
}

/// One archive MOVE over the proven filing path (optimistic-local +
/// durable `imap_outbox` + immediate verb, ack dequeues, failure stays
/// queued for reconnect replay — mirrors `confirm_suggestion` exactly).
/// Journaling stays with the caller (intent BEFORE, moved AFTER).
pub async fn archive_one(
    manager: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    mailbox: &str,
    uid: u32,
    dest_wire: &str,
    dest_display: &str,
) -> Result<ConfirmResult, filing::FilingError> {
    if mailbox == dest_wire {
        return Err(filing::FilingError::AlreadyThere);
    }
    filing::move_one_uid(manager, store, mailbox, uid, dest_wire, &archive_detail(dest_display)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::batch::{journal_intent, journal_moved, journaled_ids};
    use crate::store::queries::*;
    use crate::store::Store;

    /// Fixed clock: 2026-10-13T00:00:00Z (deterministic age math).
    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-13T00:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn dt_days_ago(days: i64) -> String {
        (fixed_now() - chrono::Duration::days(days)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    /// Seed: INBOX (old 100d + young 10d), Sent old, `A Classificar` old,
    /// `Auto/Archive/2026/INBOX` old, `Auto/Financeiro` old.
    fn seeded() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let inbox = ensure_mailbox(conn, "INBOX").unwrap();
        let sent = ensure_mailbox(conn, "Sent").unwrap();
        let review = ensure_mailbox(conn, "A Classificar").unwrap();
        let archive = ensure_mailbox(conn, "Auto/Archive/2026/INBOX").unwrap();
        let filed = ensure_mailbox(conn, "Auto/Financeiro").unwrap();
        upsert_message(conn, inbox, 1, None, "Boleto velho", "f@x", "[]", "[]",
            &dt_days_ago(100), "[]", false, "").unwrap();
        upsert_message(conn, inbox, 2, None, "Oi novo", "g@x", "[]", "[]",
            &dt_days_ago(10), "[]", false, "").unwrap();
        upsert_message(conn, sent, 1, None, "Enviado velho", "m@x", "[]", "[]",
            &dt_days_ago(200), "[]", false, "").unwrap();
        upsert_message(conn, review, 1, None, "Pendente velho", "r@x", "[]", "[]",
            &dt_days_ago(200), "[]", false, "").unwrap();
        upsert_message(conn, archive, 1, None, "Já arquivado", "a@x", "[]", "[]",
            &dt_days_ago(400), "[]", false, "").unwrap();
        upsert_message(conn, filed, 5, None, "Fatura velha", "fin@x", "[]", "[]",
            &dt_days_ago(120), "[]", false, "").unwrap();
        exclude_folder(conn, "Sent", "test").unwrap();
        store
    }

    #[test]
    fn settings_round_trip_with_backend_validation() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        // Defaults: 90d + INBOX + Auto tops.
        assert_eq!(get_archive_threshold(conn), 90);
        let defaults = default_included_folders();
        assert!(defaults.contains(&"INBOX".to_string()));
        assert!(defaults.iter().any(|f| f.starts_with("Auto/")));
        // Persist + read back.
        set_archive_threshold(conn, 30).unwrap();
        assert_eq!(get_archive_threshold(conn), 30);
        set_archive_included_json(conn, r#"["INBOX"]"#).unwrap();
        assert_eq!(resolve_included_folders(conn), vec!["INBOX".to_string()]);
        // Backend rejects out-of-range with plain language (UI never the
        // only guard): 6 and 731 fail, boundaries hold.
        let err = set_archive_threshold(conn, 6).unwrap_err();
        assert!(err.contains("7 e 730"), "plain-language range error, got: {err}");
        set_archive_threshold(conn, 731).unwrap_err();
        set_archive_threshold(conn, 7).unwrap();
        set_archive_threshold(conn, 730).unwrap();
        assert!(set_archive_included_json(conn, "[]").is_err());
        assert!(set_archive_included_json(conn, "não-json").is_err());
        // Corrupt stored values fall back to safe defaults (never panic).
        set_setting(conn, "archive_threshold_days", "abc").unwrap();
        assert_eq!(get_archive_threshold(conn), 90);
        set_setting(conn, "archive_included_folders", "{bad").unwrap();
        assert_eq!(resolve_included_folders(conn), default_included_folders());
    }

    #[test]
    fn selector_moves_old_only_from_included_scope() {
        let store = seeded();
        let conn = store.conn();
        let now = fixed_now();
        let scope = vec!["INBOX".to_string(), "Auto/Financeiro".to_string()];
        let cands = collect_archive_candidates(conn, 90, &scope, now).unwrap();
        let mut got: Vec<(String, u32)> = cands.into_iter().map(|(_, mb, uid)| (mb, uid)).collect();
        got.sort();
        // Old INBOX + old filed mail; young INBOX stays.
        assert_eq!(got, vec![("Auto/Financeiro".to_string(), 5), ("INBOX".to_string(), 1)]);
        // Narrower threshold admits the young mail too.
        let cands7 = collect_archive_candidates(conn, 7, &scope, now).unwrap();
        assert_eq!(cands7.len(), 3);
        // Empty scope is a no-op (never a silent whole-account op).
        assert!(collect_archive_candidates(conn, 7, &[], now).unwrap().is_empty());
    }

    #[test]
    fn selector_never_sources_excluded_folders() {
        let store = seeded();
        let conn = store.conn();
        let now = fixed_now();
        // Even explicitly included, the hardcoded set + archive subtree
        // stay structurally unsourceable.
        let scope = vec![
            "INBOX".to_string(),
            "Sent".to_string(),
            "A Classificar".to_string(),
            "Auto/Archive/2026/INBOX".to_string(),
            "Auto/Financeiro".to_string(),
        ];
        let cands = collect_archive_candidates(conn, 90, &scope, now).unwrap();
        for (_, mb, _) in &cands {
            assert!(!ALWAYS_EXCLUDED.contains(&mb.as_str()), "sourced excluded {mb}");
            assert!(!is_archive_tree(mb), "sourced archive subtree {mb}");
        }
        // Undatable mail stays put.
        let inbox = mailbox_id(conn, "INBOX").unwrap().unwrap();
        upsert_message(conn, inbox, 9, None, "Sem data", "z@x", "[]", "[]",
            "not-a-date", "[]", false, "").unwrap();
        let cands2 = collect_archive_candidates(conn, 7, &["INBOX".to_string()], now).unwrap();
        assert!(cands2.iter().all(|(_, _, uid)| *uid != 9));
    }

    #[test]
    fn destination_shape_year_top_taxonomy_independent() {
        let store = seeded();
        let conn = store.conn();
        let now = fixed_now();
        // Filed mail: folder context preserved under the Date year.
        let filed_mid = find_message_id(conn, mailbox_id(conn, "Auto/Financeiro").unwrap().unwrap(), 5).unwrap().unwrap();
        let (dest, year, top) = archive_dest_for(conn, filed_mid, "Auto/Financeiro", now).unwrap();
        assert_eq!((year, top.as_str()), (2026, "Financeiro"));
        assert_eq!(dest, "Auto/Archive/2026/Financeiro");
        // Unfiled labeled mail: classified TOP name (never the id).
        let inbox_mid = find_message_id(conn, mailbox_id(conn, "INBOX").unwrap().unwrap(), 1).unwrap().unwrap();
        upsert_label(conn, inbox_mid, "financeiro.cobrancas", None, 0.9, 0.6, false).unwrap();
        let (dest2, _, _) = archive_dest_for(conn, inbox_mid, "INBOX", now).unwrap();
        assert_eq!(dest2, "Auto/Archive/2026/Financeiro");
        assert!(!dest2.contains("financeiro.cobrancas"), "no category-ID references in paths");
        // Unlabeled INBOX mail: source leaf fallback.
        let inbox_mid2 = find_message_id(conn, mailbox_id(conn, "INBOX").unwrap().unwrap(), 2).unwrap().unwrap();
        let (dest3, _, _) = archive_dest_for(conn, inbox_mid2, "INBOX", now).unwrap();
        assert_eq!(dest3, "Auto/Archive/2026/INBOX");
    }

    #[test]
    fn archive_detail_is_redacted() {
        // Destination echo carries no secrets and survives the sanitizer
        // byte-identical; a secret-bearing string does not.
        let clean = archive_detail("Auto/Archive/2026/Financeiro");
        assert_eq!(clean, "Arquivado em Auto/Archive/2026/Financeiro");
        assert!(redact::looks_clean(&clean));
        let dirty = redact::filter_output("Arquivado senha 482917 terminal 033");
        assert!(!dirty.contains("482917"), "digit runs must not survive: {dirty}");
    }

    #[test]
    fn archive_reuses_batch_chunk_25() {
        // Chunk-size discipline is shared with the batch runner (tuned
        // against the throttling UTFPR server in Phase 20): archive
        // introduces no new size — this pin fails the build if either
        // side drifts.
        assert_eq!(super::super::batch::BATCH_CHUNK, 25);
    }

    #[test]
    fn archive_path_imports_no_removal_verb() {
        // STRUCTURAL never-remove: the non-test half of this file must
        // contain no removal verb (mirrors the Phase 17 zero-new-verbs /
        // Phase 18 zero-IMAP-traffic assertion style). Test-only doubles
        // live below the split marker and don't count.
        const SELF_SRC: &str = include_str!("archive.rs");
        let code = SELF_SRC.split("#[cfg(test)]").next().unwrap_or("");
        for verb in ["EXPUNGE", "expunge", "STORE \\Deleted", "STORE Deleted", "delete_mailbox", "uid_expunge"] {
            assert!(
                !code.contains(verb),
                "removal verb `{verb}` on the archive path — MOVE-only invariant violated"
            );
        }
        // Positive control: the allowed verbs ARE present.
        for allowed in ["move_one_uid", "create_mailbox_in", "journal_intent", "journal_moved"] {
            assert!(code.contains(allowed), "expected allowed verb `{allowed}` missing");
        }
    }

    // ── Tracer e2e: one archive MOVE over a fake session ──────────
    // Minimal SyncSession double (MockSession-style): records CREATE +
    // MOVE legs, answers LIST/SELECT/capabilities. Removal-verb methods
    // panic when called — the e2e proves MOVE-only by construction.

    use crate::imap::manager::SessionManager;
    use crate::imap::{MailboxInfo, MailboxStatus, MailboxSummary, SyncError};
    use crate::imap::{PinBox, SyncSession};

    struct ArchiveFake {
        mailboxes: std::sync::Mutex<Vec<String>>,
        created: std::sync::Mutex<Vec<String>>,
        moved: std::sync::Mutex<Vec<(String, String)>>,
        removal_calls: std::sync::Mutex<Vec<String>>,
    }

    impl ArchiveFake {
        fn with_mailboxes(names: &[&str]) -> (Arc<SessionManager>, Arc<ArchiveFake>) {
            let fake = Arc::new(ArchiveFake {
                mailboxes: std::sync::Mutex::new(names.iter().map(|s| s.to_string()).collect()),
                created: std::sync::Mutex::new(Vec::new()),
                moved: std::sync::Mutex::new(Vec::new()),
                removal_calls: std::sync::Mutex::new(Vec::new()),
            });
            let session: Box<dyn SyncSession> = Box::new(ArchiveFakeHandle(fake.clone()));
            let manager = Arc::new(SessionManager::for_test_session(session));
            (manager, fake)
        }
    }

    struct ArchiveFakeHandle(Arc<ArchiveFake>);

    impl SyncSession for ArchiveFakeHandle {
        fn select_mailbox(&mut self, name: &str) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
            let name = name.to_string();
            Box::pin(async move {
                Ok(MailboxSummary { selected_mailbox: name, uid_validity: 7, exists: 1, uid_next: None })
            })
        }
        fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            Box::pin(async move { Ok(vec![1]) })
        }
        fn fetch_envelopes<'a>(&'a mut self, _range: &'a str)
            -> PinBox<'a, Result<Vec<crate::imap::headers::MessageHeader>, SyncError>> {
            Box::pin(async move { Ok(vec![]) })
        }
        fn fetch_body(&mut self, _uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
            Box::pin(async move { Ok(vec![]) })
        }
        fn set_seen(&mut self, _uid: u32, _seen: bool) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }
        fn store_deleted(&mut self, uid: u32, _deleted: bool) -> PinBox<'_, Result<(), SyncError>> {
            let msg = format!("archive fake: store_deleted called for uid {uid}");
            Box::pin(async move { Err(SyncError::Protocol(msg)) })
        }
        fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            Box::pin(async move { Err(SyncError::Protocol("archive fake: expunge called — MOVE-only violated".to_string())) })
        }
        fn uid_expunge(&mut self, _set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            Box::pin(async move { Err(SyncError::Protocol("archive fake: uid_expunge called — MOVE-only violated".to_string())) })
        }
        fn uid_copy_to(&mut self, _set: &str, _dest: &str) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }
        fn uid_move_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
            let this = self.0.clone();
            let (set, dst) = (uid_set.to_string(), dest.to_string());
            Box::pin(async move {
                this.moved.lock().unwrap().push((set, dst));
                Ok(())
            })
        }
        fn capabilities(&mut self) -> PinBox<'_, Result<Vec<String>, SyncError>> {
            Box::pin(async move { Ok(vec!["IMAP4rev1".to_string(), "MOVE".to_string(), "UIDPLUS".to_string()]) })
        }
        fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
            let this = self.0.clone();
            let name = name.to_string();
            Box::pin(async move {
                this.created.lock().unwrap().push(name.clone());
                this.mailboxes.lock().unwrap().push(name);
                Ok(())
            })
        }
        fn rename_mailbox(&mut self, _old: &str, _new: &str) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }
        fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
            let this = self.0.clone();
            let name = name.to_string();
            Box::pin(async move {
                this.removal_calls.lock().unwrap().push(format!("delete_mailbox:{name}"));
                Err(SyncError::Protocol("archive fake: delete_mailbox called — MOVE-only violated".to_string()))
            })
        }
        fn append_message(&mut self, _mb: &str, _flags: &str, _bytes: &[u8]) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }
        fn uid_search_header(&mut self, _f: &str, _v: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            Box::pin(async move { Ok(vec![]) })
        }
        fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
            let this = self.0.clone();
            Box::pin(async move {
                Ok(this.mailboxes.lock().unwrap().iter().map(|n| MailboxInfo {
                    name: n.clone(),
                    display_name: n.clone(),
                    delimiter: "/".to_string(),
                    attributes: vec![],
                }).collect())
            })
        }
        fn mailbox_status(&mut self, name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>> {
            let name = name.to_string();
            let _ = name;
            Box::pin(async move {
                Ok(MailboxStatus { uid_validity: 7, uid_next: Some(2), unseen: 0, messages: 1 })
            })
        }
        fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }

        fn select_condstore(&mut self) -> PinBox<'_, Result<bool, SyncError>> {
            Box::pin(async move { Ok(false) })
        }

        fn condstore_fetch_changedsince<'a>(
            &'a mut self,
            _range: &'a str,
            _modseq: u64,
        ) -> PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
            Box::pin(async move { Ok(Vec::new()) })
        }
    }

    #[test]
    fn tracer_old_mail_archives_young_stays() {        async_std::task::block_on(async {
            let store = seeded();
            let store = Arc::new(Mutex::new(store));
            let (manager, fake) = ArchiveFake::with_mailboxes(&["INBOX", "Sent", "A Classificar", "Auto/Financeiro"]);
            let now = fixed_now();
            // 1. Candidates: old INBOX mail only.
            let cands = {
                let guard = store.lock().unwrap();
                collect_archive_candidates(guard.conn(), 90, &["INBOX".to_string()], now).unwrap()
            };
            assert_eq!(cands.len(), 1);
            let (mid, mailbox, uid) = cands[0].clone();
            // 2. Destination + tree pre-create over the fake session.
            let (dest_display, year, _) = {
                let guard = store.lock().unwrap();
                archive_dest_for(guard.conn(), mid, &mailbox, now).unwrap()
            };
            assert_eq!(dest_display, format!("Auto/Archive/{year}/INBOX"));
            let made = ensure_archive_tree(&manager, &store, &[dest_display.clone()]).await.unwrap();
            assert!(made >= 3, "Auto + Archive + year levels created, got {made}");
            assert!(fake.created.lock().unwrap().iter().any(|n| n == "Auto/Archive"));
            // 3. Journal intent → MOVE → journal moved (verbatim batch order).
            let run = {
                let guard = store.lock().unwrap();
                create_batch_run(guard.conn()).unwrap()
            };
            {
                let guard = store.lock().unwrap();
                journal_intent(guard.conn(), run, mid, &mailbox, &dest_display, ARCHIVE_LABEL_ID, 0).unwrap();
            }
            let wire = display_to_wire(&dest_display, "/");
            archive_one(&manager, &store, &mailbox, uid, &wire, &dest_display).await.unwrap();
            {
                let guard = store.lock().unwrap();
                journal_moved(guard.conn(), run, mid).unwrap();
                assert_eq!(journaled_ids(guard.conn(), run).unwrap(), vec![mid]);
            }
            // 4. MOVE-only proof: exactly one UID MOVE leg to the archive
            // wire, zero removal verbs, young mail untouched.
            let moved = fake.moved.lock().unwrap();
            assert_eq!(moved.len(), 1);
            assert_eq!(moved[0].1, wire);
            assert!(fake.removal_calls.lock().unwrap().is_empty());
            let guard = store.lock().unwrap();
            let young = find_message_id(guard.conn(), mailbox_id(guard.conn(), "INBOX").unwrap().unwrap(), 2).unwrap().unwrap();
            assert!(journaled_ids(guard.conn(), run).unwrap().iter().all(|id| *id != young));
        });
    }

    #[test]
    fn shared_undo_reverses_archive_runs_and_kind_tag_labels() {
        async_std::task::block_on(async {
            // Journal an archive run by hand (same rows archive_run
            // writes): one moved row labeled "archive".
            let store = Store::open_in_memory().unwrap();
            let conn = store.conn();
            let inbox = ensure_mailbox(conn, "INBOX").unwrap();
            let dest = ensure_mailbox(conn, "Auto/Archive/2026/INBOX").unwrap();
            upsert_message(conn, dest, 1, None, "Arquivado", "a@x", "[]", "[]",
                &dt_days_ago(100), "[]", false, "").unwrap();
            let mid = find_message_id(conn, dest, 1).unwrap().unwrap();
            let run = create_batch_run(conn).unwrap();
            journal_intent(conn, run, mid, "INBOX", "Auto/Archive/2026/INBOX", ARCHIVE_LABEL_ID, 0).unwrap();
            journal_moved(conn, run, mid).unwrap();
            // Resume contract: a second pass skips the journaled row.
            let remaining: Vec<u64> = vec![mid, 999]
                .into_iter()
                .filter(|m| !journaled_ids(conn, run).unwrap().contains(m))
                .collect();
            assert_eq!(remaining, vec![999]);
            // The command finishes with {"kind":"archive",...}: the report
            // contract — backend truth the UI parses for labeling.
            let totals = serde_json::json!({"kind": "archive", "threshold_days": 90, "done": 1, "moved": 1}).to_string();
            finish_batch_run(conn, run, "done", &totals).unwrap();
            let stored: String = conn.query_row(
                "SELECT totals_json FROM batch_runs WHERE id = ?1",
                rusqlite::params![run], |r| r.get(0)).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
            assert_eq!(parsed["kind"], serde_json::Value::String("archive".to_string()));
            assert_eq!(parsed["threshold_days"], serde_json::Value::Number(90.into()));
            // Shared undo_batch reverses the archive move (reverse
            // to→from order), marks the run undone. The report survives
            // in the Phase 20 sense: per-category rows (grouped by the
            // "archive" label id) + undone count stay queryable — the UI
            // labels archive runs by totals kind pre-undo, by the
            // per-category "archive" id post-undo. undo_run itself is
            // reused VERBATIM (its "{}" totals overwrite is shared
            // Phase-20 behavior, not archive-specific).
            let store = Arc::new(Mutex::new(store));
            let (manager, fake) = ArchiveFake::with_mailboxes(&["INBOX", "Auto/Archive/2026/INBOX"]);
            let restored = super::super::batch::undo_run(&manager, &store, run).await.unwrap();
            assert_eq!(restored, 1);
            let moved = fake.moved.lock().unwrap();
            assert_eq!(moved.len(), 1);
            assert_eq!(moved[0].1, "INBOX");
            assert!(fake.removal_calls.lock().unwrap().is_empty());
            let guard = store.lock().unwrap();
            let state: String = guard.conn().query_row(
                "SELECT state FROM batch_runs WHERE id = ?1",
                rusqlite::params![run], |r| r.get(0)).unwrap();
            assert_eq!(state, "undone");
            // Report survives: per-category still groups the moved row
            // under the "archive" label id; undone count is 1.
            let per_cat: Vec<(String, i64)> = guard.conn().prepare(
                "SELECT label_id, count(*) FROM batch_items WHERE run_id = ?1 AND moved = 1 GROUP BY label_id")
                .unwrap().query_map(rusqlite::params![run], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(per_cat, vec![("archive".to_string(), 1)]);
            let undone: i64 = guard.conn().query_row(
                "SELECT count(*) FROM batch_items WHERE run_id = ?1 AND undone = 1",
                rusqlite::params![run], |r| r.get(0)).unwrap();
            assert_eq!(undone, 1);
            let _ = inbox;
        });
    }
}
