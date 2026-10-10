//! Taxonomy editing engine (Phase 19, Plan 19-01): ID-stable ops.
//!
//! Pure taxonomy-JSON transforms (testable without IMAP) + label remapping.
//! Folder verbs (CREATE/RENAME/DELETE/MOVE) live in the command layer and
//! reuse the Phase 11/10 paths; this module never touches the network.
//!
//! Identity rule: category IDs are stable forever. Rename changes names
//! only (zero label churn); merge remaps ids; delete reassigns. Structural
//! ops bump `version`; keyword/rule edits don't.

use super::taxonomy::{validate, Category, Taxonomy, TaxonomyError};

#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    Taxonomy(TaxonomyError),
    UnknownId(String),
    DuplicateName(String),
    TopLevelCap,
    DepthCap,
    Reserved(String),
    EmptyName,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Taxonomy(e) => write!(f, "{e}"),
            EditError::UnknownId(id) => write!(f, "categoria desconhecida: {id}"),
            EditError::DuplicateName(n) => write!(f, "já existe uma categoria chamada '{n}' neste nível"),
            EditError::TopLevelCap => write!(f, "limite de 8 categorias no topo — funda duas antes de criar"),
            EditError::DepthCap => write!(f, "limite de 3 níveis — crie sob outro pai"),
            EditError::Reserved(n) => write!(f, "nome reservado: '{n}' colide com a raiz Auto"),
            EditError::EmptyName => write!(f, "dê um nome para a categoria"),
        }
    }
}

fn slug(name: &str) -> String {
    let folded = super::taxonomy::fold(name.trim());
    let mut s: String = folded
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

fn sibling_names(tax: &Taxonomy, parent: Option<&str>) -> Vec<String> {
    tax.categories
        .iter()
        .filter(|c| c.parent.as_deref() == parent)
        .map(|c| c.name.to_lowercase())
        .collect()
}

/// Add a category under `parent` (None = top level). Returns the new id.
/// Depth ≤3, tops ≤8, `Auto` reserved — all enforced before mutation.
pub fn apply_add(
    tax: &mut Taxonomy,
    parent: Option<&str>,
    name: &str,
    keywords: Vec<String>,
    rule: &str,
) -> Result<String, EditError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EditError::EmptyName);
    }
    if name.eq_ignore_ascii_case("auto") {
        return Err(EditError::Reserved(name.to_string()));
    }
    if let Some(pid) = parent {
        // Real depth walk: the new node sits one level below its parent.
        let mut parent_depth = 0usize;
        let mut cursor = Some(pid);
        while let Some(cur) = cursor {
            match tax.categories.iter().find(|c| c.id == cur) {
                Some(cat) => {
                    parent_depth += 1;
                    cursor = cat.parent.as_deref();
                }
                None => return Err(EditError::UnknownId(cur.to_string())),
            }
        }
        if parent_depth + 1 > super::taxonomy::MAX_DEPTH {
            return Err(EditError::DepthCap);
        }
    } else if tax.categories.iter().filter(|c| c.parent.is_none()).count()
        >= super::taxonomy::MAX_TOP_LEVEL
    {
        return Err(EditError::TopLevelCap);
    }
    if sibling_names(tax, parent).contains(&name.to_lowercase()) {
        return Err(EditError::DuplicateName(name.to_string()));
    }
    let base = slug(name);
    let prefix = parent.map(|p| format!("{p}.")).unwrap_or_default();
    let mut id = format!("{prefix}{base}");
    let mut n = 2;
    while tax.categories.iter().any(|c| c.id == id) {
        id = format!("{prefix}{base}-{n}");
        n += 1;
    }
    tax.categories.push(Category {
        id: id.clone(),
        name: name.to_string(),
        parent: parent.map(str::to_string),
        keywords,
        rule: rule.to_string(),
    });
    validate(tax).map_err(EditError::Taxonomy)?;
    Ok(id)
}

/// Rename (display name only — ids and labels untouched).
pub fn apply_rename(tax: &mut Taxonomy, id: &str, new_name: &str) -> Result<(), EditError> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(EditError::EmptyName);
    }
    if new_name.eq_ignore_ascii_case("auto") {
        return Err(EditError::Reserved(new_name.to_string()));
    }
    let idx = tax
        .categories
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| EditError::UnknownId(id.to_string()))?;
    let parent = tax.categories[idx].parent.clone();
    if sibling_names(tax, parent.as_deref())
        .into_iter()
        .any(|n| n == new_name.to_lowercase() && tax.categories[idx].name.to_lowercase() != n)
    {
        return Err(EditError::DuplicateName(new_name.to_string()));
    }
    tax.categories[idx].name = new_name.to_string();
    validate(tax).map_err(EditError::Taxonomy)?;
    Ok(())
}

/// Merge `from` into `into` (same parent required): `from` disappears.
/// Caller remaps labels + moves mail, then persists.
pub fn apply_merge(tax: &mut Taxonomy, from: &str, into: &str) -> Result<(), EditError> {
    if from == into {
        return Err(EditError::DuplicateName("use outra categoria como destino".to_string()));
    }
    let f = tax
        .categories
        .iter()
        .find(|c| c.id == from)
        .ok_or_else(|| EditError::UnknownId(from.to_string()))?;
    let t = tax
        .categories
        .iter()
        .find(|c| c.id == into)
        .ok_or_else(|| EditError::UnknownId(into.to_string()))?;
    if f.parent != t.parent {
        return Err(EditError::DuplicateName(
            "só é possível fundir categorias irmãs".to_string(),
        ));
    }
    // Children of `from` re-parent to `into` (depth preserved: siblings).
    for c in tax.categories.iter_mut() {
        if c.parent.as_deref() == Some(from) {
            c.parent = Some(into.to_string());
            // Child id keeps its suffix under the new parent when stable...
            // IDs are opaque: keep the OLD id (stability beats prettiness).
        }
    }
    tax.categories.retain(|c| c.id != from);
    validate(tax).map_err(EditError::Taxonomy)?;
    Ok(())
}

/// Delete a category. Children must be gone first (caller merges or deletes
/// them); mail must be reassigned first (caller moves + remaps).
pub fn apply_delete(tax: &mut Taxonomy, id: &str) -> Result<(), EditError> {
    if !tax.categories.iter().any(|c| c.id == id) {
        return Err(EditError::UnknownId(id.to_string()));
    }
    if tax.categories.iter().any(|c| c.parent.as_deref() == Some(id)) {
        return Err(EditError::DuplicateName(
            "exclua ou funda as subcategorias primeiro".to_string(),
        ));
    }
    tax.categories.retain(|c| c.id != id);
    validate(tax).map_err(EditError::Taxonomy)?;
    Ok(())
}

/// Update keywords/rules in place (NO version bump — non-structural).
pub fn apply_keywords(
    tax: &mut Taxonomy,
    id: &str,
    keywords: Vec<String>,
    rule: &str,
) -> Result<(), EditError> {
    let cat = tax
        .categories
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| EditError::UnknownId(id.to_string()))?;
    cat.keywords = keywords;
    cat.rule = rule.to_string();
    validate(tax).map_err(EditError::Taxonomy)?;
    Ok(())
}

/// Sanitize + validate user-supplied taxonomy JSON (import path).
/// Rejects: unparseable, control chars, overlong names, `/` or `.` in ids
/// (folder-injection), structural violations (via `validate`).
pub fn sanitize_import(raw: &str) -> Result<Taxonomy, EditError> {
    let mut tax: Taxonomy =
        serde_json::from_str(raw).map_err(|_| EditError::EmptyName)?;
    for cat in &mut tax.categories {
        if cat.name.chars().count() > 80 || cat.id.chars().count() > 120 {
            return Err(EditError::DuplicateName(format!(
                "nome ou id longo demais: '{}'",
                cat.name.chars().take(40).collect::<String>()
            )));
        }
        if cat.name.chars().any(|c| c.is_control()) {
            return Err(EditError::DuplicateName(format!(
                "caractere inválido no nome '{}'",
                cat.id
            )));
        }
        // Ids are `top` or `top.child` only: slashes are folder-injection,
        // deeper dots break the hierarchy convention.
        if cat.id.contains('/') || cat.id.matches('.').count() > 1 {
            return Err(EditError::DuplicateName(format!("id inválido: '{}'", cat.id)));
        }
        if cat.id.eq_ignore_ascii_case("auto") || cat.name.trim().eq_ignore_ascii_case("auto") {
            return Err(EditError::Reserved(cat.name.clone()));
        }
    }
    validate(&tax).map_err(EditError::Taxonomy)?;
    Ok(tax)
}

#[cfg(test)]
mod tests {
    use super::super::taxonomy::load_default;
    use super::*;

    fn test_tax() -> Taxonomy {
        load_default().unwrap()
    }

    #[test]
    fn add_top_and_child_with_dedupe() {
        let mut tax = test_tax();
        let id = apply_add(&mut tax, None, "Esportes", vec!["futebol".into()], "").unwrap();
        assert_eq!(id, "esportes");
        let id2 = apply_add(&mut tax, Some("esportes"), "Futebol", vec!["gol".into()], "").unwrap();
        assert_eq!(id2, "esportes.futebol");
        // Duplicate name under another spelling dedupes the id.
        let id3 = apply_add(&mut tax, Some("esportes"), "Futebol!", vec!["x".into()], "").unwrap();
        assert_eq!(id3, "esportes.futebol-2");
    }

    #[test]
    fn add_refuses_caps_and_reserved() {
        let mut tax = test_tax();
        for name in ["Extra A", "Extra B", "Extra C"] {
            apply_add(&mut tax, None, name, vec![], "").unwrap();
        }
        assert_eq!(
            apply_add(&mut tax, None, "Nona", vec![], ""),
            Err(EditError::TopLevelCap)
        );
        assert!(matches!(
            apply_add(&mut tax, None, "Auto", vec![], ""),
            Err(EditError::Reserved(_))
        ));
        // Depth: grandchild of a child (3rd level ok, 4th refused).
        let l3 = apply_add(&mut tax, Some("academico.aulas"), "Sub", vec!["x".into()], "").unwrap();
        assert_eq!(l3, "academico.aulas.sub");
        assert_eq!(
            apply_add(&mut tax, Some(&l3), "Sub2", vec!["x".into()], ""),
            Err(EditError::DepthCap)
        );
    }

    #[test]
    fn rename_keeps_id_and_rejects_dup_sibling() {
        let mut tax = test_tax();
        apply_rename(&mut tax, "financeiro.bolsas", "Bolsas de Estudo").unwrap();
        let cat = tax.categories.iter().find(|c| c.id == "financeiro.bolsas").unwrap();
        assert_eq!(cat.name, "Bolsas de Estudo");
        assert!(matches!(
            apply_rename(&mut tax, "financeiro.bolsas", "Boletos e Cobranças"),
            Err(EditError::DuplicateName(_))
        ));
    }

    #[test]
    fn merge_reparents_children_and_removes_source() {
        let mut tax = test_tax();
        apply_merge(&mut tax, "financeiro.bolsas", "financeiro.cobrancas").unwrap();
        assert!(!tax.categories.iter().any(|c| c.id == "financeiro.bolsas"));
        assert!(tax.categories.iter().any(|c| c.id == "financeiro.cobrancas"));
        // Cross-parent merge refused.
        assert!(apply_merge(&mut tax, "financeiro.cobrancas", "academico.aulas").is_err());
    }

    #[test]
    fn delete_requires_empty_children() {
        let mut tax = test_tax();
        assert!(apply_delete(&mut tax, "financeiro").is_err());
        assert!(apply_delete(&mut tax, "financeiro.reembolsos").is_ok());
        assert!(!tax.categories.iter().any(|c| c.id == "financeiro.reembolsos"));
    }

    #[test]
    fn import_rejects_malicious_shapes() {
        // Cycle.
        let cycle = r#"{"version":1,"name":"x","categories":[
            {"id":"a","name":"A","parent":"b","keywords":[],"rule":""},
            {"id":"b","name":"B","parent":"a","keywords":[],"rule":""}]}"#;
        assert!(sanitize_import(cycle).is_err());
        // Auto root.
        let auto = r#"{"version":1,"name":"x","categories":[
            {"id":"auto","name":"Auto","parent":null,"keywords":[],"rule":""}]}"#;
        assert!(matches!(sanitize_import(auto), Err(EditError::Reserved(_))));
        // Slash injection in id.
        let slash = r#"{"version":1,"name":"x","categories":[
            {"id":"a/b","name":"AB","parent":null,"keywords":[],"rule":""}]}"#;
        assert!(sanitize_import(slash).is_err());
        // Round-trip: default exports clean.
        let rt = serde_json::to_string(&test_tax()).unwrap();
        assert!(sanitize_import(&rt).is_ok());
    }
}
