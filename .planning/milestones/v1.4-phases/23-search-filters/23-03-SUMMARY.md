---
phase: 23-search-filters
plan: "03"
subsystem: ui
tags: [react, tauri-invoke, typescript, search-ui]

# Dependency graph
requires:
  - phase: 23-01-search-engine
    provides: fts_search_scoped + search_snippet (snippet text per hit)
  - phase: 23-02-filter-backend
    provides: 8 filter/state commands (CRUD, apply, search_scoped, save/load state)
provides:
  - SRCH-01/02/03 user-visible feature (scoped search UI, filter panel, state restore)
affects: [future-rules, verify-work-uat]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 8400
  tasks: 3
  commits: 1

# Tech tracking
tech-stack:
  added: []
  patterns: [move-menu-class dropdown reuse, text-node snippet bolding, restore-guard persisted view]

key-files:
  created: [src/components/SearchScopeSelector.tsx, src/components/SavedFiltersPanel.tsx]
  modified: [src/components/SearchBar.tsx, src/components/MailboxView.tsx, src/components/MessageList.tsx]

key-decisions:
  - "Date sort re-sorts client-side over the capped 200 (no Rust change, same engine)"
  - "Shared search types live in component files (types.ts out of scope)"
  - "Persist guard blocks state saves until launch restore completes"

patterns-established:
  - "Scope/filter types exported from their component modules (SearchScope, SavedFilterRow, SearchHit)"
  - "Backend errors shown verbatim; panel never invents success"

requirements-completed: [SRCH-01, SRCH-02, SRCH-03]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Debounced scoped search (All/current/multi) renders ranked results with count+scope+clear header; cross-folder click jumps folders"
    requirement: "SRCH-01"
    verification:
      - kind: other
        ref: "npx tsc --noEmit (clean)"
        status: pass
    human_judgment: true
    rationale: "Rendered dropdown, header copy, and jump behavior need a human to see and click in the app window"
  - id: D2
    description: "Filter save/apply/rename/delete from the panel with verbatim backend errors; snippets bolded as text; 200-cap hint and pt-BR empty states"
    requirement: "SRCH-02"
    verification:
      - kind: other
        ref: "npx tsc --noEmit (clean)"
        status: pass
    human_judgment: true
    rationale: "Panel flows, bolding visuals, and empty-state copy need human eyes in the app window"
  - id: D3
    description: "Restart and reconnect restore the exact search view; empty query restores folder list; hidden rows stay hidden"
    requirement: "SRCH-03"
    verification:
      - kind: other
        ref: "npx tsc --noEmit (clean)"
        status: pass
    human_judgment: true
    rationale: "Restart/reconnect restore can only be confirmed by relaunching the app and observing the view"
  - id: D4
    description: "No new route/pane; 300 ms debounce kept; Phase 18 badges unchanged; redaction invariant holds"
    requirement: "SRCH-01"
    verification:
      - kind: other
        ref: "grep DEBOUNCE_MS + CategoryBadge + Route audit (all hold)"
        status: pass
    human_judgment: false

# Metrics
duration: 60min
completed: 2026-10-13
status: complete
---

# Phase 23 Plan 03: Search + Filters UI Summary

**Scoped search box with All/current/multi selector, filter dropdown panel, results header with 200-cap hint and bolded snippets, plus launch/reconnect state restore — tsc clean**

## Performance

- **Duration:** 60 min
- **Started:** 2026-10-13T21:45:00Z
- **Completed:** 2026-10-13T22:45:00Z
- **Tasks:** 3
- **Files modified:** 5

## Accomplishments

- `SearchScopeSelector`: All / current-folder / multi-select checkbox dropdown reusing `move-menu*` classes; whole-account default keeps "Buscar em todas as pastas" direction
- `MessageList` search path uses `search_scoped` (one local FTS invoke per debounced pause): results header (count + scope + clear), 200-cap hint, sanitized snippets with `<strong>` bolding via text nodes only, pt-BR empty states, cross-folder jump chip kept
- `SavedFiltersPanel` off the search bar: save-current, apply, rename, delete (immediate, no confirm), backend errors verbatim, empty-list create affordance in pt-BR
- Restore: launch loads `load_search_state` with scope/filter re-validation + stale fallback; reconnect re-runs via existing refresh; empty query renders the normal folder list; `pending_delete` hiding unchanged (backend predicate)
- `npx tsc --noEmit` clean; DEBOUNCE_MS still 300; Phase 18 badges render unchanged; no new route or pane

## Task Commits

All three tasks landed in one atomic commit (shared files, plan-level verification):

1. **Task 1: Tracer: scoped search box to results list, one path** - `77b8e89` (feat)
2. **Task 2: Saved-filter panel + result cap hint + snippet bolding** - `77b8e89` (feat)
3. **Task 3: State restore on launch + reconnect, pending_delete hiding** - `77b8e89` (feat)

## Files Created/Modified

- `src/components/SearchScopeSelector.tsx` - Scope picker + `SearchScope` type, `scopeToFolders`, `scopeLabel` helpers
- `src/components/SavedFiltersPanel.tsx` - Filter dropdown + `SavedFilterRow`/`SearchSort` types
- `src/components/SearchBar.tsx` - `initialValue` seed (restore/clear) + `addon` slot; debounce untouched
- `src/components/MailboxView.tsx` - Scope/sort/filter state, persist-on-change, launch restore with re-validation, panel toggle, reconnect re-run
- `src/components/MessageList.tsx` - `search_scoped` path, `SearchHit` type, results header, cap hint, snippet bolding, empty states

## Decisions Made

- `sort: "date"` re-sorts client-side over the capped 200 hits (ISO `date_utc` strings sort lexicographically) — no Rust change, same engine, within the frontend-only file scope.
- Shared search types live in the component files (`SearchScope`, `SavedFilterRow`, `SearchHit`) because `src/types.ts` is outside this plan's file scope.
- `restoredRef` persist guard: SearchBar's mount emission (`onSearch("")`) must not overwrite the saved view before the async launch restore completes.
- New dropdowns reuse `move-menu*`/`btn`/`pager-info` classes (no CSS files in scope); visual polish beyond existing tokens is deferred, not silently skipped.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Duplicated list-branch block after search-branch edit**
- **Found during:** Task 1 (MessageList search-path rework)
- **Issue:** An edit replacing the search branch left the old list-branch lines duplicated (two `setMessages` paths, dead code)
- **Fix:** Removed the duplicated block; verified region by re-read, `tsc` clean
- **Files modified:** src/components/MessageList.tsx
- **Verification:** `npx tsc --noEmit` clean; list/search branches re-read correct
- **Committed in:** 77b8e89 (part of task commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Mechanical edit repair, no behavior change. No scope creep.

## Issues Encountered

- `npm run build` (vite bundle) fails in this environment: Node v18.19.1 vs bundled rolldown-vite requiring `node:util styleText` (Node 20.12+). Pre-existing environmental failure at vite startup — before any project code loads. Per dispatch, `npx tsc --noEmit` is the UI gate and it is clean. Recommend the orchestrator/verifier retry `npm run build` on a Node 20+ host.
- Same self-clipped-edit hazard as 23-01/23-02 (once, `setSelectedMailbox` line); restored immediately, tree verified intact.

## Threat Model Coverage

- T-23-09 (snippet XSS): `renderSnippet` builds `<strong>`/`<span>` text nodes only — never `dangerouslySetInnerHTML` on backend strings (grep-verifiable: no such call in the touched files).
- T-23-10 (spoofed success): panel shows backend errors verbatim; failed save/rename leaves prior state visible.
- T-23-11 (restore tampering): scope folders must exist + filter id must exist before apply; stale ids fall back to whole-account empty query.
- T-23-12 (invoke storms): 300 ms debounce kept; one local FTS invoke per pause; 200-row cap bounds render.
- T-23-SC: no new dependencies.
- Live-gate: this UI issues only local-SQLite invokes on the search path and opens no IMAP session — the live leg is not-applicable (not skipped): backend search/filter paths are test-committed local-only in 23-01/23-02.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- SRCH-01/02/03 user-visible complete: scoped ranked search UI, saved-filter panel, persisted state restore.
- Suggested UAT (human): scoped search across folders, filter save/apply/rename/delete round-trip, relaunch restore, 200-cap hint on a broad query, snippet bolding with HTML-heavy bodies.
- Note for verifier: `npm run build` needs Node 20+; `npx tsc --noEmit` is the green gate in this env.

---
*Phase: 23-search-filters*
*Completed: 2026-10-13*
