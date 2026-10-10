//! Classify worker (Phase 17, Plan 17-01): behind-sync queue drain.
//!
//! The worker NEVER blocks sync: the post-sync hook only enqueues;
//! draining happens here on a detached task under [`ClassifyGate`]
//! (single-flight, mirrors `SyncGate`). Sidecar down → rows stay `pending`
//! and sync is unaffected.
//!
//! Crash safety: rows marked `processing` at boot reset to `pending`
//! ([`reap_processing`]); UIDVALIDITY shifts drop the folder's rows
//! ([`Store`](crate::store::queries) epoch helper).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use super::bridge::{Bridge, BridgeError};
use super::suggest::{suggest, DEFAULT_THRESHOLD};
use super::taxonomy::{children_of, load_default, top_level};
use crate::store::queries;
use crate::store::Store;

/// Single-flight drain guard: at most one drain loop runs at a time, so a
/// poll tick firing mid-drain skips instead of overlapping (mirrors
/// `SyncGate` exactly).
#[derive(Debug, Default)]
pub struct ClassifyGate {
    running: AtomicBool,
}

impl ClassifyGate {
    pub fn try_acquire(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn release(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Per-message classification event payload (Phase 17 UI surface + Phase 18
/// badges). IDs + scores only — never content.
#[derive(Debug, Clone, Serialize)]
pub struct ClassificationReady {
    pub message_id: u64,
    pub primary_id: String,
    pub child_id: Option<String>,
    pub secondary_id: Option<String>,
    pub confidence: f64,
    pub needs_review: bool,
}

/// Classify one queued message end-to-end: evidence → bridge → gate →
/// label upsert. Returns the event payload on success.
///
/// Lock discipline: `store` is touched only in two short sync sections
/// (fetch happens via the passed fields, upsert at the end) — the Mutex
/// guard is NEVER held across the bridge `.await`.
pub async fn classify_one(
    store: &Store,
    bridge: &Bridge,
    message_id: u64,
    subject: &str,
    snippet: &str,
    from_domain: &str,
    has_attachments: bool,
    is_reply: bool,
    threshold: f64,
) -> Result<ClassificationReady, BridgeError> {
    let ready = suggest_label(
        bridge,
        subject,
        snippet,
        from_domain,
        has_attachments,
        is_reply,
        threshold,
    )
    .await?;
    persist_label(
        store,
        message_id,
        &ready.primary_id,
        ready.child_id.as_deref(),
        ready.secondary_id.as_deref(),
        ready.confidence,
        threshold,
    )
    .map_err(BridgeError::Io)?;
    Ok(ClassificationReady { message_id, ..ready })
}

/// Bridge round-trip without any store access (pure async step).
pub async fn suggest_label(
    bridge: &Bridge,
    subject: &str,
    snippet: &str,
    from_domain: &str,
    has_attachments: bool,
    is_reply: bool,
    threshold: f64,
) -> Result<ClassificationReady, BridgeError> {
    let evidence = super::evidence::build_evidence(
        subject,
        snippet,
        from_domain,
        has_attachments,
        is_reply,
    );
    let tax = load_default().map_err(|e| BridgeError::Io(e.to_string()))?;
    let tops = top_level(&tax);
    let mut criteria = std::collections::HashMap::new();
    for t in &tops {
        criteria.insert(t.name.clone(), Some(t.rule.clone()));
    }
    let outcome = bridge
        .choice(
            &evidence,
            "top",
            "Classifique este e-mail em UMA categoria principal.",
            &criteria,
            Some(1024),
        )
        .await?;
    // Map the model's NAME answer back to the taxonomy id.
    let top_id = tops
        .iter()
        .find(|t| t.name == outcome.choice)
        .map(|t| t.id.clone())
        .unwrap_or_else(|| outcome.choice.clone());
    let kids: Vec<&super::taxonomy::Category> = children_of(&tax, &top_id);
    let runner_name = outcome.runner_up();
    let runner_id = runner_name.as_ref().and_then(|n| {
        tops.iter()
            .find(|t| &t.name == n)
            .map(|t| t.id.clone())
    });
    let s = suggest(
        &top_id,
        outcome.confidence,
        runner_id.as_deref(),
        &kids,
        &evidence,
        threshold,
    );
    Ok(ClassificationReady {
        message_id: 0, // filled by classify_one / drain
        primary_id: s.primary_id,
        child_id: s.child_id,
        secondary_id: s.secondary_id,
        confidence: s.confidence,
        needs_review: !s.auto_routable,
    })
}

/// Persist a suggestion (short sync section — caller holds the lock only
/// for this call, never across network awaits).
pub fn persist_label(
    store: &Store,
    message_id: u64,
    primary_id: &str,
    child_id: Option<&str>,
    secondary_id: Option<&str>,
    confidence: f64,
    threshold: f64,
) -> Result<(), String> {
    let _ = child_id; // child rides inside primary tree path (Phase 18 folders)
    queries::upsert_label(
        store.conn(),
        message_id,
        primary_id,
        secondary_id,
        confidence,
        threshold,
    )
    .map_err(|e| e.to_string())
}

/// Drain up to `limit` pending rows. Returns (labeled, still_pending).
/// Failures stay queued (status back to pending) — the next drain retries
/// with backoff; the queue never loses rows to a transient sidecar outage.
///
/// `store` is `Arc<Mutex<..>>`-friendly: every lock is a short sync
/// section, none held across bridge awaits.
pub async fn drain(
    store: &Arc<std::sync::Mutex<Store>>,
    bridge: &Bridge,
    gate: &ClassifyGate,
    fetch: &(dyn Fn(u64) -> Option<(String, String, String, bool, bool)> + Send + Sync),
    limit: usize,
    threshold: f64,
) -> (usize, usize) {
    if !gate.try_acquire() {
        return (0, 0);
    }
    let result = drain_inner(store, bridge, fetch, limit, threshold).await;
    gate.release();
    result
}

async fn drain_inner(
    store: &Arc<std::sync::Mutex<Store>>,
    bridge: &Bridge,
    fetch: &(dyn Fn(u64) -> Option<(String, String, String, bool, bool)> + Send + Sync),
    limit: usize,
    threshold: f64,
) -> (usize, usize) {
    let rows: Vec<(u64, String)> = {
        let guard = store.lock().unwrap();
        queries::dequeue_classify(guard.conn(), limit).unwrap_or_default()
    };
    let mut labeled = 0usize;
    for (message_id, folder) in rows {
        let Some((subject, snippet, domain, atts, reply)) = fetch(message_id) else {
            let guard = store.lock().unwrap();
            let _ = queries::set_queue_state(guard.conn(), message_id, &folder, "done");
            continue;
        };
        match suggest_label(bridge, &subject, &snippet, &domain, atts, reply, threshold).await
        {
            Ok(ready) => {
                let guard = store.lock().unwrap();
                let ok = worker_persist(&guard, message_id, &ready, threshold).is_ok()
                    && queries::set_queue_state(guard.conn(), message_id, &folder, "done")
                        .is_ok();
                if ok {
                    labeled += 1;
                }
            }
            Err(_) => {
                // Transient (down/timeout/500): back to pending for retry.
                let guard = store.lock().unwrap();
                let _ = queries::set_queue_state(guard.conn(), message_id, &folder, "pending");
            }
        }
    }
    let pending = {
        let guard = store.lock().unwrap();
        queries::queue_depth(guard.conn()).unwrap_or(0) as usize
    };
    (labeled, pending)
}

fn worker_persist(
    store: &Store,
    message_id: u64,
    ready: &ClassificationReady,
    threshold: f64,
) -> Result<(), String> {
    persist_label(
        store,
        message_id,
        &ready.primary_id,
        ready.child_id.as_deref(),
        ready.secondary_id.as_deref(),
        ready.confidence,
        threshold,
    )
}

/// Reset `processing` rows to `pending` at boot (crash safety).
pub fn reap_processing(store: &Store) -> usize {
    let conn = store.conn();
    conn.execute(
        "UPDATE classify_queue SET status = 'pending' WHERE status = 'processing'",
        [],
    )
    .unwrap_or(0) as usize
}

#[allow(dead_code)]
fn _gate_type_assert(gate: Arc<ClassifyGate>) -> Arc<ClassifyGate> {
    gate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::queries::*;

    #[test]
    fn gate_is_single_flight() {
        let g = ClassifyGate::default();
        assert!(g.try_acquire());
        assert!(!g.try_acquire());
        g.release();
        assert!(g.try_acquire());
    }

    #[test]
    fn reap_resets_processing_rows() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb = ensure_mailbox(conn, "INBOX").unwrap();
        upsert_message(conn, mb, 1, None, "s", "f", "[]", "[]",
            "2026-10-10T00:00:00Z", "[]", false, "").unwrap();
        let mid = find_message_id(conn, mb, 1).unwrap().unwrap();
        enqueue_classify(conn, mid, "INBOX", 1).unwrap();
        conn.execute("UPDATE classify_queue SET status = 'processing'", []).unwrap();
        assert_eq!(reap_processing(&store), 1);
        assert_eq!(queue_depth(conn).unwrap(), 1);
    }
}
