//! Suggestion gate (Phase 17, Plan 17-02): confidence → primary vs
//! review, child keyword-resolution, runner-up secondary.
//!
//! Pure logic (no I/O): the worker calls [`suggest`] with the bridge
//! outcome + taxonomy children, persists the returned [`Suggestion`].
//! `A Classificar` is a REVIEW STATE — never a category id, never a MOVE
//! destination (Phase 18's confirm path refuses it structurally).

use super::taxonomy::{children_of, match_keywords, Category, Taxonomy};

/// Shipped default confidence threshold. Tuned on the pt-BR mini-eval at
/// plan verification; exposed as a user setting in Phase 18 (future
/// suggestions only — history is never relabeled).
pub const DEFAULT_THRESHOLD: f64 = 0.6;

/// Review-bucket marker. NOT a taxonomy id: it never appears in `labels`
/// as a primary (rows below threshold store the top choice + a
/// `needs_review` flag... — see below: they store the top choice WITH the
/// review state carried by the caller).
pub const REVIEW_BUCKET: &str = "A Classificar";

/// Transient justification line for the confirm surface (Phase 18).
/// Derived ONLY from matched keywords (never model prose, never content):
/// `Sinais: boleto, fatura`. Callers run it through `filter_output`
/// before display; it is never persisted.
pub fn justification(top_id: &str, evidence_text: &str, tax: &Taxonomy) -> String {
    let kids: Vec<&Category> = children_of(tax, top_id);
    let hits = match_keywords(evidence_text, &kids);
    let top_kw: Vec<String> = hits
        .into_iter()
        .take(3)
        .flat_map(|(id, _)| {
            kids.iter()
                .find(|c| c.id == id)
                .map(|c| c.keywords.iter().take(2).cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .take(4)
        .collect();
    if top_kw.is_empty() {
        "Classificação geral do conteúdo.".to_string()
    } else {
        format!("Sinais: {}.", top_kw.join(", "))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Top-level category id (the model's best guess, stored either way).
    pub primary_id: String,
    /// Child id after keyword-resolution (None when no child hits).
    pub child_id: Option<String>,
    /// Runner-up top-level id (local-only secondary, never moves).
    pub secondary_id: Option<String>,
    pub confidence: f64,
    /// False when routed to the review bucket.
    pub auto_routable: bool,
}

/// Route a bridge outcome to a suggestion.
///
/// - `confidence >= threshold` → routable primary (+ resolved child).
/// - Below threshold (or empty choice) → review: primary kept as the
///   model's guess, `auto_routable = false`, caller surfaces REVIEW_BUCKET.
/// - Secondary = runner-up top-level (never the primary, never moves).
/// - KEYWORD VETO (measured 2026-10-10: confident boundary misses like
///   `matrícula → Acadêmico 0.67`): when the chosen top's children score
///   ZERO keyword hits but the runner-up's children DO hit, the model's
///   confidence is distrusted and the mail routes to review. Cheap,
///   principled, and accountable in the tests below.
/// - A pinned override (Phase 18) wins over the model — enforced by the
///   CALLER via `latest_override` (this fn stays pure).
pub fn suggest(
    top_choice: &str,
    confidence: f64,
    runner_up: Option<&str>,
    tax: &Taxonomy,
    evidence_text: &str,
    threshold: f64,
) -> Suggestion {
    let top_kids: Vec<&Category> = children_of(tax, top_choice);
    let top_hits = match_keywords(evidence_text, &top_kids)
        .into_iter()
        .next()
        .map(|(_, h)| h)
        .unwrap_or(0);
    let child_id = if top_hits > 0 {
        match_keywords(evidence_text, &top_kids)
            .into_iter()
            .next()
            .map(|(id, _)| id)
    } else {
        None
    };
    let secondary_id = runner_up
        .filter(|r| *r != top_choice && !r.is_empty())
        .map(str::to_string);
    // Keyword veto: runner-up's family matches evidence, winner's doesn't.
    let vetoed = match &secondary_id {
        Some(runner) => {
            let run_kids: Vec<&Category> = children_of(tax, runner);
            let run_hits = match_keywords(evidence_text, &run_kids)
                .into_iter()
                .next()
                .map(|(_, h)| h)
                .unwrap_or(0);
            top_hits == 0 && run_hits > 0
        }
        None => false,
    };
    Suggestion {
        primary_id: top_choice.to_string(),
        child_id,
        secondary_id,
        confidence,
        auto_routable: !top_choice.is_empty() && confidence >= threshold && !vetoed,
    }
}

#[cfg(test)]
mod tests {
    use super::super::taxonomy::load_default;
    use super::*;

    #[test]
    fn confident_routes_with_child_and_secondary() {
        let tax = load_default().unwrap();
        let s = suggest(
            "financeiro",
            0.81,
            Some("academico"),
            &tax,
            "boleto vencido, fatura e pagamento",
            DEFAULT_THRESHOLD,
        );
        assert!(s.auto_routable);
        assert_eq!(s.primary_id, "financeiro");
        assert_eq!(s.child_id.as_deref(), Some("financeiro.cobrancas"));
        assert_eq!(s.secondary_id.as_deref(), Some("academico"));
    }

    #[test]
    fn low_confidence_routes_to_review_but_keeps_guess() {
        let tax = load_default().unwrap();
        let s = suggest("academico", 0.42, Some("comunidade"), &tax, "texto vago", 0.6);
        assert!(!s.auto_routable);
        assert_eq!(s.primary_id, "academico");
    }

    #[test]
    fn keyword_veto_catches_confident_boundary_miss() {
        // Measured 2026-10-10: `matrícula → Acadêmico 0.67`. No academic
        // child keyword hits, but administrativo.matricula hits → review.
        let tax = load_default().unwrap();
        let s = suggest(
            "academico",
            0.672,
            Some("administrativo"),
            &tax,
            "Rematrícula pelo portal do aluno até sexta. Veteranos pelo derac.",
            DEFAULT_THRESHOLD,
        );
        assert!(!s.auto_routable, "veto must fire: {s:?}");
        assert_eq!(s.primary_id, "academico");
        assert_eq!(s.secondary_id.as_deref(), Some("administrativo"));
    }

    #[test]
    fn keyword_veto_stays_quiet_when_winner_matches() {
        // Genuine Financeiro with finance keywords: no veto despite runner-up.
        let tax = load_default().unwrap();
        let s = suggest(
            "financeiro",
            0.81,
            Some("academico"),
            &tax,
            "boleto vencido, fatura e pagamento",
            DEFAULT_THRESHOLD,
        );
        assert!(s.auto_routable);
    }

    #[test]
    fn secondary_never_echoes_primary() {
        let tax = load_default().unwrap();
        let s = suggest("a", 0.9, Some("a"), &tax, "t", 0.6);
        assert_eq!(s.secondary_id, None);
        let s2 = suggest("a", 0.9, None, &tax, "t", 0.6);
        assert_eq!(s2.secondary_id, None);
    }

    #[test]
    fn review_bucket_is_not_a_category() {
        // Structural: the taxonomy must never contain the bucket, so no
        // caller can file mail "into" review.
        let tax = load_default().unwrap();
        assert!(tax.categories.iter().all(|c| c.name != REVIEW_BUCKET));
        assert!(tax.categories.iter().all(|c| c.id != REVIEW_BUCKET));
    }
}
