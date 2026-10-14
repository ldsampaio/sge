---
phase: 30
fixed_at: 2026-10-14T15:30:00Z
review_path: .planning/phases/30-batch-trust-pair/30-REVIEW.md
iteration: 1
findings_in_scope: 11
fixed: 11
skipped: 0
status: all_fixed
---

# Phase 30: Code Review Fix Report

**Fixed at:** 2026-10-14T15:30:00Z
**Source review:** `.planning/phases/30-batch-trust-pair/30-REVIEW.md`
**Iteration:** 1

**Summary:**
- Findings in scope: 11 (7 Warning + 4 Info)
- Fixed: 11
- Skipped: 0

## Verification
- `cargo check --lib` passes (library compiled; Tauri binary resource `binaries/sge-laya-x86_64-unknown-linux-gnu` missing is a pre-existing build-env issue unrelated to fixes).
- `npx tsc --noEmit` on `BatchPanel.tsx` clean (edit verified).
- Regression tests added for WR-01/WR-02/WR-03/WR-06/WR-07 (structural assertions on totals honesty, wire-canonical decode, resolve-verb atomicity, transactional merge + claim guard, RAII drop-guard).
- Sandbox constraints respected: BODY.PEEK-only held, UID-only + UIDVALIDITY-first gating preserved, zero new dependencies, no SMTP/send, no .live-test.env, no migration, redaction untouched.

## Fixed Issues

### WR-01: Retry totals honesty
- **Files:** `src-tauri/src/commands/classify.rs`
- **Commit:** `7a019a4`
- **Fix:** Stale/vanished branch now continues after `moved += 1` (already-filed) instead of falling through to `skipped += 1`; already-at-destination branch removed `skipped += 1`; retry now counts only verified recoveries.

### WR-02: Wire-canonical already-filed detection
- **Files:** `src-tauri/src/commands/classify.rs`
- **Commit:** `7a019a4`
- **Fix:** Compare decoded display (`decode_modified_utf7`) on both `res.from_folder` and `item.to_folder`; same for `cur_folder == item.to_folder`; `archive_one` gets decoded `dest_display` instead of wire string.

### WR-03: Resolve + journal unified lock
- **Files:** `src-tauri/src/commands/classify.rs`, `src-tauri/src/classify/batch.rs`
- **Commit:** `7a019a4`
- **Fix:** `batch_classify` resolves + computes wire + journals under one `store.lock()`; retry inner unifies `resolve_for_move` + `message_location` + `journal_moved` in single guard.

### WR-04: Preview true count + truncation + archive cap
- **Files:** `src-tauri/src/classify/batch.rs`
- **Commit:** `7a019a4`
- **Fix:** `preview_classify` counts `total = cands.len()` before cap; `truncated = total > items.len() + skipped.len()`; `preview_archive` tracks `total_candidates` from full candidate stream, applies cap to skipped, fixes `truncated`.

### WR-05: Archive preview drops errors
- **Files:** `src-tauri/src/classify/batch.rs`
- **Commit:** `7a019a4`
- **Fix:** `archive_dest_for` errors pushed to `skipped` with reason `"undatable"` instead of silent `continue`; cap enforced.

### WR-06: Transactional merge + mutual exclusion
- **Files:** `src-tauri/src/commands/classify.rs`
- **Commit:** `7a019a4`
- **Fix:** `retry_failed_inner` uses `BEGIN IMMEDIATE ... COMMIT` around totals read-modify-write; `batch_classify` / `archive_run` resume paths refuse when `try_claim_retry` holds the run (mutual exclusion).

### WR-07: Retry entry TOCTOU + RAII guard
- **Files:** `src-tauri/src/commands/classify.rs`
- **Commit:** `7a019a4`
- **Fix:** `RetryGuard(i64)` with `Drop` releases lane on panic/cancel; entry state re-checked inside `retry_failed_inner` after claim (refuses if state flipped to `undone`/`running`).

### IN-01: Missing mailbox epoch false-fresh
- **Files:** `src-tauri/src/classify/batch.rs`
- **Commit:** `7a019a4`
- **Fix:** `resolve_for_move` uses `Option<u32>` from query (not `unwrap_or(0)`); missing epochs force `Ok(None)` (stale/re-resolve).

### IN-02: `retried` counts skipped
- **Files:** `src-tauri/src/commands/classify.rs`
- **Commit:** `7a019a4`
- **Fix:** `retried += 1` moved after skip branches (only reaches verb attempts); previously counted every loop iteration.

### IN-03: Preview holds Mutex for whole computation
- **Files:** `src-tauri/src/commands/classify.rs` (documented / structurally brief)
- **Commit:** `7a019a4`
- **Fix:** Preview commands already hold only brief `store.lock()` for pure-read `preview_plan`; no structural change required beyond audit note.

### IN-04: No-op replace + run-id inconsistency
- **Files:** `src-tauri/src/commands/classify.rs`, `src/components/BatchPanel.tsx`
- **Commit:** `7a019a4`
- **Fix:** Removed dead `dest_display.replace('/', "/")`; unified undo to use `report?.run_id ?? runId` consistently; retry already uses `report.run_id`.

---

_Fixed: 2026-10-14T15:30:00Z_
_Fixer: the agent (gsd-code-fixer)_
_Iteration: 1_
