---
phase: 30-batch-trust-pair
reviewed: 2026-10-14T12:00:00Z
depth: deep
files_reviewed: 6
files_reviewed_list:
  - src-tauri/src/classify/batch.rs
  - src-tauri/src/commands/classify.rs
  - src-tauri/src/classify/archive.rs
  - src-tauri/src/lib.rs
  - src/components/BatchPanel.tsx
  - src/components/ArchivePanel.tsx
  - src/components/BatchPreviewTable.tsx
findings:
  critical: 0
  warning: 7
  info: 4
  total: 11
status: fixed
---

# Phase 30: Code Review Report (Batch Trust Pair)

**Reviewed:** 2026-10-14T12:00:00Z
**Depth:** deep
**Files Reviewed:** 7 (commits `9757cad` + `1899e60`)
**Status:** issues_found

## Summary

Reviewed the Batch Trust Pair backend (`preview_plan`, `resolve_for_move`,
`retry_failed` + single-flight guard, totals merge) and the trust-pair UI
(`BatchPreviewTable`, both panels) against the BATCH-03/BATCH-04 contracts:
exactly-once under stress, stale-UID-to-verb paths, transaction atomicity of
the retry totals merge, crash-window rows, preview caps, redaction, and the
sandbox/no-migration/zero-dep constraints. Cross-checked `archive_one`,
`message_location` (returns `mailboxes.name`), and `finish_batch_run`
semantics to verify the string-domain comparisons in the new code.

No BLOCKERs: no secret leakage, no injection/XSS surface (all UI strings
render as text, backend `filter_output` on basis strings), caps are enforced,
no migration was added, no new deps, BODY.PEEK-only holds (preview reads
cached rows only). But 7 WARNINGs are real correctness defects, three of them
in exactly-once-sensitive paths (double-counted retry totals, wire-vs-name
already-filed comparisons, resolve/journal lock split), plus 4 INFO items.
The `status: passed` 7/7 VERIFICATION verdict overstates: the headless tests
exercise the helpers through a simulated transcript, but WR-01/WR-02/WR-03
below are all reachable in the real command path and unasserted by any test.

## Warnings

### WR-01: Retry double-counts already-filed rows as both moved AND skipped

**File:** `src-tauri/src/commands/classify.rs:1884-1885`
**Issue:** In `retry_failed_inner`, the already-at-destination path does
`moved += 1; skipped += 1` for the same row, and the stale-but-already-filed
path (`:1873` + `:1876`) likewise counts `moved += 1` then falls through to
`skipped += 1`. One row increments both counters, so `moved + skipped >
retried` and the merged `retry_skipped` in `totals_json` grows on every retry
even when every failure is recovered. The user-facing message
(`{moved} recuperados · {skipped} ignorados`) double-reports, and the
persisted `retry_skipped` count is monotonically wrong across retries.
**Fix:**
```rust
if res.from_folder == item.to_folder {
    let guard = store.lock().unwrap();
    let _ = batch::journal_moved(guard.conn(), run_id, item.message_id);
    moved += 1;
    // Do NOT also count as skipped: the row is resolved, not skipped.
    continue;
}
```
Apply the same split in the stale branch: count stale-skips and
already-filed recoveries in separate counters.

### WR-02: Already-filed detection compares mailbox name against wire string

**File:** `src-tauri/src/commands/classify.rs:1879-1884`
**Issue:** `res.from_folder` / `cur_folder` come from `message_location`,
which returns `mailboxes.name` (`store/queries.rs:2574`), while
`item.to_folder` is the journaled `dest_wire` (delimiter-encoded,
MUTF7-encoded non-ASCII leaves — see the `encode_modified_utf7` call in the
confirm path). For any non-`/` delimiter account or non-ASCII taxonomy name,
`res.from_folder == item.to_folder` and `cur_folder == item.to_folder` are
never true, so the crash-window exactly-once shortcut
(`30-02-SUMMARY.md` decision 3) is dead code on those accounts: an
already-filed row falls through to a re-issued `move_one_uid`/`archive_one`
verb instead of being recorded without a verb. Same-domain concern at
`:1891`, where `archive_one(..., &item.to_folder, &item.to_folder)` passes
the wire string as `dest_display`, so `archive_detail()` logs MUTF7 gibberish
and the `mailbox == dest_wire` AlreadyThere guard inside `archive_one`
(`archive.rs:380`) inherits the same domain mismatch.
**Fix:**
```rust
// Compare in one domain: decode wire → display before comparing, or store
// dest_display alongside to_folder at journal_intent time.
let dest_display = wire_to_display(&item.to_folder, &delim);
if res.from_folder == dest_display { /* record without verb */ }
```
At minimum, pass the decoded display string (not the wire string) as
`archive_one`'s `dest_display` argument.

### WR-03: `resolve_for_move` and `journal_intent` are separate lock acquisitions

**File:** `src-tauri/src/commands/classify.rs:1510-1560`
**Issue:** The code comments claim the MOVE verb addresses "the re-resolved
pair read in the same lock section as the journal write" (T-30-01), but
re-resolution, wire resolution, and `journal_intent` each take and drop
`store.lock()` independently (three separate `{ let guard = ... }` blocks in
both `batch_classify` and `archive_run`, and the same split in
`retry_failed_inner` at `:1850-1876`). A burst-EXISTS relocation landing
between the resolve block and the journal block journals a now-stale
(folder, UID) as intent and fires the verb at it. The in-process Mutex never
protected against server-side moves anyway; the structural guarantee as
stated does not hold.
**Fix:** Hold one guard across resolve → journal (compute the wire string
from cached data before locking, then do resolve + `journal_intent` under a
single `store.lock()` acquisition), and document the residual
resolve-to-verb window honestly instead of claiming same-section atomicity.

### WR-04: `total_candidates` under-reports and `truncated` is tautological

**File:** `src-tauri/src/classify/batch.rs:310,320`
**Issue:** `preview_classify` caps `items` and `skipped` at `PREVIEW_CAP`
independently during collection, then reports
`total = items.len() + skipped.len()` — the capped sum, not the true
candidate count (e.g. 600 movable rows report `total_candidates: 500`).
`truncated` is `len >= CAP && total >= CAP` where `total` IS that same sum,
so the second conjunct is dead. Same shape in `preview_archive` (`:468-477`),
where `truncated` additionally ignores the skipped-list cap entirely. The UI
renders `total_candidates` as truth ("500 (valores totais acima)"), so users
see a wrong total on large accounts.
**Fix:**
```rust
let mut total = 0usize;
for (mid, mb, _uid) in cands {
    total += 1;
    let row = build_classify_preview_item(conn, &tax, mid, &mb)?;
    // ... cap pushes, count everything
}
let truncated = total > items.len() + skipped.len();
```

### WR-05: Archive preview silently drops `archive_dest_for` errors (parity gap)

**File:** `src-tauri/src/classify/batch.rs:418-419`
**Issue:** When `archive_dest_for` fails for a candidate, preview does
`continue` — the row appears in neither `items` nor `skipped`, so it is
invisible in the plan. The real run path handles these rows (or fails them
visibly); T-30-04 parity ("preview and execute cannot diverge") is broken
exactly where the matcher disagrees. These rows should surface as skipped
with a reason (e.g. `undatable`/destination error), consistent with the
undatable-skipped treatment below.
**Fix:**
```rust
let Ok((dest_display, _, _)) = super::archive::archive_dest_for(conn, mid, &mb, now)
else {
    if skipped.len() < PREVIEW_CAP {
        skipped.push(PreviewItem { message_id: mid, from_folder: mb,
            to_folder: String::new(), dest_display: String::new(),
            basis: String::new(), skipped_reason: Some("undatable".to_string()) });
    }
    continue;
};
```

### WR-06: Retry totals merge is a non-atomic read-modify-write; resume+retry not mutually excluded

**File:** `src-tauri/src/commands/classify.rs:1935-1945`
**Issue:** The merge reads `totals_json`, adds `skipped`, and writes via
`finish_batch_run` with per-row `journal_moved` calls outside any SQLite
transaction — a crash between the last `journal_moved` and the merge loses
the `retry_skipped` accumulation while keeping the row facts (count drift
across crash recovery). Worse, BatchPanel simultaneously offers Retomar
(resume) and Repetir (retry) on `interrupted` runs with failures, and the
run-level single-flight guard covers retry-vs-retry only: a concurrent
`batch_classify(resumeRun)` + `retry_failed` on the same `run_id` both issue
verbs for the same failed rows (double-file attempt) and both read-merge the
same `totals_json` base (last-writer-wins loses one merge).
**Fix:** Wrap the per-row journal updates + totals merge in a single
`conn.execute("BEGIN IMMEDIATE")` … `COMMIT` transaction (or defer the merge
into the same lock section), and extend the run-level claim guard to resume
(`try_claim_retry` checked by the resume entry path, or refuse retry while a
run is `running` — re-check state inside `retry_failed_inner` after claiming
the lane).

### WR-07: Retry entry gate is TOCTOU — state can flip to `undone` mid-retry

**File:** `src-tauri/src/commands/classify.rs:1762-1784`
**Issue:** `check_retry_entry` runs before `try_claim_retry`, and
`retry_failed_inner` re-reads state only to preserve it (`run_state`), never
to re-validate. A user hitting Desfazer (undo → state `undone`, originals
restored) after the entry check but during the retry means the retry keeps
re-issuing MOVEs for rows whose mail was just restored — directly violating
"undone runs never retry". The single-flight lane also has no RAII guard: a
panic inside the awaited inner (or a cancelled Tauri future) skips
`release_retry` at `:1784`, and every subsequent `lock().unwrap()` on a
poisoned `retry_set` mutex panics — the lane wedges permanently.
**Fix:** Re-validate `check_retry_entry`-equivalent state after claiming the
lane and again before the verb loop; wrap the claim in a drop-guard so the
lane always releases:
```rust
struct RetryGuard(i64);
impl Drop for RetryGuard { fn drop(&mut self) { release_retry(self.0); } }
```

## Info

### IN-01: Missing-mailbox epoch defaults to 0 on both sides (false-fresh)

**File:** `src-tauri/src/classify/batch.rs:526`
**Issue:** `resolve_for_move` maps a missing `mailboxes` row to epoch `0`,
and every caller maps a missing snapshot epoch to `0` as well — two unknowns
compare equal and the item resolves as fresh. A folder deleted + recreated
between snapshot and resolve should be stale, not fresh.
**Fix:** Return `Ok(None)` when either epoch lookup misses instead of
defaulting to `0`.

### IN-02: `retried` counts skipped rows; `failed_remaining` is the only honest field

**File:** `src-tauri/src/commands/classify.rs:1846`
**Issue:** `retried += 1` fires at the top of the loop for every row
including skips, so `RetryReport.retried` conflates attempted and skipped.
Harmless while the UI only renders `moved/skipped/failed_remaining`, but any
future consumer will misread it.
**Fix:** Increment `retried` only on rows that reach the verb, or rename to
`considered`.

### IN-03: Preview commands hold the store Mutex for the entire computation

**File:** `src-tauri/src/commands/classify.rs:1714-1715,1724-1725`
**Issue:** `preview_batch`/`preview_archive` hold `state.store.lock()` across
the whole `preview_plan` run — full candidate collection, per-row label and
keyword matching, plus the archive leg's per-mailbox full message scan. On a
large account this blocks the sync worker and all other DB users for the
duration. No deadlock (preview never re-locks), but a responsiveness hazard
that grows with account size.
**Fix:** Snapshot the needed rows under a short lock (or clone the needed
data), drop the guard, then run the pure matcher off-lock; or paginate the
preview scan.

### IN-04: Copied `replace('/', "/")` no-op and undo/retry run-id inconsistency

**File:** `src-tauri/src/commands/classify.rs:1543-1545`; `src/components/BatchPanel.tsx:153-154,207-208`
**Issue:** `dest_display.replace('/', "/")` replaces a char with itself
(carried over from the pre-30 code) — dead expression that suggests encoding
that isn't happening. Separately, BatchPanel's retry targets
`report.run_id` while undo targets the `runId` state variable, so after
viewing a past run's report the two buttons can address different runs.
**Fix:** Replace the no-op with a plain clone (or the real delimiter
encoding) and make undo use `report?.run_id ?? runId` consistently.

---

_Reviewed: 2026-10-14T12:00:00Z_
_Reviewer: the agent (gsd-code-reviewer)_
_Depth: deep_
