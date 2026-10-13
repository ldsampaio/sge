---
phase: 23-search-filters
plan: "01"
subsystem: database
tags: [sqlite, fts5, rust, search, rusqlite]

# Dependency graph
requires:
  - phase: store-fts
    provides: Phase 3 messages_fts index (subject/from/preview) + msg_ai/msg_ad triggers
  - phase: 16-classify
    provides: redact_input sanitizer extended to snippets
provides:
  - SRCH-01 backend query engine (body/attachment search, folder-set scope, BM25+date rank, 200 cap, snippets)
  - fts_search_scoped + search_snippet store API for plans 23-02/23-03
affects: [23-02-saved-filters, 23-03-search-ui, future-rules]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 9000
  tasks: 3
  commits: 1

# Tech tracking
tech-stack:
  added: []
  patterns: [FTS5 external-content rebuild + backfill migration, bound-parameter MATCH escaping, store-side excerpt helper]

key-files:
  created: []
  modified: [src-tauri/src/store/mod.rs, src-tauri/src/store/queries.rs]

key-decisions:
  - "One-shot re-index inside M17 (not lazy query-time backfill)"
  - "fts_search kept as thin wrapper over fts_search_scoped (sync.rs/worker.rs call sites untouched)"
  - "Store-side Rust excerpt instead of FTS5 snippet() (external-content tables cannot snippet off-table columns)"

patterns-established:
  - "FTS freshness triggers for every write path (msg/body/attachment × insert/update/delete)"
  - "WHEN-gated msg_au so flag/pending churn never rewrites the index"

requirements-completed: [SRCH-01]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Whole-account search matches body text and attachment filenames, BM25-ranked with recent-first tie-break, capped at 200"
    requirement: "SRCH-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_matches_body_text_and_attachment_filename_whole_account"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_ranking_bm25_with_recent_first_tiebreak"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_result_cap_is_200"
        status: pass
    human_judgment: false
  - id: D2
    description: "Multi-folder scoping (None = whole account, set = IN filter) with pending_delete rows hidden"
    requirement: "SRCH-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_folder_set_scoping_filters"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_hides_pending_delete_body_and_attachment_matches"
        status: pass
    human_judgment: false
  - id: D3
    description: "Query escaping (FTS operators degrade to literals) and sanitized redacted snippets"
    requirement: "SRCH-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_query_operators_are_literal_not_syntax"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#search_snippet_is_sanitized_text_without_secrets"
        status: pass
    human_judgment: false
  - id: D4
    description: "M17 forward migration preserves rows and backfills the rebuilt index"
    requirement: "SRCH-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#m17_rebuilds_fts_preserving_rows_with_body_backfill"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#fts_triggers_stay_fresh_on_body_and_attachment_writes"
        status: pass
    human_judgment: false
  - id: D5
    description: "Search path is local-SQLite with no IMAP leg"
    requirement: "SRCH-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#search_path_is_local_sqlite_with_no_imap_leg"
        status: pass
    human_judgment: false

# Metrics
duration: 55min
completed: 2026-10-13
status: complete
---

# Phase 23 Plan 01: FTS Search Engine Summary

**M17 FTS5 rebuild indexing body text + attachment filenames, with folder-set scoped BM25 search, 200-row cap, and redacted text snippets — all local-SQLite**

## Performance

- **Duration:** 55 min
- **Started:** 2026-10-13T20:00:00Z
- **Completed:** 2026-10-13T20:55:00Z
- **Tasks:** 3
- **Files modified:** 2

## Accomplishments

- M17 forward migration rebuilds `messages_fts` with `body_text` + `attachment_names` columns, one-shot backfill, 8 freshness triggers
- `fts_search_scoped`: whole-account default, folder-set narrowing, BM25 + `date_utc DESC` tie-break, `LIMIT 200`, `pending_delete` hidden, operator escaping
- `search_snippet`: store-side match-anchored excerpt from sanitized text with tag-strip + `redact_input`, never `body_html`
- Full store suite green (60 tests); no IMAP verb on any search path

## Task Commits

All three tasks landed in one atomic commit (shared files, plan-level verification):

1. **Task 1: Tracer: body + attachment filename searchable end-to-end** - `5351ce5` (feat)
2. **Task 2: Multi-folder scope, snippets, trigger freshness + backfill** - `5351ce5` (feat)
3. **Task 3: Preserve-rows test + ranking/scope/snippet test suite** - `5351ce5` (feat)

## Files Created/Modified

- `src-tauri/src/store/mod.rs` - M17 migration SQL + `m17_rebuilds_fts_preserving_rows_with_body_backfill` test
- `src-tauri/src/store/queries.rs` - `fts_search_scoped`, `search_snippet`, `SEARCH_RESULT_LIMIT`, 9 new tests

## Decisions Made

- One-shot re-index inside the M17 migration SQL (agent-discretion option; cheapest consistent choice, no lazy backfill machinery).
- `fts_search(mailbox: Option<&str>)` kept as a thin wrapper so `search_messages` and the sync-worker assertion compile untouched; folder sets go through the new `fts_search_scoped`.
- `msg_au` trigger WHEN-gated to FTS-indexed columns so Seen-flag/pending_delete churn never rewrites the index (also fixes pre-existing subject-update staleness).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] FTS5 snippet()/highlight() unusable on the external-content index**
- **Found during:** Task 2 (snippet helper)
- **Issue:** `snippet(messages_fts, …)` fails with `no such column: T.body_text` — for `content='messages'` tables the aux functions resolve columns against the content table, and `body_text` lives in `message_bodies`. Isolated with throwaway debug tests (plain-table snippet works, content-sync fails).
- **Fix:** Store-side excerpt builder (`excerpt_around_hit`): first case-insensitive token hit anchors a ~120-char `…`-marked window over `body_text` (fallback `preview`), then tag-strip + `redact_input`. Throwaway debug test removed.
- **Files modified:** src-tauri/src/store/queries.rs
- **Verification:** `search_snippet_is_sanitized_text_without_secrets` green; plan's agent-discretion clause covers excerpt mechanics
- **Committed in:** 5351ce5 (part of task commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Required for correctness — the planned `snippet()` call cannot work on this index shape. No scope creep; public API (`search_snippet(conn, id, query) -> String`) unchanged.

## Issues Encountered

- Two self-clipped `edit` calls during implementation (dropped M16 const once, comment line once) — both caught immediately by re-read + `cargo check`, restored before commit. No impact on final tree (verified via `git diff` review + green suite).

## Threat Model Coverage

- T-23-01 (MATCH injection): tokens quoted + operator chars stripped, bound parameter only — hostile-query test green.
- T-23-02 (snippet disclosure): sanitized-text sources only, tag-strip, `redact_input` — secret/tag assertions green.
- T-23-03 (DoS): `LIMIT 200` in-SQL, ordering in-SQL.
- T-23-04 (migration tampering): forward-only M17 + preserve-rows test.
- T-23-SC: no new dependencies.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- SRCH-01 backend complete; `fts_search_scoped` + `search_snippet` ready for 23-02 filter re-execution and 23-03 UI.
- Live-gate position: search is local-only, no live server leg applies (test-committed).

---
*Phase: 23-search-filters*
*Completed: 2026-10-13*
