//! Rule import/export with versioned JSON schema (Phase 29, IMP-01).
//!
//! Versioned portability envelope over the Phase-25 `folder_rules` rows:
//! file → validation → store → file, with a round-trip fixpoint guarantee
//! (export → import → export is byte-identical — `rule_version` bumps stay
//! server-side and never leak into the envelope).
//!
//! Schema note: rule ids are the Phase-25 slug ids (`String`, e.g.
//! `boletos-2`), NOT UUIDs — the envelope carries the real row shape
//! (`conditions_json` / `scope_json` as JSON values, `dest_kind` as
//! `folder|category`). Timestamps stay server-side (`created_at` feeds the
//! evaluation order but is not portable state).
//!
//! Validation contract (CONTEXT D-02):
//! - ERRORS (item rejected): empty id, empty name, reserved names
//!   (`system`/`auto`, case-insensitive), empty conditions, bad regex,
//!   priority outside 1..=999, `dest_kind` outside the enum, empty or
//!   destructive destination, duplicate names (within the file or against
//!   a different stored id), name injection (`/`, control chars).
//! - WARNINGS (item imported anyway): scope folders or folder destinations
//!   absent from the cached folder tree, unknown taxonomy category ids.
//!
//! Security: every value reaches SQLite through bound parameters only
//! (via `queries::*`); conditions are data, never SQL (T-29-01). Export
//! carries rule metadata only — no message content, no credentials
//! (T-29-03). No new dependencies (T-29-SC).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::classify::rules::{
    self, RuleConditions, RULE_NAME_MAX, RULE_REGEX_MAX,
};
use crate::store::queries::{self, RuleRow};
use crate::store::Store;

/// Current envelope version. Imports reject any other version loudly.
pub const EXPORT_VERSION: u32 = 1;

/// Priority bounds enforced on import (CONTEXT D-02).
pub const PRIORITY_MIN: i64 = 1;
pub const PRIORITY_MAX: i64 = 999;

/// Cap on the raw import payload: rules are metadata-only, so 1 MiB is
/// generous (WR-02). Checked before parsing, before any lock is taken.
pub const MAX_IMPORT_BYTES: usize = 1_048_576;

/// Cap on the rule count per file (WR-02). Bounds validation + write work
/// before the store lock is ever acquired.
pub const MAX_IMPORT_RULES: usize = 500;

/// Reserved rule names (case-insensitive) — rejected on import.
const RESERVED_NAMES: [&str; 2] = ["system", "auto"];

/// One portable rule. Field types mirror the `folder_rules` row with the
/// JSON TEXT columns carried as parsed values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleExportItem {
    pub id: String,
    pub name: String,
    pub priority: i64,
    pub conditions_json: Value,
    pub dest_kind: String,
    pub destination: String,
    pub scope_json: Value,
    pub enabled: bool,
}

/// Versioned envelope: `{ version, rules }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleExport {
    pub version: u32,
    pub rules: Vec<RuleExportItem>,
}

/// Import outcome with per-item reporting (dry-run and commit share it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportResult {
    pub imported: u32,
    pub updated: u32,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Export every rule as versioned, deterministically-ordered JSON.
///
/// Key order is alphabetical (`serde_json::Map` is a `BTreeMap` without
/// `preserve_order`) and row order is the evaluation order, so two
/// exports of the same store bytes are byte-identical (fixpoint).
pub fn export_rules(store: &Arc<Mutex<Store>>) -> Result<String, String> {
    let rows = {
        let guard = store.lock().map_err(|e| format!("store lock: {e}"))?;
        queries::export_all_rules(guard.conn()).map_err(|e| e.to_string())?
    };
    let mut rules = Vec::with_capacity(rows.len());
    for row in &rows {
        rules.push(row_to_item(row)?);
    }
    let envelope = RuleExport { version: EXPORT_VERSION, rules };
    serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())
}

fn row_to_item(row: &RuleRow) -> Result<RuleExportItem, String> {
    let conditions_json: Value =
        serde_json::from_str(&row.conditions_json).map_err(|e| {
            format!("rule “{}” has corrupt conditions_json: {e}", row.name)
        })?;
    let scope_json: Value = serde_json::from_str(&row.scope_json)
        .map_err(|e| format!("rule “{}” has corrupt scope_json: {e}", row.name))?;
    Ok(RuleExportItem {
        id: row.id.clone(),
        name: row.name.clone(),
        priority: row.priority,
        conditions_json,
        dest_kind: row.dest_kind.clone(),
        destination: row.destination.clone(),
        scope_json,
        enabled: row.enabled,
    })
}

fn item_to_row(item: &RuleExportItem) -> Result<RuleRow, String> {
    Ok(RuleRow {
        id: item.id.trim().to_string(),
        name: item.name.trim().to_string(),
        priority: item.priority,
        conditions_json: serde_json::to_string(&item.conditions_json)
            .map_err(|e| e.to_string())?,
        dest_kind: item.dest_kind.clone(),
        destination: item.destination.trim().to_string(),
        scope_json: serde_json::to_string(&item.scope_json).map_err(|e| e.to_string())?,
        enabled: item.enabled,
        // Fresh imports start at 1; updates bump in `upsert_rule_import`.
        // These placeholders are overwritten there — never trusted here.
        rule_version: 1,
        rule_etag: None,
    })
}

/// Validate one item structurally (no DB). Returns `(errors, warnings)`.
/// DB-backed checks (duplicate-against-store, folder existence) run in
/// [`import_rules`] where a connection is available.
fn validate_item_structure(item: &RuleExportItem) -> (Vec<String>, Vec<String>) {
    let warnings = Vec::new();
    let mut errors = Vec::new();

    if item.id.trim().is_empty() {
        errors.push("empty rule id not allowed".to_string());
    }
    let name = item.name.trim();
    if name.is_empty() {
        errors.push("empty rule name not allowed".to_string());
    } else {
        if name.chars().count() > RULE_NAME_MAX {
            errors.push(format!(
                "rule name too long (max {RULE_NAME_MAX} characters): {name}"
            ));
        }
        if name.contains('/') || name.chars().any(|c| c.is_control()) {
            errors.push(format!("invalid rule name: '{name}'"));
        }
        if RESERVED_NAMES.contains(&name.to_lowercase().as_str()) {
            errors.push(format!("reserved name: {name}"));
        }
    }

    // Conditions must be a non-empty object (at least one matcher set).
    match conditions_of(item) {
        Ok(None) => errors.push("empty pattern not allowed".to_string()),
        Err(e) => errors.push(e),
        Ok(Some(_)) => {}
    }

    if item.priority < PRIORITY_MIN || item.priority > PRIORITY_MAX {
        errors.push("priority must be between 1 and 999".to_string());
    }
    if item.dest_kind != "folder" && item.dest_kind != "category" {
        errors.push(format!(
            "invalid dest_kind: {} (must be 'folder' or 'category')",
            item.dest_kind
        ));
    }
    let dest = item.destination.trim();
    if dest.is_empty() {
        errors.push("empty destination not allowed".to_string());
    } else if rules::is_destructive_destination(dest) {
        errors.push(format!(
            "forbidden destination: '{dest}' — rules only move, never delete"
        ));
    }

    (errors, warnings)
}

/// Parse the item's conditions; `Ok(None)` = present-but-empty.
fn conditions_of(item: &RuleExportItem) -> Result<Option<RuleConditions>, String> {
    if !item.conditions_json.is_object() {
        return Err("empty pattern not allowed".to_string());
    }
    let conds: RuleConditions =
        serde_json::from_value(item.conditions_json.clone())
            .map_err(|e| format!("invalid conditions: {e}"))?;
    let any = conds.sender_substring.as_ref().is_some_and(|s| !s.trim().is_empty())
        || conds.sender_domain.as_ref().is_some_and(|s| !s.trim().is_empty())
        || conds.subject_keyword.as_ref().is_some_and(|s| !s.trim().is_empty())
        || conds.subject_regex.as_ref().is_some_and(|s| !s.trim().is_empty())
        || conds.snippet_keyword.as_ref().is_some_and(|s| !s.trim().is_empty());
    if !any {
        return Ok(None);
    }
    if let Some(p) = conds.subject_regex.as_ref().filter(|s| !s.trim().is_empty()) {
        if p.chars().count() > RULE_REGEX_MAX {
            return Err(format!(
                "pattern too long (max {RULE_REGEX_MAX} characters)"
            ));
        }
        if regex::Regex::new(p).is_err() {
            return Err("invalid pattern: bad regular expression".to_string());
        }
    }
    Ok(Some(conds))
}

/// Scope folders as a string list (lenient: non-arrays read as empty).
fn scope_folders_of(item: &RuleExportItem) -> Vec<String> {
    match &item.scope_json {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
    }
}

/// Import rules from a versioned JSON envelope.
///
/// `commit = false` is a dry run: validation runs identically and counts
/// are hypothetical, but nothing is written. `commit = true` upserts by
/// id (new → insert, existing → overwrite + `rule_version` bump) and
/// rejects name collisions against a *different* id.
///
/// Atomicity (CR-01): validation runs fully BEFORE any write. Items that
/// fail validation are skipped with per-item reasons (partial-valid-subset
/// contract — WR-04); the surviving staged rows then apply inside ONE
/// rusqlite transaction, so a mid-apply DB failure rolls everything back
/// instead of leaving a half-applied file.
///
/// Locking (WR-06): the store Mutex is held only for two short windows —
/// the duplicate/folder snapshot read, and the commit transaction. All
/// structural validation runs lock-free in between, so a large import
/// never starves sync/classify.
pub fn import_rules(
    store: &Arc<Mutex<Store>>,
    json: &str,
    commit: bool,
) -> Result<ImportResult, String> {
    // WR-02: bound the payload before parsing, before any lock.
    if json.len() > MAX_IMPORT_BYTES {
        return Err("rules file too large (max 1 MiB)".to_string());
    }
    let envelope: RuleExport =
        serde_json::from_str(json).map_err(|e| plain_parse_error(&e))?;
    if envelope.version != EXPORT_VERSION {
        return Err(format!(
            "unsupported rules version {} (expected {EXPORT_VERSION})",
            envelope.version
        ));
    }
    if envelope.rules.len() > MAX_IMPORT_RULES {
        return Err(format!("too many rules (max {MAX_IMPORT_RULES})"));
    }

    // Snapshot window #1 (short lock): duplicate + warn-but-allow inputs.
    // The guard is dropped at the end of this block — everything below
    // until the commit transaction runs lock-free.
    let snap = {
        let guard = store.lock().map_err(|e| format!("store lock: {e}"))?;
        let conn = guard.conn();
        let names: Vec<(String, String)> = queries::get_all_rule_names(conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();
        let cached_folders: Vec<String> = conn
            .prepare("SELECT name FROM mailboxes")
            .and_then(|mut st| {
                st.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();
        let cached_display: Vec<String> = conn
            .prepare("SELECT display_name FROM mailboxes")
            .and_then(|mut st| {
                st.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();
        let known_category_ids: HashSet<String> = conn
            .prepare("SELECT json FROM taxonomy WHERE id = 1")
            .and_then(|mut st| {
                st.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default()
            .into_iter()
            .flat_map(|j| {
                serde_json::from_str::<crate::classify::taxonomy::Taxonomy>(&j)
                    .map(|t| t.categories.into_iter().map(|c| c.id).collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect();
        ImportSnapshot { names, cached_folders, cached_display, known_category_ids }
    };

    // Phase 1 (lock-free): validate every item. The working duplicate map
    // is seeded from the store snapshot and updated as rows stage, so a
    // rename earlier in the same file frees its old name for later items
    // (CR-01 stale-snapshot fix). One map covers both within-file and
    // against-store collisions — no divergent `seen` vs `known` sources.
    let mut working: std::collections::HashMap<String, String> =
        snap.names.iter().map(|(id, n)| (n.to_lowercase(), id.clone())).collect();
    let mut staged: Vec<RuleRow> = Vec::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    for item in &envelope.rules {
        let label = if item.name.trim().is_empty() { item.id.clone() } else { item.name.clone() };
        let (mut item_errors, mut item_warnings) = validate_item_structure(item);

        // Duplicate names (case-insensitive, like validate_rule): checked
        // against `working`, which already includes rows staged earlier in
        // this same file.
        let lname = item.name.trim().to_lowercase();
        if !lname.is_empty() {
            if let Some(owner) = working.get(&lname) {
                if owner != &item.id {
                    item_errors.push(format!("duplicate name: {}", item.name.trim()));
                }
            }
        }

        if !item_errors.is_empty() {
            for e in item_errors {
                errors.push(format!("rule “{label}”: {e}"));
            }
            continue;
        }

        // Warn-but-allow existence checks (CONTEXT D-03) against the snapshot.
        for folder in scope_folders_of(item) {
            if !snap.cached_folders.iter().any(|n| n == &folder) {
                item_warnings.push(format!("folder not found: {folder}"));
            }
        }
        if item.dest_kind == "folder" {
            let d = item.destination.trim();
            let known_folder = snap.cached_display.iter().any(|n| n.eq_ignore_ascii_case(d))
                || snap.cached_folders.iter().any(|n| n.eq_ignore_ascii_case(d));
            if !known_folder {
                item_warnings.push(format!("folder not found: {d}"));
            }
        } else if !snap.known_category_ids.contains(item.destination.trim()) {
            item_warnings
                .push(format!("category not found: {}", item.destination.trim()));
        }

        warnings.extend(item_warnings);

        let row = item_to_row(item)?;
        // Stage: this id now owns `lname`; a rename frees whatever name
        // this id previously held (visible to later items in this file).
        if !lname.is_empty() {
            working.retain(|_, owner| owner != &item.id);
            working.insert(lname, item.id.clone());
        }
        staged.push(row);
    }

    // Dry run: counts only, nothing written.
    if !commit {
        let known_ids: HashSet<&str> = snap.names.iter().map(|(id, _)| id.as_str()).collect();
        let (mut imported, mut updated) = (0u32, 0u32);
        for row in &staged {
            if known_ids.contains(row.id.as_str()) {
                updated += 1;
            } else {
                imported += 1;
            }
        }
        return Ok(ImportResult { imported, updated, warnings, errors });
    }

    // Snapshot window #2 (commit transaction): re-verify duplicates against
    // FRESH store state inside the tx — a concurrent writer may have landed
    // between the snapshot and now. Any conflict aborts with zero writes
    // (tx drops uncommitted). Staged ids overwrite their own rows, so a
    // fresh row owned by an id this import also stages is not a conflict.
    let mut guard = store.lock().map_err(|e| format!("store lock: {e}"))?;
    let tx = guard.conn_mut().transaction().map_err(|e| e.to_string())?;
    {
        let fresh: Vec<(String, String)> =
            queries::get_all_rule_names(&tx).map_err(|e| e.to_string())?;
        let staged_ids: HashSet<&str> = staged.iter().map(|r| r.id.as_str()).collect();
        for row in &staged {
            let lname = row.name.to_lowercase();
            for (fid, fname) in &fresh {
                if fname.to_lowercase() == lname
                    && fid != &row.id
                    && !staged_ids.contains(fid.as_str())
                {
                    return Err(format!(
                        "rule “{}”: duplicate name (store changed during import)",
                        row.name
                    ));
                }
            }
        }
        let mut imported = 0u32;
        let mut updated = 0u32;
        for row in &staged {
            let was_update =
                queries::upsert_rule_import(&tx, row).map_err(|e| e.to_string())?;
            if was_update {
                updated += 1;
            } else {
                imported += 1;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(ImportResult { imported, updated, warnings, errors })
    }
}

/// Short-lived snapshot inputs for import validation (WR-06): read under
/// one brief lock, then used lock-free for the whole validation pass.
struct ImportSnapshot {
    /// `(id, name)` pairs for duplicate detection.
    names: Vec<(String, String)>,
    /// Cached folder-tree internal names (`mailboxes.name`).
    cached_folders: Vec<String>,
    /// Cached folder-tree display names (`mailboxes.display_name`).
    cached_display: Vec<String>,
    /// Known taxonomy category ids.
    known_category_ids: HashSet<String>,
}

/// Map serde parse failures to the plan's plain-language vocabulary where
/// the shape is recognizable (notably `dest_kind` enum violations).
fn plain_parse_error(e: &serde_json::Error) -> String {
    let msg = e.to_string();
    if msg.contains("dest_kind") || (msg.contains("unknown variant") && msg.contains("expected")) {
        return "invalid dest_kind (must be 'folder' or 'category')".to_string();
    }
    msg
}

/// Read-only capability probe for the import/export surface (29-03 UI
/// gate). Portability is local-file JSON — it needs no server capability,
/// so this always returns `true`; the authoritative enforcement stays in
/// [`import_rules`]/[`export_rules`] validation. Kept as a function (not
/// a constant) so a future server-gated revision has one call site.
///
/// IN-01: retained (not deleted) — see note below.
///
/// NOTE (IN-01): `import_export_available` (this stub) and `folder_known`
/// (deleted below) had zero non-test callers; the import path inlines its
/// own snapshot queries. `import_export_available` is KEPT as the single
/// documented UI-gate call site for a future server-gated revision;
/// `folder_known` was REMOVED because its exact-match `name` check was
/// weaker than the inlined display-name-aware check and would rot.
pub fn import_export_available() -> bool {
    true
}

/// Read the IDLE atom from a Phase-27 `capability-profile.json` file.
/// Missing file, bad JSON, or absent atom → `false` (poll-only, the safe
/// degradation). Pure file read — no network, no secrets.
pub fn idle_allowed_from_profile(path: &std::path::Path) -> bool {
    read_idle_atom(path).unwrap_or(false)
}

fn read_idle_atom(path: &std::path::Path) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let accounts = v.get("accounts")?.as_array()?;
    // WR-03: consult EVERY account, not just accounts[0]. The watcher may
    // spawn when any account advertises IDLE; poll-only holds only when no
    // account carries a `true` atom (absent atom → abstain, fail-closed).
    let mut any_present = false;
    let mut any_idle = false;
    for a in accounts {
        if let Some(idle) = a
            .get("atoms")
            .and_then(|at| at.get("IDLE"))
            .and_then(|i| i.as_bool())
        {
            any_present = true;
            any_idle = any_idle || idle;
        }
    }
    if any_present {
        Some(any_idle)
    } else {
        None
    }
}

/// Default profile location: the repo-relative probe artifact (dev/test).
/// Production Tauri runs from a bundle dir where the file is absent →
/// `false` (poll-only).
///
/// IN-04: `SGE_CAPABILITY_PROFILE` is a dev/test escape hatch only —
/// honored in debug builds, IGNORED in release/bundled builds, so a
/// crafted profile file cannot force IDLE spawn via the environment (the
/// gate chooses whether to open a persistent connection).
pub fn default_profile_path() -> std::path::PathBuf {
    #[cfg(debug_assertions)]
    if let Ok(p) = std::env::var("SGE_CAPABILITY_PROFILE") {
        return std::path::PathBuf::from(p);
    }
    std::path::PathBuf::from(".planning/probes/capability-profile.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    fn seed_rule(conn: &rusqlite::Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next) VALUES ('INBOX', 1, 1)",
            [],
        )
        .unwrap_or(0);
        conn.execute(
            "INSERT INTO folder_rules (id, name, priority, conditions_json, dest_kind, destination, scope_json, enabled) \
             VALUES (?1, ?2, 100, '{\"sender_domain\":\"utfpr.edu.br\"}', 'folder', 'Arquivo', '[]', 1)",
            rusqlite::params![id, name],
        )
        .unwrap();
    }

    fn item_json(
        id: &str,
        name: &str,
        priority: i64,
        conditions: &str,
        dest_kind: &str,
        destination: &str,
    ) -> String {
        format!(
            "{{\"version\":1,\"rules\":[{{\"id\":\"{id}\",\"name\":\"{name}\",\"priority\":{priority},\
             \"conditions_json\":{conditions},\"dest_kind\":\"{dest_kind}\",\
             \"destination\":\"{destination}\",\"scope_json\":[],\"enabled\":true}}]}}"
        )
    }

    #[test]
    fn round_trip_fixpoint() {
        let store = test_store();
        seed_rule(store.lock().unwrap().conn(), "r-1", "Boletos");

        let json = export_rules(&store).unwrap();
        let before: Value = serde_json::from_str(&json).unwrap();

        // Re-import the export: same ids → updates, version bumps stay
        // server-side, so the second export must be byte-identical.
        let result = import_rules(&store, &json, true).unwrap();
        assert_eq!(result.updated, 1, "re-import must update, got {result:?}");
        assert_eq!(result.imported, 0);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let json2 = export_rules(&store).unwrap();
        assert_eq!(json, json2, "export→import→export must be byte-identical");
        let _ = before;
    }

    #[test]
    fn dry_run_writes_nothing() {
        let store = test_store();
        let json = item_json(
            "r-new",
            "Nova",
            100,
            "{\"sender_domain\":\"utfpr.edu.br\"}",
            "folder",
            "Arquivo",
        );
        let preview = import_rules(&store, &json, false).unwrap();
        assert_eq!(preview.imported, 1);
        assert!(preview.errors.is_empty(), "errors: {:?}", preview.errors);
        let n: i64 = store
            .lock()
            .unwrap()
            .conn()
            .query_row("SELECT COUNT(*) FROM folder_rules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "dry run must not write");
        let committed = import_rules(&store, &json, true).unwrap();
        assert_eq!(committed.imported, 1);
    }

    #[test]
    fn validation_empty_pattern() {
        let json = item_json("r-1", "Vazia", 100, "[]", "folder", "Arquivo");
        let r = import_rules(&test_store(), &json, true).unwrap();
        assert_eq!(r.imported, 0);
        assert!(r.errors.iter().any(|e| e.contains("empty pattern not allowed")), "got {:?}", r.errors);
        let json2 = item_json("r-1", "Vazia", 100, "{}", "folder", "Arquivo");
        let r2 = import_rules(&test_store(), &json2, true).unwrap();
        assert!(r2.errors.iter().any(|e| e.contains("empty pattern not allowed")), "got {:?}", r2.errors);
    }

    #[test]
    fn validation_reserved_names() {
        for reserved in ["system", "SYSTEM", "auto", "Auto"] {
            let json = item_json(
                "r-1",
                reserved,
                100,
                "{\"sender_domain\":\"x.com\"}",
                "folder",
                "Arquivo",
            );
            let r = import_rules(&test_store(), &json, true).unwrap();
            assert!(r.errors.iter().any(|e| e.contains("reserved name")), "for {reserved}: {:?}", r.errors);
        }
    }

    #[test]
    fn validation_duplicate_names() {
        // Within the file.
        let both = format!(
            "{{\"version\":1,\"rules\":[{}, {}]}}",
            rule_obj("r-1", "Dup", 100),
            rule_obj("r-2", "Dup", 200),
        );
        let r = import_rules(&test_store(), &both, true).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("duplicate name")), "got {:?}", r.errors);
        // Against the store under a different id.
        let store = test_store();
        seed_rule(store.lock().unwrap().conn(), "r-old", "Boletos");
        let clash = format!("{{\"version\":1,\"rules\":[{}]}}", rule_obj("r-new", "Boletos", 100));
        let r2 = import_rules(&store, &clash, true).unwrap();
        assert!(r2.errors.iter().any(|e| e.contains("duplicate name")), "got {:?}", r2.errors);
    }

    fn rule_obj(id: &str, name: &str, priority: i64) -> String {
        format!(
            "{{\"id\":\"{id}\",\"name\":\"{name}\",\"priority\":{priority},\
             \"conditions_json\":{{\"sender_domain\":\"x.com\"}},\"dest_kind\":\"folder\",\
             \"destination\":\"Arquivo\",\"scope_json\":[],\"enabled\":true}}"
        )
    }

    #[test]
    fn validation_priority_range() {
        for bad in [0, 1000, -5] {
            let json = format!("{{\"version\":1,\"rules\":[{}]}}", rule_obj("r-1", "P", bad));
            let r = import_rules(&test_store(), &json, true).unwrap();
            assert!(
                r.errors.iter().any(|e| e.contains("priority must be between 1 and 999")),
                "for {bad}: {:?}",
                r.errors
            );
        }
        let ok = format!("{{\"version\":1,\"rules\":[{}]}}", rule_obj("r-1", "P", 999));
        let r = import_rules(&test_store(), &ok, true).unwrap();
        assert!(r.errors.is_empty(), "got {:?}", r.errors);
    }

    #[test]
    fn validation_invalid_dest_kind() {
        let json = item_json(
            "r-1",
            "D",
            100,
            "{\"sender_domain\":\"x.com\"}",
            "invalid",
            "Arquivo",
        );
        let r = import_rules(&test_store(), &json, true).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("invalid dest_kind")), "got {:?}", r.errors);
    }

    #[test]
    fn validation_unknown_folder_warns_but_imports() {
        let json = item_json(
            "r-1",
            "Fantasma",
            100,
            "{\"sender_domain\":\"x.com\"}",
            "folder",
            "PastaFantasma",
        );
        let r = import_rules(&test_store(), &json, true).unwrap();
        assert_eq!(r.imported, 1, "warn-but-allow must import: {r:?}");
        assert!(r.warnings.iter().any(|w| w.contains("folder not found")), "got {:?}", r.warnings);
        assert!(r.errors.is_empty(), "got {:?}", r.errors);
    }

    #[test]
    fn import_rejects_oversize_and_overcount() {
        // WR-02: absurd payloads fail BEFORE parsing/locking, loudly.
        let big = format!("{{\"version\":1,\"rules\":[]}}{}", "x".repeat(MAX_IMPORT_BYTES));
        let r = import_rules(&test_store(), &big, false);
        assert!(r.as_ref().is_err_and(|e| e.contains("too large")), "got {r:?}");
        let many: Vec<String> =
            (0..MAX_IMPORT_RULES + 1).map(|i| rule_obj(&format!("r-{i}"), &format!("N{i}"), 100)).collect();
        let crowded = format!("{{\"version\":1,\"rules\":[{}]}}", many.join(","));
        let r2 = import_rules(&test_store(), &crowded, true);
        assert!(r2.as_ref().is_err_and(|e| e.contains("too many rules")), "got {r2:?}");
        // Boundary: exactly MAX_IMPORT_RULES is accepted (dry run).
        let exact: Vec<String> =
            (0..MAX_IMPORT_RULES).map(|i| rule_obj(&format!("r-{i}"), &format!("N{i}"), 100)).collect();
        let ok = format!("{{\"version\":1,\"rules\":[{}]}}", exact.join(","));
        let r3 = import_rules(&test_store(), &ok, false).unwrap();
        assert_eq!(r3.imported as usize, MAX_IMPORT_RULES, "got {r3:?}");
        assert!(r3.errors.is_empty(), "got {:?}", r3.errors);
    }

    #[test]
    fn import_same_file_rename_frees_name() {
        // CR-01 stale-snapshot fix: a rename earlier in the same file frees
        // its old name — a later item claiming it must NOT be a duplicate.
        let store = test_store();
        seed_rule(store.lock().unwrap().conn(), "r-a", "Antigo");
        let file = format!(
            "{{\"version\":1,\"rules\":[{}, {}]}}",
            rule_obj("r-a", "Novo", 100),
            rule_obj("r-b", "Antigo", 100),
        );
        let r = import_rules(&store, &file, true).unwrap();
        assert!(r.errors.is_empty(), "rename must free the old name: {:?}", r.errors);
        assert_eq!((r.imported, r.updated), (1, 1), "got {r:?}");
    }

    #[test]
    fn import_commit_counts_match_dry_run() {
        // Validate-all-then-apply: preview counts must equal commit counts
        // for the same file (no write happens before validation completes).
        let store = test_store();
        seed_rule(store.lock().unwrap().conn(), "r-old", "Boletos");
        let file = format!(
            "{{\"version\":1,\"rules\":[{}, {}, {}]}}",
            rule_obj("r-old", "Boletos", 200),
            rule_obj("r-new", "Nova", 100),
            rule_obj("r-bad", "", 100),
        );
        let preview = import_rules(&store, &file, false).unwrap();
        assert_eq!((preview.imported, preview.updated), (1, 1), "got {preview:?}");
        assert_eq!(preview.errors.len(), 1, "got {:?}", preview.errors);
        let committed = import_rules(&store, &file, true).unwrap();
        assert_eq!(
            (committed.imported, committed.updated),
            (preview.imported, preview.updated),
            "commit must apply exactly the previewed subset"
        );
        assert_eq!(committed.errors, preview.errors);
    }

    #[test]
    fn import_existing_id_updates_and_bumps_version() {
        let store = test_store();
        seed_rule(store.lock().unwrap().conn(), "r-1", "Boletos");
        let v0: i64 = store
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT rule_version FROM folder_rules WHERE id = 'r-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v0, 1);
        let json = format!("{{\"version\":1,\"rules\":[{}]}}", rule_obj("r-1", "Boletos", 200));
        let r = import_rules(&store, &json, true).unwrap();
        assert_eq!((r.imported, r.updated), (0, 1), "got {r:?}");
        let v1: i64 = store
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT rule_version FROM folder_rules WHERE id = 'r-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v1, 2, "import update must bump rule_version");
    }

    #[test]
    fn live_import_export_gate_skips_offline() {
        // Live-gate shape (sandbox contract): without operator credentials
        // this leg must SKIP, never fail and never touch the network.
        if std::env::var("SGE_LIVE_IMAP_USER").is_err() {
            eprintln!("SKIP: live import/export leg needs SGE_LIVE_IMAP_USER (operator-only)");
            return;
        }
        // With creds the operator runs the credentialed leg externally;
        // the headless round-trip above is the assertion.
        let _store = test_store();
    }

    #[test]
    fn idle_gate_reads_profile_atom() {
        let dir = std::env::temp_dir();
        let off = dir.join("sge-29-idle-off.json");
        std::fs::write(
            &off,
            r#"{"accounts":[{"atoms":{"IDLE":false}}]}"#,
        )
        .unwrap();
        assert!(!idle_allowed_from_profile(&off));
        let on = dir.join("sge-29-idle-on.json");
        std::fs::write(&on, r#"{"accounts":[{"atoms":{"IDLE":true}}]}"#).unwrap();
        assert!(idle_allowed_from_profile(&on));
        assert!(!idle_allowed_from_profile(dir.join("sge-29-does-not-exist.json").as_path()));
        let _ = std::fs::remove_file(&off);
        let _ = std::fs::remove_file(&on);
    }

    #[test]
    fn idle_gate_consults_every_account() {
        // WR-03: the gate must not read only accounts[0]. A capable second
        // account allows spawn; all-false refuses; absent atoms abstain
        // (fail-closed to poll-only).
        let dir = std::env::temp_dir();
        let second = dir.join("sge-29-idle-second.json");
        std::fs::write(
            &second,
            r#"{"accounts":[{"atoms":{"IDLE":false}},{"atoms":{"IDLE":true}}]}"#,
        )
        .unwrap();
        assert!(idle_allowed_from_profile(&second));
        let none = dir.join("sge-29-idle-none.json");
        std::fs::write(
            &none,
            r#"{"accounts":[{"atoms":{"IDLE":false}},{"atoms":{"IDLE":false}}]}"#,
        )
        .unwrap();
        assert!(!idle_allowed_from_profile(&none));
        let noatoms = dir.join("sge-29-idle-noatoms.json");
        std::fs::write(&noatoms, r#"{"accounts":[{}]}"#).unwrap();
        assert!(!idle_allowed_from_profile(&noatoms));
        let empty = dir.join("sge-29-idle-empty.json");
        std::fs::write(&empty, r#"{"accounts":[]}"#).unwrap();
        assert!(!idle_allowed_from_profile(&empty));
        let _ = std::fs::remove_file(&second);
        let _ = std::fs::remove_file(&none);
        let _ = std::fs::remove_file(&noatoms);
        let _ = std::fs::remove_file(&empty);
    }
}
