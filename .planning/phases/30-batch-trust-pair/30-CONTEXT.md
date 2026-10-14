# Phase 30: Batch Trust Pair - Context

**Gathered:** 2026-10-14
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, batch proposals accepted)

<domain>
## Phase Boundary

Whole-account batch operations become trustworthy: (1) dry-run preview of batch/archive ops as a per-item plan over the shared matcher with `execute=false` and writes off, plus a [Run for real] path (BATCH-03); (2) retry-only-failures with writes re-on for failures while already-executed rows execute exactly once — burst-EXISTS + double-click-retry stress with no double-replay (BATCH-04); (3) stale previews re-resolve UIDs at confirm time so the confirmed run never files the wrong mail. Covers both `batch_classify` and `archive_run` (archive runs ARE batch runs per the single-journal invariant). Depends on Phase 29 (all triggers funneled through SyncGate with row-level outbox claims). UI in BatchPanel/ArchivePanel. Headless-first testing with fake-server transcripts; sandbox contract for any live legs (exit-2 SKIP, no creds file).

</domain>

<decisions>
## Implementation Decisions

### Dry-Run Preview (BATCH-03)
- Scope: one shared preview path serving BOTH `batch_classify` and `archive_run` — archive preview reuses the same per-item plan shape with `{"kind":"archive"}` tagging, never a second preview implementation
- Shared matcher: preview runs the identical candidate collection (`collect_candidates` / archive selector) and the identical decision function (`suggest_label` / `evaluate_rule`) as the real run — `execute=false` gates only the verb + journal writes, never a forked matcher
- Writes off: dry-run writes NOTHING — no `batch_runs` row, no `batch_items` rows, no label upserts, no `override_events` rows (Phase 29 `dry_run_writes_nothing` precedent); the plan is a transient return value, not persisted state
- Per-item plan shape: `message_id, from_folder, to_folder/dest_display, basis (label+confidence or matched rule id), skipped_reason (nullable — review-bucket, excluded, undatable)` — enough for the user to review what WOULD move where, including the `A Classificar` remainder count
- [Run for real] path: the confirm button does NOT replay the preview rows — it starts a real run that re-collects candidates fresh (stale-preview re-resolution applies), so preview and execute can never diverge silently

### Retry-Only-Failures (BATCH-04)
- Failure model: failed = `batch_items` intent rows with `moved=0` in a finished/interrupted run (journaled intent, no journaled fact); rows with `moved=1` are done and never re-executed; rows never journaled (candidate vanished mid-run) are re-collected, not resurrected
- Same-run retry: retry targets the SAME `run_id` (new attempt over the failed subset), reusing the `resume_run` machinery restricted to `moved=0` rows — no new run row, report accumulates on the original run
- Writes re-on, scoped: the retry flips writes back on ONLY for the failed subset; already-`moved=1` rows are skipped before any IMAP verb is issued (skip check precedes the verb, not after)
- Exactly-once under stress: double-click-retry is single-flighted at the run level (second caller sees running-retry and skips, SyncGate-style); burst-EXISTS during retry re-resolves location per row and skips rows whose UID/folder shifted instead of double-filing
- Retry entry condition: only runs in `done`/`interrupted` state with at least one `moved=0` row offer retry; `undone` runs never retry (undo already restored originals)

### Stale Preview Re-Resolution
- Staleness definition: a preview item is stale when, at confirm time, its `message_location` (folder+UID) differs from preview time OR its folder's `uid_validity` epoch differs from the preview-time epoch snapshot
- Re-resolve mechanism: confirm re-reads `message_location` + `mailboxes.uid_validity` per item immediately before journaling intent (same per-chunk epoch re-check pattern as Phase 20); stale items are dropped from the run with a surfaced skip count, never filed by stale UID
- UIDVALIDITY shift: folder-level epoch change aborts that folder's items to resync (Phase 20 precedent) — the run continues for unaffected folders and reports the dropped-folder count
- Wrong-mail guarantee: the MOVE verb always addresses the re-resolved (folder, UID) pair read in the same lock section as the journal write — a preview UID is never passed to the verb directly (structural, test-asserted)

### Integration & Testing
- Test strategy: headless `cargo test` only. Fake-server transcripts for: dry-run-writes-nothing (DB diff empty after preview), commit-matches-preview (same candidates, same destinations), retry-skips-moved (moved=1 rows see zero verbs), double-click-retry single-flight, burst-EXISTS re-resolution (shifted rows skipped, no double-file), stale-preview drop count
- UI integration: BatchPanel + ArchivePanel show the per-item preview table (destination + basis + skipped rows) with [Run for real]; finished runs with `moved=0` rows show a retry-failed button with the failure count; stale-drop and retry-skip counts surface in the run report
- SyncGate interplay: preview issues zero IMAP verbs (no gate interaction at all); real run + retry funnel through the existing SyncGate/outbox-claim path unchanged (Phase 29 invariant, no new gate)
- Live legs: sandbox contract only — exit-2 SKIP without creds, never create a creds file, self-only mail, pre-flight guard, post-run cleanup with `undo_batch`; live is a verification leg, never the correctness gate

### the agent's Discretion
- Exact preview command name (`preview_batch` vs `batch_preview` vs `dry_run` flag on existing commands) — planner's choice
- Whether failed rows carry an error-text column (forward migration) or derive failure from `moved=0` alone — planner's choice
- Preview row-count caps / pagination for very large accounts — planner's choice
- Exact fake-server transcript format for burst-EXISTS + double-click-retry stress — planner's choice

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/classify/batch.rs` — `collect_candidates`, `journal_intent`, `journal_moved`, `journaled_ids`, `BATCH_CHUNK=25`, `ensure_full_tree`, per-chunk UIDVALIDITY re-check (the exact patterns preview + retry + re-resolution extend)
- `src-tauri/src/classify/archive.rs` — archive selector (`resolve_included_folders`, `is_source_excluded`, `parse_date_utc`), `ARCHIVE_LABEL_ID`, `totals_json {"kind":"archive"}` tagging, shared undo
- `src-tauri/src/classify/rules.rs` — `evaluate_rule` shared matcher for the rules-driven preview leg
- `src-tauri/src/classify/rules_io.rs` — `dry_run_writes_nothing` + `import_commit_counts_match_dry_run` precedent (dry-run/commit parity pattern to mirror)
- `src-tauri/src/commands/classify.rs` — `batch_classify` (resume_run machinery retry reuses), `archive_run`, `undo_batch`, `batch_runs_list`
- `src-tauri/src/store/mod.rs` M16 — `batch_items(run_id, message_id, from_folder, to_folder, label_id, chunk, moved, undone)` journal with intent-vs-fact `moved` flag; M12 `batch_runs(state running|done|interrupted|undone, totals_json)`
- `src-tauri/src/sync/worker.rs` — `SyncGate` single-flight + `SyncWake` coalescing (retry double-click guard model)
- `src/components/BatchPanel.tsx` + `src/components/ArchivePanel.tsx` — existing progress/report/undo surfaces the preview + retry buttons extend

### Established Patterns
- Journal-before-move with intent-vs-fact `moved` flag (crash between journal and MOVE retries instead of double-moving)
- Forward-only migrations with preserve-rows tests; BODY.PEEK-only reads; UID-only addressing with UIDVALIDITY epoch gating
- Dry-run/commit parity (Phase 29 rules_io): identical validation, counts match, dry-run writes nothing
- Fake-server transcript tripwire tests (Phases 28/29); capability gating via `capability-profile.json`
- Fire-and-forget `override_events` logging (confirm/override/dismiss/batch) — preview logs nothing, real run + retry log normally
- Sandbox contract + exit-2 SKIP for all live legs; redaction sanitizer on every new surface

### Integration Points
- Phase 20 batch machinery → Phase 30 preview/verify/retry layer sits on top, verbs unchanged
- Phase 24 archive-as-batch (`{"kind":"archive"}`) → preview covers archive with the same code path
- Phase 28 `override_events` + CONDSTORE cheap pass → retry wakeups run the fast path; preview emits no events
- Phase 29 SyncGate funnel + row-level outbox claims → real run + retry ride unchanged; preview never touches the gate
- Phase 31 (Sieve/history/learning) consumes the `batch` action rows — retry must log with the same action + run_id so the dataset stays consistent

</code>

<specifics>
## Specific Ideas

- Mirror the Phase 29 rules_io dry-run contract verbatim: `dry_run_writes_nothing` DB-diff test + `commit_counts_match_dry_run` parity test, adapted to batch candidates/destinations
- Retry is `resume_run` restricted to `moved=0` rows on the same `run_id` — no new run-row or journal-table design needed
- Re-resolution reuses the Phase 20 per-chunk epoch re-check plus a per-item `message_location` re-read in the journal lock section
- BatchPanel/ArchivePanel preview table columns: from → to, basis (label+confidence / rule), skipped rows with reason; report gains stale-dropped + retry-skipped counts

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. Learning-from-batch-outcomes belongs to Phase 31 (LEARN-01 consumes the `batch` event rows this phase logs consistently).

</deferred>
