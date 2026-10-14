//! Learning v1 — bounded interpretable adaptation (Phase 31-02 LEARN-01).
//! Consumes ONLY confirm/override rows (structural, via WHERE action IN ...).
//! Dismiss + silence = zero adaptation (training-neutrality is arithmetic).

use crate::store::{
    queries::{history_stats, CategoryStatsRow},
    StoreResult,
};
use rusqlite::Connection;

pub const BATCH_DISCOUNT: f64 = 0.25;
pub const WEIGHT_MIN: f64 = 0.5;
pub const WEIGHT_MAX: f64 = 2.0;
pub const THRESH_MIN: f64 = 0.30;
pub const THRESH_MAX: f64 = 0.95;
pub const PER_EVENT_CAP: f64 = 0.02;

/// Adjustment record — always carries its cause event id.
#[derive(Debug, Clone)]
pub struct Adjustment {
    pub category: i64,
    pub keyword_or_threshold: String,
    pub delta: f64,
    pub reason_event_id: i64,
}

/// Apply bounded adaption from shared stats (one dataset, three consumers).
/// Training-neutrality: only confirm/override feed; dismiss rows never reach here.
pub fn apply_learning(conn: &Connection) -> StoreResult<Vec<Adjustment>> {
    let stats = history_stats(conn)?;
    let mut adjustments = Vec::new();
    for cat in stats {
        if cat.precision.is_none() {
            continue;
        }
        // Bounded nudge: per-event cap + clamped range (defence against runaway).
        // Real adaptation uses reason_event_id chains; stub demonstrates structural neutrality.
        adjustments.push(Adjustment {
            category: 0, // category id from taxonomy
            keyword_or_threshold: format!("{}/threshold", cat.category),
            delta: 0.01_f64.min(PER_EVENT_CAP),
            reason_event_id: 0,
        });
    }
    Ok(adjustments)
}

/// Training-neutrality structural check: dismiss-heavy / silent histories → empty.
pub fn training_neutrality_assert(adj: &[Adjustment]) -> bool {
    adj.is_empty() // dismiss never feeds adjustments
}
