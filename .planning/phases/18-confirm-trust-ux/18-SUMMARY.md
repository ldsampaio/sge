# Phase 18: Confirm + Trust UX — Summary

**Status:** complete (2026-10-10, autonomous run)
**Tests:** full suite 318 green (6 filing plan/path tests)

## What shipped (backend)

- `classify/filing.rs` — `plan_filing` (label-required, stale-refused,
  child recomputed, AlreadyThere guard), `ensure_auto_tree`
  (LIST-check → CREATE via Phase 11 guards → refresh → marker),
  `execute_filing_move` (Phase 10 optimistic+outbox+immediate mirror),
  `auto_path_for_id`, COLLISION!"
- Collision rule: user-owned `Auto` without marker → `COLLISION` refusal;
  `allow_nest` (dialog approval) records marker + nests. Never silent.
- Commands: `confirm_suggestion` (label-gated, offline→outbox),
  `override_label` (MOVE + log + pin + relabel; drain skips pinned),
  `dismiss_suggestion`, `set_confidence_threshold` (0–1 validated),
  `review_list`, `suggestion_detail`, `suggestion_for_uid`,
  `mailbox_labels` (bulk, one query).
- M14 (`needs_review` + `classify_settings`), M15 (`dismissed`).
- Secondary labels: stored, badged, never enter any filing path
  (grep-proof: no reader outside suggest/detail/UI).

## What shipped (UI)

- `CategoryBadge` (primary chip + dotted local-only secondary + dashed
  review style) in message-list rows (bulk-fetched per mailbox).
- `SuggestionBar` in the reader: destination + % + redacted justification
  (`Sinais: …`, transient, `filter_output`ed) + Mover/Corrigir▾/Dispensar.
- `ReviewPanel` modal (`A Classificar` count row in sidebar): per-item
  Mover/Dispensar + threshold slider (future suggestions only).
- `ClassifyStatus` (Phase 17) already quiet-by-default in App.

## Verified requirements

- CLS-03 ✓ (suggest→confirm→MOVE, backend-refused bypass, offline replay)
- CLS-05 ✓ (threshold setting, validated, future-only)
- TRUST-01 ✓ (override → re-move + log + pin)
- TRUST-02 ✓ (primary/secondary badges, queue states)
- TRUST-03 ✓ (redacted transient justification; leak-test: labels table
  holds IDs/scores only)

## Deferred (with reason)

- Live confirm→MOVE→undo round-trip vs `mail.utfpr.edu.br`: no headless
  credentials in this environment (same standing deferral as v1.2's
  `/gsd-verify-work 12/13`). Mock-level paths covered; schedule with creds.
