---
phase: 30-batch-trust-pair
verified: 2026-10-14T07:30:00Z
status: passed
score: 7/7 must-haves verified
covered_files: [".planning/phases/30-batch-trust-pair/30-01-PLAN.md", ".planning/phases/30-batch-trust-pair/30-01-SUMMARY.md", ".planning/phases/30-batch-trust-pair/30-02-PLAN.md", ".planning/phases/30-batch-trust-pair/30-02-SUMMARY.md", "src-tauri/src/classify/batch.rs", "src-tauri/src/classify/archive.rs", "src-tauri/src/classify/mod.rs", "src-tauri/src/commands/classify.rs", "src-tauri/src/lib.rs", "src/components/BatchPanel.tsx", "src/components/ArchivePanel.tsx", "src/components/BatchPreviewTable.tsx"]
covered_digest: "v1:sha256:57e30a77c4573b4ecba9df807af1988ffdcec5821bd7cbab6c392613efd0ecb0"
behavior_unverified: 0
overrides_applied: 0
---

# Phase 30: Batch Trust Pair Verification Report

**Phase Goal:** Whole-account batch operations become trustworthy — dry-run preview over the shared matcher with writes off plus a [Run for real] path (BATCH-03), retry-only-failures with exactly-once under stress (BATCH-04), and stale-preview re-resolution so a confirmed run never files the wrong mail.
**Verified:** 2026-10-14T07:30:00Z
**Status:** passed

## Goal Achievement

### Observable Truths

**30-01-1 — Dry-run batch preview with zero writes: VERIFIED.** `preview_plan(conn, Classify, …)` returns per-item `{message_id, from_folder, to_folder, dest_display, basis, skipped_reason}` with review-bucket skipped rows and remainder. `preview_writes_nothing` snapshots `batch_runs`/`batch_items`/`labels`/`override_events` counts across BOTH preview legs and asserts identical — green. Preview takes `&Connection` read-only paths, issues zero IMAP verbs (no SyncGate interaction by construction).

**30-01-2 — Archive preview through the SAME path with kind=archive: VERIFIED.** `preview_archive` calls the same `preview_plan` with `PreviewKind::Archive` (no second implementation — one function, one matcher each leg). Test asserts `kind == Archive`, `kind_tag == "archive"`, all items carry the `archive` basis and `Auto/Archive/` destinations.

**30-01-3 — Confirm re-collects fresh, verb takes only re-resolved pairs: VERIFIED.** Both `batch_classify` and `archive_run` loops call `batch::resolve_for_move` in the journal lock section; the MOVE verb receives only `(res.from_folder, res.uid)`. Destination shape shared via `dest_display_for` (real loop refactored onto it — preview/execute cannot diverge; `commit_counts_match_preview` pins parity).

**30-01-4 — Stale items dropped with surfaced count: VERIFIED.** `stale_preview_drop` shifts one location + one epoch: `stale_dropped == N`, verb transcript empty for stale UIDs, fresh control resolves identical pairs. `stale_dropped` counted at chunk + item level, carried in `totals_json`, parsed into `BatchReport.stale_dropped`.

**30-02-1 — Retry same run_id, moved=1 rows see zero verbs, no new run: VERIFIED.** `retry_skips_moved`: transcript holds exactly one MOVE per `moved=0` row, zero verbs mentioning `moved=1` ids, `batch_runs` count stays 1, `failed_count` reaches 0. `retry_failed` filters by `moved=0` intent rows before any verb (skip precedes verb).

**30-02-2 — Single-flight double-click + burst-EXISTS re-resolution: VERIFIED.** `double_click_retry_single_flight`: sequential re-claim refused while held, plus an 8-thread barrier race proving max concurrent holders == 1. `burst_exists_retry_skips_shifted`: mid-retry relocation skips with `retry_skipped == 1`, no verb for the shifted UID, row stays failed. Second `retry_failed` caller receives an `already-running` report (skip, never execution).

**30-02-3 — Preview table + [Run for real] + retry button + counts in both panels: VERIFIED.** Shared `BatchPreviewTable` rendered by BatchPanel (`preview_batch`) and ArchivePanel (`preview_archive`); confirm wired to the existing fresh-run commands (never replays rows); `Repetir N falhas` gated on `done`/`interrupted` + `failed > 0` (undone never offers retry); report shows stale-dropped + retry-skipped from backend truth. `tsc --noEmit` clean.

### Test Evidence

- `cargo test --lib classify::batch::` — 13/13 pass (4 pre-existing + 9 new Phase 30 legs).
- `cargo test --lib` — 434 passed, 0 failed, 1 ignored (no regressions).
- `cargo test --bins` — compiles green, 0 tests (live-gate bins untouched).
- `npx tsc --noEmit` — clean.
- Live legs (`live_preview_gate_skips_offline`, `live_retry_gate_skips_offline`) SKIP cleanly offline per the sandbox contract (no creds file, no network).

### Constraints Honored

BODY.PEEK-only (no new fetch paths — preview reads cached rows); UID-only addressing with UIDVALIDITY-first gating; single SessionManager session (verbs unchanged); no migration (failure derived from `moved=0`); `imap_outbox`/`send_queue` untouched; redaction asserted on basis strings (`looks_clean`); zero new deps (Cargo.toml unchanged); no SMTP/send changes; sandbox contract on live legs.
