//! Versioned category taxonomy (Phase 16, Plan 16-01).
//!
//! The default UTFPR pt-BR taxonomy ships embedded (`include_str!`, always
//! offline). Labels everywhere else reference category IDs — never names —
//! so renames (Phase 19) cannot orphan labels. `A Classificar` is a review
//! STATE, not a category: it never appears here, is never offered as a
//! classification choice, and is never a MOVE destination.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hard caps from the Laya accuracy pitfalls: flat `choice` questions lose
/// accuracy past ~20 options, and `choice:11+` hits the temperature-0.10
/// overconfidence bug. Top-level routing stays far below both.
pub const MAX_TOP_LEVEL: usize = 8;
/// Taxonomy depth cap: Top → Child → (optional) Sub. Deeper trees break the
/// `Auto/<Top>/<Child>` folder mapping (Phase 18).
pub const MAX_DEPTH: usize = 3;
/// The classifier tree root. Reserved: no category may be named `Auto`
/// (case-insensitive) or it would collide with user folders at Phase 18's
/// `ensure_auto_tree`.
pub const RESERVED_ROOT: &str = "auto";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub keywords: Vec<String>,
    pub rule: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Taxonomy {
    pub version: u32,
    pub name: String,
    pub categories: Vec<Category>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaxonomyError {
    #[error("categoria duplicada: id '{0}' aparece mais de uma vez")]
    DuplicateId(String),
    #[error("categoria '{0}' aponta para pai inexistente '{1}'")]
    MissingParent(String, String),
    #[error("ciclo detectado na hierarquia envolvendo '{0}'")]
    Cycle(String),
    #[error("nome duplicado entre irmãos: '{0}'")]
    DuplicateSiblingName(String),
    #[error("profundidade máxima excedida em '{0}' (limite: 3 níveis)")]
    TooDeep(String),
    #[error("categorias demais no topo: {0} (limite: 8)")]
    TooManyTopLevel(usize),
    #[error("categoria folha '{0}' precisa de palavras-chave")]
    LeafWithoutKeywords(String),
    #[error("nome reservado: '{0}' colide com a raiz Auto do classificador")]
    ReservedName(String),
    #[error("categoria '{0}' é seu próprio pai")]
    SelfParent(String),
}

/// Load + validate the shipped default taxonomy. Fails loudly (not empty
/// fallback): a corrupt embedded taxonomy is a build-time bug, and Phase 19
/// import validation reuses [`validate`].
pub fn load_default() -> Result<Taxonomy, TaxonomyError> {
    let tax: Taxonomy = serde_json::from_str(include_str!("taxonomy_default.json"))
        .expect("embedded default taxonomy must parse");
    validate(&tax)?;
    Ok(tax)
}

/// Validate structural invariants. Pure: reused by the Phase 19 importer on
/// user-supplied JSON (with plain-language errors in pt-BR).
pub fn validate(tax: &Taxonomy) -> Result<(), TaxonomyError> {
    let mut ids = HashSet::new();
    for cat in &tax.categories {
        if !ids.insert(cat.id.clone()) {
            return Err(TaxonomyError::DuplicateId(cat.id.clone()));
        }
        if cat.name.trim().eq_ignore_ascii_case(RESERVED_ROOT) {
            return Err(TaxonomyError::ReservedName(cat.name.clone()));
        }
        if cat.parent.as_deref() == Some(cat.id.as_str()) {
            return Err(TaxonomyError::SelfParent(cat.id.clone()));
        }
    }
    let by_id: HashMap<&str, &Category> =
        tax.categories.iter().map(|c| (c.id.as_str(), c)).collect();
    for cat in &tax.categories {
        if let Some(parent) = &cat.parent {
            if !by_id.contains_key(parent.as_str()) {
                return Err(TaxonomyError::MissingParent(
                    cat.id.clone(),
                    parent.clone(),
                ));
            }
        }
    }
    // Cycle + depth: walk each chain to the root.
    for cat in &tax.categories {
        let mut seen = HashSet::new();
        seen.insert(cat.id.as_str());
        let mut depth = 1usize;
        let mut cursor = cat.parent.as_deref();
        while let Some(pid) = cursor {
            if !seen.insert(pid) {
                return Err(TaxonomyError::Cycle(cat.id.clone()));
            }
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(TaxonomyError::TooDeep(cat.id.clone()));
            }
            cursor = by_id.get(pid).and_then(|c| c.parent.as_deref());
        }
    }
    // Sibling-name uniqueness.
    let mut sibling_names: HashMap<Option<&str>, HashSet<String>> = HashMap::new();
    for cat in &tax.categories {
        let key = cat.parent.as_deref();
        let lowered = cat.name.to_lowercase();
        if !sibling_names.entry(key).or_default().insert(lowered) {
            return Err(TaxonomyError::DuplicateSiblingName(cat.name.clone()));
        }
    }
    // Top-level cap.
    let tops = tax
        .categories
        .iter()
        .filter(|c| c.parent.is_none())
        .count();
    if tops > MAX_TOP_LEVEL {
        return Err(TaxonomyError::TooManyTopLevel(tops));
    }
    // Leaf children carry keywords (the keyword-resolution step needs them).
    let parents: HashSet<&str> = tax
        .categories
        .iter()
        .filter_map(|c| c.parent.as_deref())
        .collect();
    for cat in &tax.categories {
        let is_leaf = !parents.contains(cat.id.as_str());
        if is_leaf && cat.parent.is_some() && cat.keywords.is_empty() {
            return Err(TaxonomyError::LeafWithoutKeywords(cat.id.clone()));
        }
    }
    Ok(())
}

/// Top-level categories (routing choices for the classifier, Phase 17).
pub fn top_level<'a>(tax: &'a Taxonomy) -> Vec<&'a Category> {
    tax.categories.iter().filter(|c| c.parent.is_none()).collect()
}

/// Children of a category id.
pub fn children_of<'a>(tax: &'a Taxonomy, parent_id: &str) -> Vec<&'a Category> {
    tax.categories
        .iter()
        .filter(|c| c.parent.as_deref() == Some(parent_id))
        .collect()
}

/// Fold diacritics + lowercase for accent-insensitive matching
/// (pt-BR mail mixes "código"/"codigo", "avaliação"/"avaliacao").
pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// Score children by keyword hits in `text` (folded, substring scan).
/// Returns `(child_id, hits)` sorted by hits desc. Pure — reused by the
/// Phase 17 suggester for child keyword-resolution after the top-level
/// `choice` call.
pub fn match_keywords(text: &str, candidates: &[&Category]) -> Vec<(String, usize)> {
    let hay = fold(text);
    let mut scored: Vec<(String, usize)> = candidates
        .iter()
        .map(|c| {
            let hits = c
                .keywords
                .iter()
                .filter(|kw| {
                    let needle = fold(kw);
                    !needle.is_empty() && hay.contains(&needle[..])
                })
                .count();
            (c.id.clone(), hits)
        })
        .filter(|(_, hits)| *hits > 0)
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_valid() -> Taxonomy {
        Taxonomy {
            version: 1,
            name: "test".to_string(),
            categories: vec![
                Category {
                    id: "top".to_string(),
                    name: "Topo".to_string(),
                    parent: None,
                    keywords: vec![],
                    rule: String::new(),
                },
                Category {
                    id: "top.child".to_string(),
                    name: "Filha".to_string(),
                    parent: Some("top".to_string()),
                    keywords: vec!["chave".to_string()],
                    rule: String::new(),
                },
            ],
        }
    }

    #[test]
    fn default_loads_and_validates() {
        let tax = load_default();
        assert!(tax.is_ok());
        let tax = tax.unwrap();
        assert_eq!(tax.version, 1);
        assert_eq!(top_level(&tax).len(), 5);
    }

    #[test]
    fn default_ids_stable_snapshot() {
        let tax = load_default().unwrap();
        let mut ids: Vec<&str> = tax.categories.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "academico",
                "academico.aulas",
                "academico.avaliacoes",
                "academico.biblioteca",
                "academico.pesquisa",
                "academico.tcc",
                "administrativo",
                "administrativo.comunicados",
                "administrativo.documentos",
                "administrativo.matricula",
                "administrativo.ti",
                "comunidade",
                "comunidade.avisos",
                "comunidade.diretorio",
                "comunidade.listas",
                "financeiro",
                "financeiro.bolsas",
                "financeiro.cobrancas",
                "financeiro.reembolsos",
                "oportunidades",
                "oportunidades.cursos",
                "oportunidades.intercambio",
                "oportunidades.vagas",
            ]
        );
    }

    #[test]
    fn rejects_duplicate_id() {
        let mut tax = minimal_valid();
        tax.categories.push(Category {
            id: "top".to_string(),
            name: "Outra".to_string(),
            parent: None,
            keywords: vec![],
            rule: String::new(),
        });
        assert_eq!(validate(&tax), Err(TaxonomyError::DuplicateId("top".to_string())));
    }

    #[test]
    fn rejects_missing_parent() {
        let mut tax = minimal_valid();
        tax.categories[1].parent = Some("fantasma".to_string());
        assert_eq!(
            validate(&tax),
            Err(TaxonomyError::MissingParent(
                "top.child".to_string(),
                "fantasma".to_string()
            ))
        );
    }

    #[test]
    fn rejects_cycle() {
        let mut tax = minimal_valid();
        tax.categories[0].parent = Some("top.child".to_string());
        assert!(matches!(validate(&tax), Err(TaxonomyError::Cycle(_))));
    }

    #[test]
    fn rejects_self_parent() {
        let mut tax = minimal_valid();
        tax.categories[0].parent = Some("top".to_string());
        assert_eq!(validate(&tax), Err(TaxonomyError::SelfParent("top".to_string())));
    }

    #[test]
    fn rejects_duplicate_sibling_name() {
        let mut tax = minimal_valid();
        tax.categories.push(Category {
            id: "top.outra".to_string(),
            name: "FILHA".to_string(),
            parent: Some("top".to_string()),
            keywords: vec!["x".to_string()],
            rule: String::new(),
        });
        assert!(matches!(
            validate(&tax),
            Err(TaxonomyError::DuplicateSiblingName(_))
        ));
    }

    #[test]
    fn rejects_too_deep() {
        let mut tax = minimal_valid();
        tax.categories.push(Category {
            id: "top.child.grand".to_string(),
            name: "Neta".to_string(),
            parent: Some("top.child".to_string()),
            keywords: vec!["x".to_string()],
            rule: String::new(),
        });
        tax.categories.push(Category {
            id: "top.child.grand.great".to_string(),
            name: "Bisneta".to_string(),
            parent: Some("top.child.grand".to_string()),
            keywords: vec!["x".to_string()],
            rule: String::new(),
        });
        assert!(matches!(validate(&tax), Err(TaxonomyError::TooDeep(_))));
    }

    #[test]
    fn rejects_too_many_tops() {
        let tax = Taxonomy {
            version: 1,
            name: "t".to_string(),
            categories: (0..9)
                .map(|i| Category {
                    id: format!("top{i}"),
                    name: format!("Topo {i}"),
                    parent: None,
                    keywords: vec![],
                    rule: String::new(),
                })
                .collect(),
        };
        assert_eq!(validate(&tax), Err(TaxonomyError::TooManyTopLevel(9)));
    }

    #[test]
    fn rejects_leaf_without_keywords() {
        let mut tax = minimal_valid();
        tax.categories[1].keywords.clear();
        assert_eq!(
            validate(&tax),
            Err(TaxonomyError::LeafWithoutKeywords("top.child".to_string()))
        );
    }

    #[test]
    fn rejects_auto_name() {
        for name in ["Auto", "AUTO", "auto"] {
            let mut tax = minimal_valid();
            tax.categories[1].name = name.to_string();
            assert_eq!(
                validate(&tax),
                Err(TaxonomyError::ReservedName(name.to_string()))
            );
        }
    }

    #[test]
    fn keyword_match_folds_accents_and_case() {
        let tax = load_default().unwrap();
        let kids = children_of(&tax, "academico");
        let scored = match_keywords("PROVA final e GABARITO de calculo, codigo 123", &kids);
        assert!(!scored.is_empty());
        assert_eq!(scored[0].0, "academico.avaliacoes");
        // Accent-insensitive: "código" keyword matches "codigo", "avaliação" matches "AVALIAÇÃO"-less text.
        let scored2 = match_keywords("avaliação de desempenho e nota", &kids);
        assert_eq!(scored2[0].0, "academico.avaliacoes");
    }

    #[test]
    fn keyword_match_empty_on_no_hits() {
        let tax = load_default().unwrap();
        let kids = children_of(&tax, "financeiro");
        assert!(match_keywords("gato cachorro passarinho", &kids).is_empty());
    }
}
