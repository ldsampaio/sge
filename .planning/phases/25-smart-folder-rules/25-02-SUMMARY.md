---
phase: 25-smart-folder-rules
plan: "02"
subsystem: rules-engine
tags: [smart-rules, conflict-resolution, taxonomy-evolution, redaction, react, bulk-journal]

# Dependency graph
requires:
  - phase: 25-01-backend-core
    provides: folder_rulesSuggestions tables, rule engine + validation, CRUD/apply commands, behind-sync hook, confirm rule branch
  - phase: 20-batch-reorganization
    provides: batch_runs/batch_items journal, journal_intent/moved/ids, undo_run, batch_report, BATCH_CHUNK discipline
  - phase: 19-taxonomy-editor
    provides: taxedit ID-stable ops, merge/delete commands, TaxonomyEditor UI patterns
provides:
  - Rule conflict semantics (priority cutoff, rule-vs-classifier user pick, taxonomy-overlap confirm)
  - ID-stable rule survival across taxonomy merge/delete/rename/bump with notices
  - Redacted transient justifications (fixture-pinned)
  - Journaled bulk apply_rules with progress + undo via shared batch machinery
  - RulesPanel UI + ReviewPanel comparison section + Sidebar entry
affects: [verify-work, ship-gate]

# Actuals (#2632)
actuals:
  tokens: 23576
  tasks: 3
  commits: 4

# Tech tracking
tech-stack:
  added: []
  patterns: [pure-detail-builder extraction for command testability, TaxonomyChangeReport notice surface, thin-vs-journaled reuse-depth cutoff]

key-files:
  created: [src/components/RulesPanel.tsx]
  modified: [src-tauri/src/commands/classify.rs, src-tauri/src/classify/taxedit.rs, src-tauri/src/classify/taxonomy.rs, src-tauri/src/classify/redact.rs, src-tauri/src/lib.rs, src/components/ReviewPanel.tsx, src/components/TaxonomyEditor.tsx, src/components/Sidebar.tsx]

key-decisions:
  - "confirm_suggestion prefer=classifier param resolves the user pick without a new command"
  - "Bulk reuses batch_runs/batch_items + undo_batch/batch_report wholesale; no duplicate journal"
  - "Rule-confirm pins an override so confirmed moves never re-fire on the next sync"
  - "Nested frontend condition keys sent in snake_case verbatim (conversion-depth-proof)"
  - "merge/delete return TaxonomyChangeReport; TaxonomyEditor surfaces rule notices"

patterns-established:
  - "Conflict comparison payload: rule_alternative + classifier_alternative on the same suggestion view"
  - "Evolution notices: revalidate-never-drop + reassign-or-disable with UI-surfaced reasons"

requirements-completed: [RULE-01, RULE-02, RULE-03]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Priority cutoff (first match wins, oldest wins ties); rule-vs-classifier comparison payload; overlap confirm; A Classificar never auto-moves; dismissal keeps mail in place"
    requirement: "RULE-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/commands/classify.rs#rule_conflict_* (5 tests)"
        status: pass
    human_judgment: false
  - id: D2
    description: "Merge remaps rule ids; delete reassigns-or-disables with notice; rename-safe; bump revalidates never drops; redaction fixtures; quote-safe conditions"
    requirement: "RULE-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/commands/classify.rs#rule_evolv_* (4) + taxedit#rule_evolv_* + taxonomy#rule_evolv_* + redact#rule_evolv_* (9 total)"
        status: pass
    human_judgment: false
  - id: D3
    description: "RulesPanel lists/toggles/edits/deletes/reorders against live commands; bulk journals with progress + undo; tsc clean; no import/export or Sieve surface"
    requirement: "RULE-03"
    verification:
      - kind: unit
        ref: "src-tauri/src/commands/classify.rs#rule_bulk_* (2 tests)"
        status: pass
      - kind: other
        ref: "npx tsc --noEmit (clean)"
        status: pass
    human_judgment: false
  - id: D4
    description: "Manual end-to-end: overlapping rules (first wins), comparison dialog pick, merge remap, delete reassign prompt, redacted justification, bulk progress + undo restore"
    requirement: "RULE-01"
    verification: []
    human_judgment: true
    rationale: "Requires running Tauri UI + live mailbox; automation covers the offline contract only"

# Metrics
duration: 1h50min
completed: 2026-10-13
status: complete
---

# Phase 25 Plan 02: Conflicts, Evolution, Redaction, UI Summary

**Rule-vs-rule priority cutoff, rule-vs-classifier user-pick dialog, taxonomy-overlap confirms, ID-stable rule survival across taxonomy edits, redacted justifications, journaled bulk apply with undo, and the RulesPanel UI — all tested, tsc clean.**

## Performance

- **Duration:** ~1h50 min
- **Started:** 2026-10-13T15:30Z
- **Completed:** 2026-10-13T17:20Z
- **Tasks:** 3
- **Files modified:** 9

## Accomplishments

- Conflict semantics: priority cutoff with oldest-wins ties (tested), `rule_alternative` comparison payload on `suggestion_detail` + `classifier_alternative` on rule views, `confirm_suggestion(prefer=)` user pick, taxonomy-overlap notes with Auto-tree confirm-then-nest routing, A-Classificar never auto-moves (tested), dismissal keeps mail in place (tested)
- Evolution: `category_ids` helper, pure `remap_rule_destination`, merge remaps in the same migration, delete reassigns-or-disables with notices, every bump revalidates (disable, never drop), rename-safe by ID storage, `TaxonomyChangeReport` surfaced in TaxonomyEditor
- Redaction: two rule-justification fixtures pinning password/code/barcode scrubbing through `filter_output` with rule names kept
- Bulk: `plan_rule_bulk` planner + `apply_rules_bulk` journaled runs reusing `batch_items`/`undo_batch`/`batch_report` wholesale, `rule-progress` events, per-chunk UIDVALIDITY re-check, collision destinations skipped loudly; thin-vs-journaled cutoff documented
- UI: `RulesPanel.tsx` (CRUD, toggle, reorder, thin + bulk apply, progress, undo) mounted in Sidebar next to Categorias; ReviewPanel rules section with comparison pick buttons and overlap warnings; `tsc --noEmit` clean
- Full lib suite: 392 passed, 0 failed (16 new 25-02 tests: 5 conflict + 9 evolution + 2 bulk)

## Task Commits

Each task was committed atomically (task 3 split into backend + frontend commits):

1. **Task 1: Conflict semantics** - `25afecf` (feat)
2. **Task 2: ID-stable evolution + redaction** - `c989dc2` (feat)
3. **Task 3a: Journaled bulk backend** - `3121944` (feat)
4. **Task 3b: RulesPanel UI + surfaces** - `f17317e` (feat)

## Files Created/Modified

- `src-tauri/src/commands/classify.rs` - Comparison payloads, prefer param, Auto-nest routing, override pinning, revalidation, reassign, bulk planner + command, 11 new tests
- `src-tauri/src/classify/taxedit.rs` - Pure `remap_rule_destination` + test
- `src-tauri/src/classify/taxonomy.rs` - `category_ids` + test
- `src-tauri/src/classify/redact.rs` - Two rule-justification fixtures
- `src-tauri/src/lib.rs` - `apply_rules_bulk` registration
- `src/components/RulesPanel.tsx` - Full rules UI (created)
- `src/components/ReviewPanel.tsx` - Rules section with comparison pick + overlap notes
- `src/components/TaxonomyEditor.tsx` - Change-report notices on delete
- `src/components/Sidebar.tsx` - Regras entry + modal mount

## Decisions Made

- `prefer="classifier"` param on `confirm_suggestion` (optional, backward-compatible) resolves the user pick without a new command; default confirms the rule hit; label-path confirm clears the pending rule hit.
- Bulk reuses the Phase 20 journal wholesale (`batch_items` with rule id as `label_id`; existing `undo_batch`/`batch_report` work unchanged) — no duplicate journal table, no new undo path.
- Rule-confirm (single + bulk) pins an override so confirmed moves never re-fire on the next sync (the message still matches the rule in its new folder).
- Nested frontend condition/destination keys are sent in snake_case verbatim, immune to IPC case-conversion depth; top-level args stay camelCase per existing precedent.
- Remap ordering: rule remap runs BEFORE `persist_taxonomy` in merge (and re-enable-after-remap in delete) so the bump revalidation never spuriously disables freshly remapped rows.
- `merge_categories`/`delete_category` now return `TaxonomyChangeReport`; the single TaxonomyEditor caller ignores/displays it without breakage.

## Deviations from Plan

None - plan executed exactly as written. Two judgment calls within agent discretion, both documented above: the `prefer` param shape for the user pick, and snake_case nested keys for IPC robustness.

## Issues Encountered

- `is_pending_delete(conn, mailbox_id, uid)` takes mailbox+uid, not message-id — fixed the dismissal test to pass both.
- Concurrent Phase 26-02 executor on the same branch: left its `live_gate_26*` working-tree edits and commits untouched (individual `git add`, never `git add -A`); verified all four 25-02 commits contain only Phase 25 files.
- Outbox/send_queue contracts untouched: no changes to `flag_outbox`, `imap_outbox` schema, or `send_queue` tables (bulk journals to `batch_items` only).
- Deferred ideas absent as required: no import/export format, no learning-from-overrides (the override pin is loop prevention, not suggestion learning), no per-category auto-confirm, no Sieve export.

## Threat Flags

None beyond the plan's register — T-25-07…T-25-12 mitigations implemented as prescribed (comparison dialog + dismissal no-op tested; merge/delete remap + revalidate-never-drop tested; transient redacted justifications fixture-pinned; journaled bulk with per-chunk epoch re-check; UI-only authoring, no import/Sieve surface). T-25-SC holds: no installs, no new dependencies.

## Known Stubs

None — no placeholder values, TODOs, or unwired paths. The `rule-progress`/`rule-suggestions` events have live emitters and live listeners (RulesPanel/ReviewPanel reload paths).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Phase 25 complete: RULE-01/02/03 fully TRUE per plan success criteria (offline contract; live UAT D4 deferred to human with a real account).
- Consumers: `verify-work` can route D1–D3 deterministically (all pass) and prompt only D4.

## Self-Check: PASSED

- All 9 files present with 25-02 content; `RulesPanel.tsx` created.
- Commits `25afecf`, `c989dc2`, `3121944`, `f17317e` all ancestors of HEAD.
- Full lib suite 392 passed / 0 failed; `tsc --noEmit` clean at commit time.

---
*Phase: 25-smart-folder-rules*
*Completed: 2026-10-13*
