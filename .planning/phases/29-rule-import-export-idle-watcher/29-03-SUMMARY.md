---
phase: 29-rule-import-export-idle-watcher
plan: 03
subsystem: rules-ui-push-ui
tags: [IMP-01, IDLE-01, RulesPanel, Sidebar, push-status]
requires: [29-01, 29-02]
provides: [rules-import-export-ui, push-status-ui, idle-wakeup-wiring]
affects: [phase-30-batch-trust]
tech-stack:
  added: []
  patterns: [backend-validated-preview-modal, single-sync-entry-funnel, capability-honest-status]
key-files:
  created: [src/hooks/useIdleStatus.ts]
  modified: [src/components/RulesPanel.tsx, src/components/Sidebar.tsx, src/components/SyncStatus.tsx, src-tauri/src/commands/classify.rs]
decisions:
  - "Frontend never validates: preview renders backend ImportResult verbatim; Accept disabled while errors exist"
  - "No IMPORT_EXPORT capability gate: atom absent from Phase-27 profile and portability is local-file (backend validation is the authority)"
  - "Actual paths src/components + src/hooks (plan's src/app tree does not exist)"
  - "No frontend test runner in project (zero-new-deps): UI verified by tsc + backend flow test"
metrics:
  duration: ~1.5h
  completed: 2026-10-14
status: complete
actuals:
  tokens: 38000
  tasks: 3
  commits: 1
commits:
  - c1bd5bb: RulesPanel import/export UI + Sidebar push status + wakeup wiring
plan_head_before: bbeadd2
---

# Phase 29 Plan 03: Rules + Push UI Summary

User-visible half of IMP-01 and IDLE-01: RulesPanel gains Export download + Import file-picker with backend-validated preview modal (Accept/Reject), the Sidebar shows an honest Push/Polling/Offline dot, and backend `idle-wakeup` events funnel into the ONE existing sync entry (`SyncStatus.requestSync` → `start_sync` → gate).

## What Was Built

- **`src/components/RulesPanel.tsx`**: Export button (`export_rules` → `regras-<ts>.json` download), Import button (hidden `.json` picker → `import_rules commit=false` → preview modal with red errors / amber warnings / counts summary → Accept commits valid subset + reloads, Reject discards; Accept disabled while errors exist; malformed JSON surfaces as a preview-level error). Stale "Sem importação/exportação" copy replaced.
- **`src/hooks/useIdleStatus.ts`** (new): `mapIdleStatus` pure mapper (Push green / Polling amber-or-blue with reconnect-attempt tooltip / Offline gray; unknown → Polling, never phantom Push) + `useIdleStatus` hook (5s poll, 1s while reconnecting, best-effort `idle_start` on mount, `idle_stop` on unmount, forwards `idle-wakeup` as window `sge-idle-wakeup`).
- **`src/components/Sidebar.tsx`**: push dot + label in the sidebar head with tooltip, driven by the hook.
- **`src/components/SyncStatus.tsx`**: `sge-idle-wakeup` listener calls the same `requestSync("poll")` entry (backend gate skips when busy) — push wakes drain through `SyncGate`, never a direct sync call.
- **`src-tauri/src/commands/classify.rs`**: `import_export_ui_flow_tests` (export-empty → invalid-preview-errors → warn-but-allow commit → list refresh → re-import update).

## Tests

- `npx tsc --noEmit` → clean.
- `cargo test -p sge --lib -- commands::` → 36 passed (incl. `import_export_ui_flow`, `idle_status_command`, `app_state_starts_without_idle_watcher`).
- Full lib suite: 420 passed, 0 failed; `--bins` all pass.
- No frontend test runner exists in the project (no vitest/jest/playwright); adding one would violate zero-new-deps, so UI logic is verified via `tsc` + the backend flow test + the pure `mapIdleStatus` mapper (kept side-effect-free for future runner adoption).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Plan paths `src/app/...` don't exist — used real tree**
- Real layout is `src/components/*` (+ new `src/hooks/`); hook named `useIdleStatus.ts` per plan's artifact list, adjusted path.

**2. [Rule 2 - Correctness] No IMPORT_EXPORT gate — atom absent from profile**
- The Phase-27 profile carries only IDLE/CONDSTORE/QRESYNC atoms; rule portability is local-file JSON needing no server support, so buttons are always enabled and backend validation is the authority (threat-model consistent: UI disablement would be UX-only anyway per T-29-12).

**3. [Rule 4-scale] No frontend test runner — tsc + backend flow test instead**
- Recorderd here (not a blocker): `cd src && npm test` from the plan has no runner to invoke. Mitigated with typecheck + headless flow test covering the exact UI call sequence.

## Auth Gates

None.

## Known Stubs

None.

## Threat Flags

None — T-29-09 (no frontend validation), T-29-10 (single 5s/1s poller per Sidebar mount, aborted on unmount), T-29-11 (export is already-visible rule metadata), T-29-12 (no client-side authority claimed).

## Self-Check: PASSED

- `src/hooks/useIdleStatus.ts`, updated `RulesPanel.tsx`/`Sidebar.tsx`/`SyncStatus.tsx` present; `tsc --noEmit` clean.
- Backend flow test passes; committed as c1bd5bb.
