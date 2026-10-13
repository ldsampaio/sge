---
phase: 23-search-filters
plan: "02"
subsystem: database
tags: [sqlite, tauri-commands, rust, saved-filters, search-state]

# Dependency graph
requires:
  - phase: 23-01-search-engine
    provides: fts_search_scoped + search_snippet + SEARCH_RESULT_LIMIT
provides:
  - SRCH-02 backend (filter CRUD, query-time apply, restart persistence)
  - SRCH-03 backend (SQLite search-state save/restore)
  - Eight Tauri filter/state commands for the 23-03 UI
affects: [23-03-search-ui, future-rules]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 9900
  tasks: 3
  commits: 1

# Tech tracking
tech-stack:
  added: []
  patterns: [backend-validated CRUD with verbatim pt-BR errors, ui_state key-value search view, query-time filter re-execution]

key-files:
  created: [src-tauri/src/commands/search.rs]
  modified: [src-tauri/src/store/mod.rs, src-tauri/src/store/queries.rs, src-tauri/src/commands/mod.rs, src-tauri/src/lib.rs]

key-decisions:
  - "search_scoped command returns SearchHit (row + snippet) so 23-03 needs no extra IPC"
  - "State save is shape-only; stale filter ids tolerated at save, re-validated at restore"
  - "M18 preserve test seeds post-upgrade (filter rows cannot predate their tables)"

patterns-established:
  - "FilterError with Display-only pt-BR copy (UI shows verbatim, never SQL text)"
  - "ui_state key namespace with SEARCH_STATE_KEY constant"

requirements-completed: [SRCH-02, SRCH-03]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Save survives store re-open; stored query re-executes to identical hits; filter delete leaves mail untouched"
    requirement: "SRCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#tracer_save_restart_read_reexecute"
        status: pass
    human_judgment: false
  - id: D2
    description: "Full CRUD with backend name validation (empty/80-char/duplicate/rename/unknown-id) in plain language"
    requirement: "SRCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#filter_name_validation_is_backend_side"
        status: pass
    human_judgment: false
  - id: D3
    description: "Folder-scope JSON round-trips All vs set; malformed scope rejected plainly"
    requirement: "SRCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#folder_scope_json_roundtrips_all_vs_set"
        status: pass
    human_judgment: false
  - id: D4
    description: "Search state round-trips, survives re-open, empty query restores folder-list branch; stale/corrupt tolerated"
    requirement: "SRCH-03"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#search_state_roundtrips_and_survives_reopen"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#search_state_tolerates_stale_filter_and_corruption"
        status: pass
    human_judgment: false
  - id: D5
    description: "Filter apply is local-only (one capped FTS query, no sync/IMAP) with rank/date ordering"
    requirement: "SRCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#filter_apply_path_is_local_only_and_sorted"
        status: pass
    human_judgment: false
  - id: D6
    description: "M18 forward migration preserves rows and creates filter/state tables"
    requirement: "SRCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#m18_adds_filters_and_ui_state_preserving_rows"
        status: pass
    human_judgment: false
  - id: D7
    description: "All eight filter/state commands registered in lib.rs next to search_messages"
    requirement: "SRCH-02"
    verification:
      - kind: other
        ref: "grep commands::search:: src-tauri/src/lib.rs (8 entries)"
        status: pass
    human_judgment: false

# Metrics
duration: 50min
completed: 2026-10-13
status: complete
---

# Phase 23 Plan 02: Saved Filters + Search State Summary

**M18 saved_filters + ui_state tables with backend-validated CRUD, query-time filter re-execution, and eight Tauri commands — sync path untouched**

## Performance

- **Duration:** 50 min
- **Started:** 2026-10-13T20:55:00Z
- **Completed:** 2026-10-13T21:45:00Z
- **Tasks:** 3
- **Files modified:** 5

## Accomplishments

- M18 migration: `saved_filters` (name UNIQUE, query, folder-scope JSON, sort) + `ui_state` key-value store
- Filter CRUD with backend pt-BR validation (non-empty, 80 chars, unique, plain errors); delete removes only the filter row
- `apply_saved_filter` re-executes the stored query at query time through the bound MATCH path; `search_scoped_with_snippets` returns hits with snippets
- `save/load_search_state` in SQLite (restart + reconnect restore, empty-query branch, stale/corrupt tolerance)
- Full Rust suite green (345 lib tests); sync worker has zero filter/state references; outbox/send_queue contracts untouched

## Task Commits

All three tasks landed in one atomic commit (shared files, plan-level verification):

1. **Task 1: Tracer: save one filter, restart-read it, re-execute it** - `aef3acf` (feat)
2. **Task 2: Filter CRUD + backend validation, search-state save/restore** - `aef3acf` (feat)
3. **Task 3: Preserve-rows test + validation/state edge-case suite** - `aef3acf` (feat)

## Files Created/Modified

- `src-tauri/src/commands/search.rs` - 8 Tauri commands (save/list/rename/delete/apply/state ×2, search_scoped), spawn_blocking pattern
- `src-tauri/src/store/queries.rs` - FilterError, SavedFilter, SearchState, SearchHit, CRUD + state functions, 6 tests
- `src-tauri/src/store/mod.rs` - M18 SQL + preserve-rows test module
- `src-tauri/src/commands/mod.rs` - `pub mod search`
- `src-tauri/src/lib.rs` - 8 command registrations next to `search_messages`

## Decisions Made

- `search_scoped` returns `SearchHit` (flat row + snippet) rather than bare rows, so 23-03 renders snippets + scope with a single invoke per pause (no N+1 snippet IPC).
- `sort: "date"` re-orders in Rust over the capped 200 (cheap, keeps one SQL shape); `"rank"` keeps BM25 order.
- State save validates shape only (sort + scope); stale `active_filter_id` is stored fine and re-validated at restore (frontend owns the fallback per T-23-11).
- Corrupt `ui_state` JSON degrades to `None` (safe folder-list view) rather than failing launch.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Missing From<rusqlite::Error> for FilterError**
- **Found during:** Task 1 (compile after appending filter section)
- **Issue:** `?` on rusqlite calls inside `Result<_, FilterError>` functions failed (8 errors, E0277)
- **Fix:** Added `impl From<rusqlite::Error> for FilterError` mapping to `FilterError::Store`
- **Files modified:** src-tauri/src/store/queries.rs
- **Verification:** `cargo check` clean, full suite green
- **Committed in:** aef3acf (part of task commit)

**2. [Rule 2 - Correctness] M18 preserve test seeds filters post-upgrade**
- **Found during:** Task 3 (preserve-rows test)
- **Issue:** Plan asks for "seeded filters + ui_state rows survive migration", but those rows cannot predate tables created by the migration itself
- **Fix:** Seed pre-existing rows (message + send_queue), upgrade, assert they survive + new tables exist, then write filter/state rows post-upgrade and round-trip them
- **Files modified:** src-tauri/src/store/mod.rs
- **Verification:** `m18_adds_filters_and_ui_state_preserving_rows` green
- **Committed in:** aef3acf (part of task commit)

---

**Total deviations:** 2 auto-fixed (1 bug, 1 correctness)
**Impact on plan:** Both required for a compiling, honest test suite. No scope creep; all plan verification items hold.

## Issues Encountered

- Test bug: first "Duplo" create wrongly expected to fail (`unwrap_err` on `Ok`) — fixed to create-then-duplicate; suite green.
- Same self-clipped-edit hazard as 23-01 (once, on a const); caught by `cargo check` + `git diff --stat` before commit. No tree impact.

## Threat Model Coverage

- T-23-05 (stored-query injection): re-execution uses the plan-01 bound MATCH path; scope JSON strict-schema (`encode_scope`/`decode_scope`), malformed rejected plainly — tests green.
- T-23-06 (validation bypass): all name/sort/scope rules backend-side in queries.rs; UI restraint is not the guard.
- T-23-07 (state disclosure): state holds query + folder names + sort only; keyring untouched.
- T-23-08 (apply storms): one capped local FTS query per apply; sync path has zero filter references (grep-verified).
- T-23-SC: no new dependencies.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- SRCH-02 backend + SRCH-03 backend complete; 8 commands registered for the 23-03 panel/scope/restore UI.
- Live-gate position: filters + state are local-only, no live server leg applies (test-committed + grep-verified).

---
*Phase: 23-search-filters*
*Completed: 2026-10-13*
