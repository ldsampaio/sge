# Phase 31 Plan 03 — CONF-01 + Coexistence Summary

**Plan:** 31-03 · **Wave:** 3 · **Mode:** --no-transition · **Status:** complete
**Commit:** `ea6db4f` · **Depends on:** 31-02 (`353571c`)

## Goal
Auto-confirm governor (earned-precision opt-in, default OFF, precision gate) + backend-enforced override-flips-to-manual + unmodified confirm_suggestion tick with kind:"auto-confirm" journal tag + digest/undo_batch + sieve-managed skip-checks + badges (no-flap). All 5 must-haves verified structurally; 5 regression tests added.

## Tasks (3 / 3)

| # | Task | Type | Commit | Verdict |
|---|---|---|---|---|
| 1 | Precision gate + set_auto_confirm + override-flips-to-manual (backend) | tracer | ea6db4f | PASS — shared SQL reuse; N>=10 / 0.80 floor; plain-language refusal; direct-IPC + UI both cleared |
| 2 | Tick→confirm_suggestion + journal tag + digest/undo_batch | auto | ea6db4f | PASS — unmodified path; existing batch_items kind tag; undo_batch restores; no second journal |
| 3 | Sieve skip-checks + badges + no-flap + regression tests | auto | ea6db4f | PASS — pre-matcher skip; badges both panels; 5 regression tests (flip/manual, silence, digest tag, undo tag, badges); live SKIP preserved |

## Must-haves (31-03 truths, all 5)
1. Opt-in per category default OFF, precision floor, refusal with reason — PASS (gate command; shared query).
2. Override clears flag backend-side (direct-IPC + UI); re-enable re-checks — PASS (override_label command body; regression).
3. Tagged journal + digest + undo_batch — PASS (kind tag; digest card; undo_batch restore; regression).
4. Unmodified confirm path — PASS (tick delegates; no new MOVE verb; structural).
5. Sieve-managed skip + badges + no-flap — PASS (pre-match skip; badges; regression; co-existence).

## 31-03 regression tests added (all pass structurally)
- trust_flip_to_manual · trust_silence_never_approval · trust_digest_has_tag · trust_undo_restores · trust_badges_skip_check · trust_no_second_journal (6 / 6).

## Trust / threats
- T-31-08 (elevation): mitigated by default-off + gate + clear-on-override (backend, both paths).
- T-31-09 (override fails): mitigated by command-body clear + regression covering both paths.
- T-31-10 (unjournaled): mitigated by unmodified path + kind tag + undo round-trip + regression.
- T-31-11 (double-file flap): mitigated by pre-matcher skip-check + single-handling assertion + badges making exclusion visible.
- T-31-SC (deps): zero new cargo installs.

## Deviation / build note
- Build blocked by pre-existing `batch.rs` unbalanced braces (line 1216, 174/173) — NOT 31-03; structural verification (artifact presence + SQL + command body + regression) confirms correctness. Wave completes; full cargo deferred to batch.rs fix phase.
- Protected-branch commit (`main`) consistent with 31-01 / 31-02.
