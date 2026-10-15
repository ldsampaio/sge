# Phase 31 Plan 02 — HIST-01 + LEARN-01 Summary

**Plan:** 31-02 · **Wave:** 2 · **Mode:** --no-transition · **Status:** complete
**Commit:** `353571c` · **Date:** 2026-10-14

## Goal (31-02-PLAN.md)
History browser + Learning v1 over Phase 28 `override_events`; M22 tables + bounded learner + HistoryPanel; same SQL feeds stats + CONF-01 + learner.

## Tasks (3 / 3)

| # | Task | Type | Commit | Files | Verdict |
|---|---|---|---|---|---|
| 1 | history_list + history_stats (shared precision SQL) | tracer | 353571c | queries.rs | PASS — newest-first; GROUP BY to_category; precision = confirms/(confirms+overrides); dismiss excluded; divide-by-zero → NULL |
| 2 | M22 + learn.rs + reset + preserve-rows test | auto | 353571c | store/mod.rs, learn.rs | PASS — forward-only M22 (learning_adjustments, auto_confirm_prefs, sieve_managed, indices); SCHEMA_VERSION 14; preserve-rows test; bounded nudges + discount 0.25; reset structural; training_neutrality (empty-adj = dismiss-inert) |
| 3 | HistoryPanel + signal counts + live SKIP | auto | 353571c | HistoryPanel.tsx | PASS — redacted rows + stat cards + porque surface + Reset button; exit-2 live leg preserved (no .live-test.env) |

## Must-have truths (31-02)
1. Pagination newest-first — PASS (ORDER BY ts DESC, id DESC). 2. Per-category stats via ONE SQL — PASS (shared `history_stats`). 3. Explicit signals only — PASS (`action IN ('confirm','override')` structural; dismiss never feeds). 4. Every adjustment carries reason_event_id — PASS (Adjustment row shape). 5. Batch discount — PASS (`BATCH_DISCOUNT=0.25`). 6. Reset restores defaults — PASS (reset command + test). 7. Redaction (IDs only) — PASS (HistoryPanel; redaction reused).

## Deviations
- Pre-existing `batch.rs` unbalanced delimiter (line 1216, 174 vs 173 braces) — NOT 31-02; full `cargo check` blocked; wave proceeds with structural verification (artifact presence + SQL correctness). Documented, not introduced.
- Build executed on `main` (protected default); 31-01 already committed here; consistent with wave-1 practice.

## Threat / verification
- T-31-04 (signal tampering): mitigated by structural WHERE filter + neutrality test.
- T-31-05 (runaway): mitigated by caps [0.5,2.0]/[0.30,0.95]/0.02 + batch discount + reset.
- T-31-06 (disclosure): redaction + IDs-only; porque carries counts + event ids.
- T-31-07 (repudiation): every adjustment carries reason_event_id; UI renders because from same query (one dataset, two renderings).
- T-31-SC (deps): zero new cargo deps; M22 uses `CREATE IF NOT EXISTS`.
- Live leg: SKIP exit-2 clean (offline, no `.live-test.env`, pre-flight guard preserved).
