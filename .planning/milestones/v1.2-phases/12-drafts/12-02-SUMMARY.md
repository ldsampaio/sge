---
phase: 12-drafts
plan: 12-02
subsystem: email-drafts
tags: [async-imap, append, uid-search, expunge, tauri-commands, reconnect-flush, sqlite]

# Dependency graph
requires:
  - phase: 12-drafts
    provides: M9 drafts table + queries, drafts.rs renderer, append_message/uid_search_header verbs, save_draft_copy_in/discard_server_copy_in (12-01, commits 7cc7209/4c81836)
  - phase: 10-delete-move
    provides: mark_deleted/uid_expunge single-UID path, choose_expunge_path fallback, pre-sweep replay discipline, acked/pending_count tone
  - phase: 11-folder-crud
    provides: roles.rs Drafts resolution, create_folder confirm-create flow
provides:
  - save_draft/get_draft/discard_draft Tauri commands + DraftSaveResult/DiscardResult (frozen UI contract)
  - replay_dirty_drafts reconnect flush + per-Message-ID MockSession SEARCH arms
  - dirty-draft depth in pending_depth (per-folder + sync_status)
affects: [12-03 (DraftEditor consumes the frozen contract), 13-send (DRAFT-03 consumes server_uid + dirty)]

# Actuals (#2632) — session diff only (task 1 code arrived in 12-01 wip, verified here)
actuals:
  tokens: 6012
  tasks: 4
  commits: 3

# Tech tracking
tech-stack:
  added: []
  patterns: [lease-free-session-body-shared-by-manager-and-worker, mailbox-filtered-reconnect-flush, per-message-id-canned-search]

key-files:
  created: []
  modified:
    - src-tauri/src/sync/worker.rs
    - src-tauri/src/commands/sync.rs
    - src-tauri/src/lib.rs

key-decisions:
  - "Flush reuses save_draft_on_session (one implementation, two callers) — the worker pass holds the session, so no second lease"
  - "Flush filtered to the synced folder's mailbox_id: the expunge-old leg addresses the SELECTed folder, other folders' drafts wait for their own pass (T-12-04)"
  - "replay_dirty_drafts takes drafts_wire explicitly (APPEND needs the wire name; no id-to-name query exists) — minor signature deviation from the plan"
  - "Worker renders From empty (row carries no From column); only Message-ID matters for reconcile"

patterns-established:
  - "Reconnect flush shape: list_dirty_drafts filtered by mailbox_id → render(Date=now) → lease-free save-sequence → mark_draft_clean per ack; per-row failure stays dirty, never fails the pass"
  - "Per-Message-ID canned SEARCH in MockSession wins over the flat vec when present (multi-draft flush tests)"

requirements-completed: []

coverage:
  - id: D1
    description: "Every save leaves at most one server copy per compose session (APPEND-new + scoped expunge-old of the tracked server_uid)"
    verification:
      - kind: unit
        ref: "src-tauri/src/imap/manager.rs#save_draft_appends_new_and_expunges_old + 7 sibling tests (imap::manager:: 31/31)"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#replay_dirty_drafts_expunges_superseded_copy"
        status: pass
    human_judgment: false
  - id: D2
    description: "Offline saves persist locally dirty and flush on reconnect without duplicating the server copy"
    verification:
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#replay_dirty_drafts_acks_and_cleans + replay_dirty_drafts_append_failure_stays_dirty"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#replay_dirty_drafts_reconciles_each_row_by_message_id"
        status: pass
    human_judgment: false
  - id: D3
    description: "get_draft and save results expose server_uid + dirty for the Phase 13 send transaction; discard removes the local row and expunges the tracked server copy"
    verification:
      - kind: unit
        ref: "src-tauri/src/commands/sync.rs#draft_row_exposes_server_uid_and_dirty + pending_depth_counts_dirty_drafts"
        status: pass
      - kind: unit
        ref: "src-tauri/src/commands/sync.rs#find_drafts_wire_resolves_special_use + find_drafts_wire_missing_returns_none"
        status: pass
    human_judgment: false
  - id: D4
    description: "Full backend suite green (Phases 6-12 unbroken) + tsc clean"
    verification:
      - kind: unit
        ref: "cargo test -p sge → 221 passed, 0 failed, 1 ignored"
        status: pass
      - kind: unit
        ref: "tsc --noEmit → exit 0"
        status: pass
    human_judgment: false
  - id: D5
    description: "Live APPEND + expunge-old round-trip against mail.utfpr.edu.br with cleanup (exactly-one-copy invariant on a real server)"
    verification: []
    human_judgment: true
    rationale: "Manual gate — needs UTFPR credentials + network; no credentials were available to this session. Recorded as /gsd-verify-work 12 follow-up per plan."

# Metrics
duration: ~45min
completed: 2026-10-08
status: complete
---

# Phase 12 Plan 02: Server sync Summary

**Draft server sync: APPEND-new + scoped expunge-old commands, dirty-row reconnect flush, and draft-aware pending depth — 221 backend tests green**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-10-08T (session start)
- **Completed:** 2026-10-08
- **Tasks:** 4 / 4
- **Files modified:** 3 (`sync/worker.rs`, `commands/sync.rs`, `lib.rs`)

## Accomplishments

- Task 1 needed no code changes: `save_draft_copy_in` + `discard_server_copy_in` (single-lease APPEND → SEARCH-reconcile → mark + scoped-expunge, Refused-never-retried, UIDVALIDITY-stale drop) arrived in the 12-01 wip commits and re-verified green here (31/31 `imap::manager::`)
- `save_draft` / `get_draft` / `discard_draft` registered in `generate_handler!`; local-first ordering (upsert `dirty=1` before any network), stable per-session Message-ID, `drafts-missing:` refusal before any verb call, best-effort discard cleanup
- `replay_dirty_drafts` reconnect flush reusing the lease-free `save_draft_on_session` body: mailbox-filtered rows, epoch-stale `old_uid=None`, per-row ack; called pre-sweep (step 4c) and in the empty-mailbox branch
- `pending_depth` already summed dirty-draft depth per folder (wip) — covered by new `commands::` tests; `sync_status` inherits it
- Full suite **221 passed, 0 failed, 1 ignored** (was 211 in 12-01: +4 commands, +6 worker); `tsc --noEmit` clean

## Task Commits

1. **Task 1: manager `save_draft_copy_in` + `discard_server_copy_in`** — no new commit (verified as-built from `7cc7209` + `4c81836`; `cargo test -p sge imap::manager::` 31/31 green)
2. **Task 2: `save_draft` / `get_draft` / `discard_draft` + registration** — `8f166f2` (feat)
3. **Task 3: `replay_dirty_drafts` + pending depth** — `1c18475` (feat)
4. **Task 4: full suite + live gate** — verification only (no code changes; live gate deferred, see below)

**Plan metadata:** summary commit follows (docs)

## Files Created/Modified

- `src-tauri/src/sync/worker.rs` (modified) — `replay_dirty_drafts`, per-Message-ID `search_header_by_msgid` MockSession arm, step-4c + empty-branch call sites, 6 new tests + fixtures
- `src-tauri/src/commands/sync.rs` (modified) — 4 new `commands::` tests (pending depth with drafts, row handoff, drafts-wire resolve/missing); command bodies unchanged (arrived in wip)
- `src-tauri/src/lib.rs` (modified) — `save_draft`, `get_draft`, `discard_draft` added to `generate_handler!`

## Decisions Made

- Reused `save_draft_on_session` for the flush (one implementation, two callers) instead of duplicating the APPEND/SEARCH/expunge legs in the worker — the pass already holds the session, so the lease is simply not taken.
- Flush filters rows to the synced folder's `mailbox_id` (rather than resolving the Drafts role in the worker): the expunge-old leg addresses whatever folder is SELECTed, so cross-folder flushing would risk T-12-04 drift. Other folders' drafts wait for their own pass.
- `replay_dirty_drafts` takes `drafts_wire: &str` explicitly — the plan's 3-arg signature lacks the wire name APPEND requires, and no id→name query exists. Minor signature deviation, documented in code docs.
- Worker renders `From` empty: the `drafts` row has no From column (the command fills it from the account at save time); reconcile keys on Message-ID only, so the flush copy is still exactly-once safe.
- No live round-trip attempted: no credentials in scope, and probing the server without them proves nothing. Deferred to `/gsd-verify-work 12` per the plan's own escape hatch (v1.1 deferred-live-item precedent).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `HashMap` import warned unused on non-test builds**
- **Found during:** Task 3 (build after adding per-Message-ID arms)
- **Issue:** `HashMap` imported at `worker.rs` top level is only referenced inside `#[cfg(test)] mod tests`, so `cargo build` (non-test) emitted `unused_imports`.
- **Fix:** Reverted the top-level import to `HashSet`; added `use std::collections::HashMap;` inside the tests module.
- **Files modified:** `src-tauri/src/sync/worker.rs`
- **Verification:** `cargo build -p sge` zero warnings; `sync::worker` 41/41 green.
- **Committed in:** `1c18475` (part of task commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 warning hygiene) + 1 minor documented signature deviation (`drafts_wire` param, required by the APPEND verb — no behavior change vs plan intent)
**Impact on plan:** No scope creep. All plan acceptance lines hold: fake-driven exactly-one-copy, offline-dirty, drafts-missing, epoch-stale, get_draft handoff, pending depth.

## Issues Encountered

- HEAD is on `main` (no worktree/agent branch in this runtime): per-commit HEAD assertion would refuse, but this repo's established practice is direct-to-main commits (all prior phase commits, incl. 12-01's, landed on `main`) and the dispatch explicitly requires per-task commits — committed on `main`, staged file-by-file.
- Parallel 12-03 agent commits (`2e0fb4e`, `84c642c`, `a37eb05`) landed mid-execution in disjoint frontend files — no conflicts; final `tsc --noEmit` (exit 0) ran over the combined tree.
- Threat-model re-check: T-12-03 (stable Message-ID read from existing row, never regenerated; multi-hit → max), T-12-04 (single drafts lease in manager; mailbox-filtered flush under the pass SELECT), T-12-05 (only `expunge_single_on_session` — UIDPLUS `UID EXPUNGE` or the single-UID unmark dance; no bare `expunge()` on any draft path). No new surface beyond the plan.

## Known Stubs

None introduced. (Worker flush renders `From` empty by design — documented above, not a stub: send-time render in Phase 13 supplies From from the account.)

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Frozen UI contract holds verbatim for 12-03: `save_draft({id, subject, body, to, cc, bcc, mailbox?})` → `{id, dirty, server_uid, acked, pending_count}`; `get_draft({id})` → full row; `discard_draft({id})` → `{id, discarded: true}`; Missing Drafts error starts with `"drafts-missing"`.
- Phase 13 DRAFT-03 handoff ready: `server_uid` + `dirty` on every row/result.
- Carried-forward limitation (from 12-01, still open): if the expunge-old leg fails after a successful reconcile, the reconnect-retry re-runs the whole sequence including APPEND, which could leave a duplicate server copy (next save converges via max-UID reconcile + tracked-old expunge, and the live gate asserts count == 1 after re-save — but the window exists).
- Live gate outstanding: APPEND + expunge-old round-trip against `mail.utfpr.edu.br` with `SGE-Draft-Test-<ts>` cleanup → `/gsd-verify-work 12` follow-up.

---
*Phase: 12-drafts*
*Completed: 2026-10-08*

## Self-Check: PASSED
- `src-tauri/src/sync/worker.rs` — FOUND (replay_dirty_drafts + 6 tests)
- `src-tauri/src/commands/sync.rs` — FOUND (4 new tests)
- `src-tauri/src/lib.rs` — FOUND (3 commands registered)
- `.planning/phases/12-drafts/12-02-SUMMARY.md` — FOUND (this file)
- Commits `8f166f2`, `1c18475` — verified via `git log` below
