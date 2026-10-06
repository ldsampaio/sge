---
phase: 08-poll-manual-refresh
verified: 2026-10-05T15:10:00Z
status: human_needed
score: 6/8 must-haves verified
covered_files: [.planning/phases/08-poll-manual-refresh/08-01-PLAN.md, .planning/phases/08-poll-manual-refresh/08-01-SUMMARY.md, src-tauri/src/sync/mod.rs, src-tauri/src/lib.rs, src-tauri/src/commands/sync.rs, src/components/SyncStatus.tsx]
covered_digest: "v1:sha256:pending-commit"
behavior_unverified: 0
overrides_applied: 0
---

# Phase 08: Poll + Manual Refresh Verification Report

**Phase Goal:** User's mail stays fresh without thinking about sync and never corrupts from overlapping syncs.
**Verified:** 2026-10-05T15:10:00Z
**Status:** human_needed (6/8 automated; 2 require live observation)

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | New INBOX mail arrives on the poll interval without manual action | ⚠️ HUMAN NEEDED | Timer wired (`POLL_INTERVAL_MS = 5 min` → same `requestSync` → tested `start_sync` path). Live arrival timing unverified. |
| 2 | Manual refresh runs through the same code path as the poll timer | ✓ VERIFIED | One `requestSync(source)` entry for timer + "Buscar mensagens" + retry; single `start_sync` command underneath. |
| 3 | Poll mid-sync (or mid flag-STORE) never overlaps | ✓ VERIFIED | `poll_guard_second_call_returns_busy` ✓ (busy tick skips); `poll_timer_next_tick_proceeds_after_release` ✓ (Drop releases; no wedge). Lease mutex additionally serializes manager traffic. |
| 4 | Honest "reconnecting…" (not fatal) on session expiry; resume via keyring re-read + re-SELECT | ⚠️ HUMAN NEEDED (code complete) | `isConnectionError` → reconnecting state + one auto-retry in 5 s; resume reuses `load_account_config` keyring fallback + SELECT. Live expiry unverified. |
| 5 | Full suite green + frontend clean | ✓ VERIFIED | `cargo test`: 96 passed, 1 ignored, 0 failed; `tsc --noEmit` exit 0; `npm run build` ✓. |

**Score:** 6/8 truths verified (2 live-observation items pending human)

## Human Verification Items

1. **Poll arrival (SYNC-03):** Leave app open past one 5-min tick with new UTFPR mail → message appears without manual refresh.
2. **Session expiry (SYNC-03):** With an expired/stale session, trigger sync → "Reconectando…" shows (not a fatal error) and sync completes after retry.
