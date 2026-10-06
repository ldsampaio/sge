# Phase 8 — Poll + Manual Refresh (Wave 1: Poll infrastructure)

## Summary

**Wave:** 1 of 1 (Wave 2 in the sketch delegated backfill to Phase 9 — no separate wave needed)
**Status:** ✅ Complete (implemented 2026-10-05 gap closure; supersedes the retracted 2026-10-04 summary, which described work that did not exist)
**Depends on:** Phase 7 (Folders + Per-Folder Sync)
**Requirements:** SYNC-03

## Tasks Completed

**Task 1 — Single-flight guard** (`src-tauri/src/sync/mod.rs`, `lib.rs`, `commands/sync.rs`):
- `SyncGate` (AtomicBool) + RAII `SyncGuard` (Drop releases — failures cannot wedge future syncs)
- Held on `AppState` as `sync_gate: Arc<SyncGate>`; `start_sync` clones into the blocking thread and returns `Ok` (skip) when busy
- Verified: `poll_guard_second_call_returns_busy`, `poll_timer_next_tick_proceeds_after_release`

**Task 2 — Poll timer** (`src/components/SyncStatus.tsx`):
- `POLL_INTERVAL_MS = 5 min`; interval effect with cleanup (plus retry-timer cleanup)
- Tick calls the same `requestSync` entry as the manual button; skips when `syncingRef` is set (backend guard is the backstop)
- Poll progress copy ("Verificação automática…") distinguishes timer ticks

**Task 3 — Manual refresh + reconnecting** (`src/components/SyncStatus.tsx`):
- "Buscar mensagens" button + "Tentar de novo" share `requestSync("manual")`
- Connection-shaped failures (`isConnectionError`) → honest "Reconectando…" state + one auto-retry after 5 s; resume reuses `start_sync`'s keyring fallback + re-SELECT
- Timer cleanup on unmount; retry swallowed safely if user syncs first (`syncingRef` guard)

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml poll_guard      # 1 passed
cargo test --manifest-path src-tauri/Cargo.toml poll_timer      # 1 passed
cargo test --manifest-path src-tauri/Cargo.toml                 # 96 passed, 1 ignored, 0 failed
npx tsc --noEmit                                                # exit 0
npm run build                                                   # ✓ built
```

## Files Modified

- `src-tauri/src/sync/mod.rs` — SyncGate/SyncGuard + gate tests
- `src-tauri/src/lib.rs` — AppState.sync_gate
- `src-tauri/src/commands/sync.rs` — single-flight check in start_sync
- `src/components/SyncStatus.tsx` — POLL timer, requestSync entry, reconnecting state
- `.planning/phases/08-poll-manual-refresh/08-01-PLAN.md` — as-built notes
