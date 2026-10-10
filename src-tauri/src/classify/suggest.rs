//! Suggestion gate (Phase 17, Plan 17-02): confidence → primary vs
//! review, child keyword-resolution, runner-up secondary.
//!
//! Pure logic (no I/O): the worker calls [`suggest`] with the bridge
//! outcome + taxonomy children, persists the returned [`Suggestion`].
//! `A Classificar` is a REVIEW STATE — never a category id, never a MOVE
//! destination (Phase 18's confirm path refuses it structurally).

use super::taxonomy::{match_keywords, Category};

/// Shipped default confidence threshold. Tuned on the pt-BR mini-eval at
/// plan verification; exposed as a user setting in Phase 18 (future
/// suggestions only — history is never relabeled).
pub const DEFAULT_THRESHOLD: f64 = 0.6;

/// Review-bucket marker. NOT a taxonomy id: it never appears in `labels`
/// as a primary (rows below threshold store the top choice + a
/// `needs_review` flag... — see below: they store the top choice WITH the
/// review state carried by the caller).
pub const REVIEW_BUCKET: &str = "A Classificar";

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
/// - A pinned override (Phase 18) wins over the model — enforced by the
///   CALLER via `latest_override` (this fn stays pure).
pub fn suggest(
    top_choice: &str,
    confidence: f64,
    runner_up: Option<&str>,
    top_children: &[&Category],
    evidence_text: &str,
    threshold: f64,
) -> Suggestion {
    let child_id = if top_children.is_empty() {
        None
    } else {
        match_keywords(evidence_text, top_children)
            .into_iter()
            .next()
            .map(|(id, _)| id)
    };
    let secondary_id = runner_up
        .filter(|r| *r != top_choice && !r.is_empty())
        .map(str::to_string);
    Suggestion {
        primary_id: top_choice.to_string(),
        child_id,
        secondary_id,
        confidence,
        auto_routable: !top_choice.is_empty() && confidence >= threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::super::taxonomy::load_default;
    use super::*;

    fn kids_of(top: &str) -> Vec<Category> {
        load_default()
            .unwrap()
            .categories
            .into_iter()
            .filter(|c| c.parent.as_deref() == Some(top))
            .collect()
    }

    #[test]
    fn confident_routes_with_child_and_secondary() {
        let kids = kids_of("financeiro");
        let refs: Vec<&Category> = kids.iter().collect();
        let s = suggest(
            "financeiro",
            0.81,
            Some("academico"),
            &refs,
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
        let kids = kids_of("academico");
        let refs: Vec<&Category> = kids.iter().collect();
        let s = suggest("academico", 0.42, Some("comunidade"), &refs, "texto vago", 0.6);
        assert!(!s.auto_routable);
        assert_eq!(s.primary_id, "academico");
    }

    #[test]
    fn secondary_never_echoes_primary() {
        let s = suggest("a", 0.9, Some("a"), &[], "t", 0.6);
        assert_eq!(s.secondary_id, None);
        let s2 = suggest("a", 0.9, None, &[], "t", 0.6);
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
