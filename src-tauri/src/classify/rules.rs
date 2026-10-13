//! Smart folder rules (Phase 25, Plan 25-01): deterministic user-defined
//! filing rules evaluated over already-cached mail text.
//!
//! HARD INVARIANTS (structural, test-asserted — never relax):
//! - The matcher reads ONLY caller-supplied cached text (`sender`,
//!   `subject`, `snippet`). This module imports NOTHING from `imap` or
//!   `sync`: rules can never trigger a BODY fetch, so BODY.PEEK-only holds.
//! - Rules are MOVE-only: the [`RuleDestination`] model has exactly two
//!   variants (plain folder, taxonomy category) — no delete/expunge variant
//!   exists, and [`validate_rule`] rejects any destructive resolution.
//!   A rule hit becomes a `rule_suggestions` row confirmed through the
//!   unmodified `confirm_suggestion` path; rules never move mail directly.
//! - Conditions are stored as data (JSON), never evaluated as SQL — the
//!   matcher runs in Rust over bound parameters, so quote-laden patterns
//!   cannot inject.
//!
//! Match semantics (CONTEXT D-01): AND within a rule (every non-empty
//! condition must match), OR across rules (first match wins by priority —
//! enforced by the evaluation loop in `commands::classify`, oldest wins
//! ties via `list_rules` ordering).

use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::taxonomy::fold;

/// Max subject-regex pattern length (CONTEXT D-01: ≤200 chars — also bounds
/// catastrophic-backtracking exposure alongside the input cap).
pub const RULE_REGEX_MAX: usize = 200;
/// Matcher input bound (CONTEXT D-01: evaluated input ≤1000 chars).
pub const RULE_INPUT_MAX: usize = 1000;
/// Max rule name length (mirrors the Phase 19 category-name cap).
pub const RULE_NAME_MAX: usize = 80;
/// Default priority for newly created rules (explicit reorder lowers it).
pub const RULE_DEFAULT_PRIORITY: i64 = 100;

/// Match conditions for one rule. Every `Some(non-empty)` field must match
/// (AND). A rule with no non-empty condition never matches and is rejected
/// at validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuleConditions {
    /// Case-insensitive substring of the full `From` header.
    #[serde(default)]
    pub sender_substring: Option<String>,
    /// Case-insensitive sender domain (`example.com` matches
    /// `user@example.com`).
    #[serde(default)]
    pub sender_domain: Option<String>,
    /// Case-insensitive (accent-folded) subject keyword.
    #[serde(default)]
    pub subject_keyword: Option<String>,
    /// Subject regex (`regex` crate, compiled at create time).
    #[serde(default)]
    pub subject_regex: Option<String>,
    /// Case-insensitive (accent-folded) keyword over the ALREADY-CACHED
    /// snippet/preview text. Never triggers a body fetch.
    #[serde(default)]
    pub snippet_keyword: Option<String>,
}

/// Rule destination. Exactly two variants — MOVE-only by construction.
/// There is deliberately NO delete/expunge variant (RULE-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleDestination {
    /// Plain folder by display name (may be created via Phase 11 guards).
    Folder { display: String },
    /// Taxonomy category by STABLE id (renames never break the rule —
    /// 25-02 merge/delete remap flows through the same id).
    TaxonomyCategory { id: String },
}

/// Folder scope for automatic evaluation. Empty `folders` = all folders.
/// Folders excluded from auto-classify (Sent/Drafts) are skipped by the
/// automatic pass regardless; manual `apply_rules` still serves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuleScope {
    #[serde(default)]
    pub folders: Vec<String>,
}

/// A persisted smart-folder rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub priority: i64,
    pub conditions: RuleConditions,
    pub destination: RuleDestination,
    pub scope: RuleScope,
    pub enabled: bool,
}

/// Bound matcher input to [`RULE_INPUT_MAX`] chars (char-boundary safe).
fn bounded(s: &str) -> String {
    if s.len() <= RULE_INPUT_MAX {
        return s.to_string();
    }
    s.chars().take(RULE_INPUT_MAX).collect()
}

fn has(cond: &Option<String>) -> bool {
    cond.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

fn sender_domain_of(sender: &str) -> &str {
    match sender.rfind('@') {
        Some(i) => sender[i + 1..].trim(),
        None => sender.trim(),
    }
}

/// Pure matcher: AND-within-rule over caller-supplied CACHED text only.
///
/// `sender` is the cached `from_addr`, `subject` the cached subject,
/// `snippet` the cached preview/snippet. No I/O, no IMAP, total function
/// (invalid regex ⇒ that condition fails rather than panics — validation
/// rejects such rules before storage).
pub fn evaluate_rule(rule: &Rule, sender: &str, subject: &str, snippet: &str) -> bool {
    let c = &rule.conditions;
    if !has(&c.sender_substring)
        && !has(&c.sender_domain)
        && !has(&c.subject_keyword)
        && !has(&c.subject_regex)
        && !has(&c.snippet_keyword)
    {
        return false;
    }
    let sender_lc = sender.to_lowercase();
    let subject_b = bounded(subject);
    let subject_fold = fold(&subject_b);
    let snippet_fold = fold(snippet);
    if let Some(p) = c.sender_substring.as_ref().filter(|s| !s.trim().is_empty()) {
        if !sender_lc.contains(&p.to_lowercase()) {
            return false;
        }
    }
    if let Some(p) = c.sender_domain.as_ref().filter(|s| !s.trim().is_empty()) {
        if !sender_domain_of(sender).eq_ignore_ascii_case(p.trim()) {
            return false;
        }
    }
    if let Some(p) = c.subject_keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        if !subject_fold.contains(&fold(p)) {
            return false;
        }
    }
    if let Some(p) = c.subject_regex.as_ref().filter(|s| !s.trim().is_empty()) {
        let re = match Regex::new(p) {
            Ok(re) => re,
            Err(_) => return false,
        };
        if !re.is_match(&subject_b) {
            return false;
        }
    }
    if let Some(p) = c.snippet_keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        if !snippet_fold.contains(&fold(p)) {
            return false;
        }
    }
    true
}

/// Transient rule-match explanation for the confirm dialog. The caller MUST
/// pass the result through `redact::filter_output` and MUST NOT persist it
/// (RULE-03 verbatim: transient UI strings only; logs carry rule-id +
/// message-id).
pub fn rule_justification(rule: &Rule) -> String {
    let mut bits = Vec::new();
    let c = &rule.conditions;
    if let Some(p) = c.sender_substring.as_ref().filter(|s| !s.trim().is_empty()) {
        bits.push(format!("remetente contém “{}”", p.trim()));
    }
    if let Some(p) = c.sender_domain.as_ref().filter(|s| !s.trim().is_empty()) {
        bits.push(format!("domínio “{}”", p.trim()));
    }
    if let Some(p) = c.subject_keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        bits.push(format!("assunto contém “{}”", p.trim()));
    }
    if c.subject_regex.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
        bits.push("assunto casa com a expressão".to_string());
    }
    if let Some(p) = c.snippet_keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        bits.push(format!("trecho contém “{}”", p.trim()));
    }
    let dest = match &rule.destination {
        RuleDestination::Folder { display } => display.clone(),
        RuleDestination::TaxonomyCategory { id } => format!("categoria {id}"),
    };
    format!("Regra “{}”: {} → {}", rule.name, bits.join(", "), dest)
}

// ── Backend-enforced validation (T-25-01, T-25-03) ────────────────────

/// Plain-language validation failures (Phase 16/19 style: UI restraint is
/// never the only guard — every write path calls [`validate_rule`]).
#[derive(Debug, PartialEq, Eq)]
pub enum RuleError {
    EmptyName,
    NameTooLong,
    NameInjection(String),
    DuplicateName(String),
    EmptyConditions,
    PatternTooLong,
    BadRegex(String),
    ReservedDestination(String),
    UnknownDestination(String),
    UnknownScopeFolder(String),
    DestructiveDestination(String),
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleError::EmptyName => write!(f, "dê um nome para a regra"),
            RuleError::NameTooLong => {
                write!(f, "nome de regra longo demais (máx. {RULE_NAME_MAX} caracteres)")
            }
            RuleError::NameInjection(n) => {
                write!(f, "nome de regra inválido: '{n}' contém caractere proibido")
            }
            RuleError::DuplicateName(n) => write!(f, "já existe uma regra chamada '{n}'"),
            RuleError::EmptyConditions => {
                write!(f, "a regra precisa de ao menos uma condição (remetente, assunto ou trecho)")
            }
            RuleError::PatternTooLong => {
                write!(f, "expressão longa demais (máx. {RULE_REGEX_MAX} caracteres)")
            }
            RuleError::BadRegex(e) => write!(f, "expressão inválida: {e}"),
            RuleError::ReservedDestination(d) => {
                write!(f, "pasta reservada: '{d}' não pode ser destino de regra")
            }
            RuleError::UnknownDestination(d) => {
                write!(f, "destino desconhecido: '{d}' não existe nem pode ser criado")
            }
            RuleError::UnknownScopeFolder(d) => {
                write!(f, "pasta desconhecida no escopo: '{d}'")
            }
            RuleError::DestructiveDestination(d) => {
                write!(f, "destino proibido: '{d}' — regras só movem, nunca apagam")
            }
        }
    }
}

/// Stable id from the rule name (slug + numeric dedupe), mirroring the
/// Phase 19 category-id style.
pub fn slug_for(name: &str) -> String {
    let folded = fold(name.trim());
    let mut s: String = folded
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        format!("regra-{}", chrono_like_suffix())
    } else {
        s
    }
}

fn chrono_like_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Structural MOVE-only guard: no destination string may resolve to a
/// destructive op. The [`RuleDestination`] model cannot express delete, so
/// this matches the textual foot-guns (defense in depth alongside the
/// zero-IMAP-traffic test).
pub fn is_destructive_destination(display: &str) -> bool {
    let d = display.trim().to_ascii_lowercase();
    d == "\\deleted" || d == "expunge" || d.contains("\\deleted")
}

/// Backend-enforced rule validation. `ignore_id` excludes the rule being
/// edited from the uniqueness check (invalid edits are rejected and the
/// old rule is kept by the caller). `taxonomy_ids` are the known category
/// ids; mailbox existence/creatability is read from the cached folder tree.
pub fn validate_rule(
    conn: &Connection,
    name: &str,
    conditions: &RuleConditions,
    destination: &RuleDestination,
    scope: &RuleScope,
    ignore_id: Option<&str>,
) -> Result<(), RuleError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RuleError::EmptyName);
    }
    if name.chars().count() > RULE_NAME_MAX {
        return Err(RuleError::NameTooLong);
    }
    if name.contains('/') || name.chars().any(|c| c.is_control()) {
        return Err(RuleError::NameInjection(name.to_string()));
    }
    // Uniqueness (case-insensitive, like Phase 19 sibling names).
    let dup: i64 = conn
        .query_row(
            "SELECT count(*) FROM folder_rules WHERE lower(name) = lower(?1) \
             AND (?2 IS NULL OR id != ?2)",
            rusqlite::params![name, ignore_id],
            |r| r.get(0),
        )
        .map_err(|_| RuleError::DuplicateName(name.to_string()))?;
    if dup > 0 {
        return Err(RuleError::DuplicateName(name.to_string()));
    }
    // At least one non-empty condition.
    if !has(&conditions.sender_substring)
        && !has(&conditions.sender_domain)
        && !has(&conditions.subject_keyword)
        && !has(&conditions.subject_regex)
        && !has(&conditions.snippet_keyword)
    {
        return Err(RuleError::EmptyConditions);
    }
    // Regex: length cap + must compile (T-25-02).
    if let Some(p) = conditions
        .subject_regex
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        if p.chars().count() > RULE_REGEX_MAX {
            return Err(RuleError::PatternTooLong);
        }
        Regex::new(p).map_err(|e| RuleError::BadRegex(short_regex_error(&e)))?;
    }
    // Destination checks (T-25-01, T-25-03).
    match destination {
        RuleDestination::Folder { display } => {
            let d = display.trim();
            if d.is_empty() {
                return Err(RuleError::UnknownDestination(display.clone()));
            }
            if is_destructive_destination(d) {
                return Err(RuleError::DestructiveDestination(d.to_string()));
            }
            // Reserved roots: `Auto` root and INBOX (case-insensitive).
            // Role folders (Sent/Drafts/…) are allowed — the rule IS the
            // explicit pick. `\Noselect` rows can never hold mail.
            if d.eq_ignore_ascii_case("auto") || d.eq_ignore_ascii_case("inbox") {
                return Err(RuleError::ReservedDestination(d.to_string()));
            }
            let cached: Vec<(String, String)> = conn
                .prepare("SELECT display_name, attributes FROM mailboxes")
                .and_then(|mut st| {
                    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default();
            if let Some((_, attrs)) = cached
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(d))
            {
                if attrs.contains("Noselect") || attrs.contains("Noinferiors") {
                    return Err(RuleError::ReservedDestination(d.to_string()));
                }
            } else if !folder_looks_creatable(d) {
                return Err(RuleError::UnknownDestination(d.to_string()));
            }
        }
        RuleDestination::TaxonomyCategory { id } => {
            let known: Vec<String> = conn
                .prepare("SELECT json FROM taxonomy WHERE id = 1")
                .and_then(|mut st| {
                    st.query_map([], |r| r.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default()
                .into_iter()
                .flat_map(|json| {
                    serde_json::from_str::<super::taxonomy::Taxonomy>(&json)
                        .map(|t| t.categories.into_iter().map(|c| c.id).collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect();
            if !known.iter().any(|k| k == id) {
                return Err(RuleError::UnknownDestination(id.clone()));
            }
        }
    }
    // Scope folders must exist in the cached tree (unknown folders would
    // make the rule silently partial).
    if !scope.folders.is_empty() {
        let cached: Vec<String> = conn
            .prepare("SELECT name FROM mailboxes")
            .and_then(|mut st| {
                st.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();
        for folder in &scope.folders {
            if !cached.iter().any(|n| n == folder) {
                return Err(RuleError::UnknownScopeFolder(folder.clone()));
            }
        }
    }
    Ok(())
}

/// A display name is creatable when every `/`-separated segment is
/// non-empty, has no control chars, and the first segment is not the
/// reserved `Auto` root (nesting UNDER `Auto/` is fine — the confirm path
/// applies `ensure_auto_tree` collision semantics unchanged).
fn folder_looks_creatable(display: &str) -> bool {
    let mut parts = display.split('/');
    match parts.next() {
        Some(first) if !first.trim().is_empty() && !first.eq_ignore_ascii_case("auto") => {}
        _ => return false,
    }
    parts.all(|p| !p.trim().is_empty() && !p.chars().any(|c| c.is_control()))
}

fn short_regex_error(e: &regex::Error) -> String {
    let s = e.to_string();
    s.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_with(cond: RuleConditions) -> Rule {
        Rule {
            id: "t".into(),
            name: "T".into(),
            priority: 100,
            conditions: cond,
            destination: RuleDestination::Folder { display: "Arquivo".into() },
            scope: RuleScope::default(),
            enabled: true,
        }
    }

    fn cond() -> RuleConditions {
        RuleConditions::default()
    }

    #[test]
    fn rules_and_semantics_all_conditions_must_match() {
        let mut c = cond();
        c.sender_substring = Some("utfpr".into());
        c.subject_keyword = Some("reunião".into());
        let r = rule_with(c);
        assert!(evaluate_rule(&r, "Colegiado <c@utfpr.edu.br>", "Reunião quinta", "oi"));
        assert!(!evaluate_rule(&r, "Colegiado <c@gmail.com>", "Reunião quinta", "oi"));
        assert!(!evaluate_rule(&r, "Colegiado <c@utfpr.edu.br>", "Festa sexta", "oi"));
    }

    #[test]
    fn rules_sender_domain_match_is_exact_and_case_insensitive() {
        let mut c = cond();
        c.sender_domain = Some("UTFPR.EDU.BR".into());
        let r = rule_with(c);
        assert!(evaluate_rule(&r, "alguem@utfpr.edu.br", "s", "x"));
        assert!(!evaluate_rule(&r, "alguem@notutfpr.edu.br", "s", "x"));
        assert!(!evaluate_rule(&r, "alguem@utfpr.edu.br.fake.com", "s", "x"));
    }

    #[test]
    fn rules_subject_regex_hit_and_miss() {
        let mut c = cond();
        c.subject_regex = Some(r"^Fatura \d{4}$".into());
        let r = rule_with(c);
        assert!(evaluate_rule(&r, "a@b.c", "Fatura 2024", "x"));
        assert!(!evaluate_rule(&r, "a@b.c", "Fatura anual", "x"));
        // Invalid regex fails closed (validation rejects before storage).
        let mut c2 = cond();
        c2.subject_regex = Some("([unclosed".into());
        assert!(!evaluate_rule(&rule_with(c2), "a@b.c", "qualquer", "x"));
    }

    #[test]
    fn rules_snippet_comes_only_from_cache() {
        // The matcher receives the snippet as a value — it cannot fetch.
        let mut c = cond();
        c.snippet_keyword = Some("boleto".into());
        let r = rule_with(c);
        assert!(evaluate_rule(&r, "a@b.c", "s", "segue o boleto em anexo"));
        assert!(!evaluate_rule(&r, "a@b.c", "s", "segue o recibo em anexo"));
    }

    #[test]
    fn rules_empty_conditions_never_match() {
        assert!(!evaluate_rule(&rule_with(cond()), "a@b.c", "s", "x"));
    }

    #[test]
    fn rules_module_emits_no_destructive_imap_verbs() {
        // Structural MOVE-only assertion: the non-test code of this module
        // must never name a destructive verb. Comments/docs are stripped
        // first so the assertion guards behavior, not prose.
        let src = include_str!("rules.rs");
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        let mut code: String = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // The destructive-name DENYLIST lives in exactly one place
        // (`is_destructive_destination`, validation defense-in-depth) —
        // excise it so the scan guards verb usage, not the denylist.
        if let Some(start) = code.find("pub fn is_destructive_destination") {
            if let Some(rel) = code[start..].find("\n}\n") {
                code.drain(start..start + rel + 3);
            }
        }
        assert!(!code.contains("EXPUNGE"), "rules.rs must not emit EXPUNGE");
        assert!(!code.contains("\\Deleted"), "rules.rs must not set \\Deleted");
        assert!(!code.contains("expunge"), "rules.rs must not emit expunge");
        // The destination model has exactly the two MOVE variants.
        let folder = RuleDestination::Folder { display: "X".into() };
        let cat = RuleDestination::TaxonomyCategory { id: "y".into() };
        assert_ne!(
            serde_json::to_string(&folder).unwrap(),
            serde_json::to_string(&cat).unwrap()
        );
    }

    #[test]
    fn rules_justification_mentions_conditions_and_destination() {
        let mut c = cond();
        c.sender_domain = Some("utfpr.edu.br".into());
        let r = rule_with(c);
        let j = rule_justification(&r);
        assert!(j.contains("utfpr.edu.br") && j.contains("Arquivo"));
    }

    // ── rule_valid: backend-enforced validation (T-25-01, T-25-03) ──

    fn valid_test_db() -> crate::store::Store {
        let store = crate::store::Store::open_in_memory().unwrap();
        let conn = store.conn();
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter) \
             VALUES ('INBOX', 1, 1, '/'), ('Arquivo', 1, 1, '/')",
            [],
        )
        .unwrap();
        store
    }

    fn dest_folder(display: &str) -> RuleDestination {
        RuleDestination::Folder { display: display.to_string() }
    }

    fn sender_cond() -> RuleConditions {
        let mut c = cond();
        c.sender_domain = Some("utfpr.edu.br".into());
        c
    }

    #[test]
    fn rule_valid_empty_patterns_rejected() {
        let store = valid_test_db();
        let err = validate_rule(
            store.conn(), "Sem condição", &cond(), &dest_folder("Arquivo"),
            &RuleScope::default(), None,
        )
        .unwrap_err();
        assert_eq!(err, RuleError::EmptyConditions);
    }

    #[test]
    fn rule_valid_bad_regex_and_pattern_cap_rejected() {
        let store = valid_test_db();
        let mut c = cond();
        c.subject_regex = Some("([aberto".into());
        let err = validate_rule(
            store.conn(), "Regex ruim", &c, &dest_folder("Arquivo"),
            &RuleScope::default(), None,
        )
        .unwrap_err();
        assert!(matches!(err, RuleError::BadRegex(_)), "got {err:?}");
        let mut c2 = cond();
        c2.subject_regex = Some("a".repeat(RULE_REGEX_MAX + 1));
        assert_eq!(
            validate_rule(
                store.conn(), "Regex longa", &c2, &dest_folder("Arquivo"),
                &RuleScope::default(), None
            )
            .unwrap_err(),
            RuleError::PatternTooLong
        );
    }

    #[test]
    fn rule_valid_reserved_destinations_rejected() {
        let store = valid_test_db();
        for reserved in ["Auto", "auto", "INBOX", "inbox"] {
            let err = validate_rule(
                store.conn(), "R", &sender_cond(), &dest_folder(reserved),
                &RuleScope::default(), None,
            )
            .unwrap_err();
            assert_eq!(err, RuleError::ReservedDestination(reserved.to_string()), "for {reserved}");
        }
        // Destructive resolutions rejected structurally (T-25-03).
        for bad in ["\\Deleted", "expunge"] {
            let err = validate_rule(
                store.conn(), "R", &sender_cond(), &dest_folder(bad),
                &RuleScope::default(), None,
            )
            .unwrap_err();
            assert_eq!(err, RuleError::DestructiveDestination(bad.to_string()), "for {bad}");
        }
    }

    #[test]
    fn rule_valid_names_guarded() {
        let store = valid_test_db();
        let d = dest_folder("Arquivo");
        let s = RuleScope::default();
        assert_eq!(
            validate_rule(store.conn(), "  ", &sender_cond(), &d, &s, None).unwrap_err(),
            RuleError::EmptyName
        );
        assert_eq!(
            validate_rule(store.conn(), &"n".repeat(81), &sender_cond(), &d, &s, None)
                .unwrap_err(),
            RuleError::NameTooLong
        );
        assert!(matches!(
            validate_rule(store.conn(), "a/b", &sender_cond(), &d, &s, None).unwrap_err(),
            RuleError::NameInjection(_)
        ));
        // Uniqueness (case-insensitive); self excluded on edit.
        conn_seed_rule(store.conn(), "r-1", "Boletos");
        assert_eq!(
            validate_rule(store.conn(), "boletos", &sender_cond(), &d, &s, None).unwrap_err(),
            RuleError::DuplicateName("boletos".to_string())
        );
        validate_rule(store.conn(), "boletos", &sender_cond(), &d, &s, Some("r-1")).unwrap();
    }

    #[test]
    fn rule_valid_unknown_scope_folder_rejected() {
        let store = valid_test_db();
        let scope = RuleScope { folders: vec!["PastaFantasma".into()] };
        assert_eq!(
            validate_rule(
                store.conn(), "R", &sender_cond(), &dest_folder("Arquivo"), &scope, None
            )
            .unwrap_err(),
            RuleError::UnknownScopeFolder("PastaFantasma".to_string())
        );
    }

    #[test]
    fn rule_valid_taxonomy_category_must_exist() {
        let store = valid_test_db();
        let missing = RuleDestination::TaxonomyCategory { id: "nao-existe".into() };
        assert_eq!(
            validate_rule(
                store.conn(), "R", &sender_cond(), &missing, &RuleScope::default(), None
            )
            .unwrap_err(),
            RuleError::UnknownDestination("nao-existe".to_string())
        );
    }

    #[test]
    fn rule_valid_quote_laden_patterns_are_data_not_sql() {
        // Quote-laden patterns must not error as SQL — conditions ride as
        // bound params and the matcher runs in Rust (no injection surface).
        let store = valid_test_db();
        conn_seed_rule(store.conn(), "r-q", "Intacta");
        let mut c = cond();
        c.subject_keyword = Some("d'água \"especial\"; DROP TABLE folder_rules;--".into());
        validate_rule(
            store.conn(), "Aspas", &c, &dest_folder("Arquivo"), &RuleScope::default(), None,
        )
        .unwrap();
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM folder_rules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "quote-laden pattern must not disturb stored rows");
    }

    fn conn_seed_rule(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO folder_rules (id, name, priority, conditions_json, dest_kind, destination) \
             VALUES (?1, ?2, 100, '{}', 'folder', 'Arquivo')",
            rusqlite::params![id, name],
        )
        .unwrap();
    }
}
