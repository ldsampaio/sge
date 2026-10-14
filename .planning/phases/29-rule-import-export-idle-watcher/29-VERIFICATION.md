# Phase 29 Verification: Rule Import/Export + IDLE Watcher

**Date:** 2026-10-14
**Verdict:** PASSED (headless) — all phase truths hold; live legs SKIP offline per sandbox contract; one accepted deviation (no IMPORT_EXPORT atom in profile → no UI gate).

## Goal-Backward Check

Phase goal: users carry smart-folder rules across machines as validated JSON (IMP-01) and get new INBOX mail via push with poll fallback (IDLE-01), both capability-gated.

### IMP-01 (29-01 + 29-03)

| Truth | Evidence | Status |
|---|---|---|
| Export versioned JSON with id/name/priority/conditions/dest_kind/destination/scope/enabled | `export_rules` + `round_trip_fixpoint` (envelope version 1 asserted in UI-flow test) | ✅ |
| Invalid files rejected with plain-language reasons (empty patterns, reserved system/auto, dup names, priority 1–999, dest_kind) | 6 validation tests asserting exact message text | ✅ |
| Round-trip export→import→export byte-identical | `assert_eq!(json, json2)` byte comparison | ✅ |
| Re-import updates + bumps `rule_version` | `import_existing_id_updates_and_bumps_version` (1→2) | ✅ |
| RulesPanel Export/Import + preview modal + Accept/Reject | Code present, `tsc --noEmit` clean, `import_export_ui_flow` drives the exact UI sequence | ✅ |
| Preview shows all error/warning classes | Modal renders backend `errors`/`warnings` verbatim; backend covers all 6 error classes + folder/category warnings (tested) | ✅ |

### IDLE-01 (29-02 + 29-03)

| Truth | Evidence | Status |
|---|---|---|
| Dedicated connection, never parks on manager session | `idle.rs` holds no manager/lease reference (grep-verified; only `release_handle` name collisions); connects via `connect_sync` per cycle | ✅ |
| SELECT→IDLE→DONE on ALL exits, 29-min re-IDLE, backoff | `drive_cycle` unconditional `end_idle`; `wait()` default 29-min timeout; `backoff_secs` growth+cap tested; verb-order asserted from transcript | ✅ |
| Advisory-only wakeup → SyncWake → SyncGate (skip-if-busy, coalesce) | `SyncWake` tests (burst→1 pass, busy-keeps-dirty); watcher holds no worker/manager ref; UI funnels `idle-wakeup` into `requestSync("poll")` → `start_sync` gate | ✅ |
| Capability gate: IDLE=false → never spawns, poll-only | `spawn_if_allowed` returns None (tested); repo profile atoms `IDLE:false`; `status_of(_, false)` never Push (tested) | ✅ |
| Kill mid-IDLE → DONE + backoff, no duplicate sync, no stall | Stop-interruptible wait (DONE then Closed); `idle_edge_kill_mid_idle_sends_done`; wakeups only set dirty flag | ✅ |
| Sidebar Push/Polling/Offline from `idle_status` | `useIdleStatus` + dot/label, `tsc` clean, `idle_status_command` + status-mapping tests | ✅ |
| IMPORT_EXPORT=false → rules UI disabled | **Deviation:** atom absent from Phase-27 profile (only IDLE/CONDSTORE/QRESYNC); portability is local-file, backend validation authoritative → buttons always enabled. Accepted: matches T-29-12 (UI gate would be UX-only). | ⚠️ accepted |

## Test Totals

- `cargo test -p sge --lib`: **420 passed, 0 failed**, 1 ignored (pre-existing).
- `cargo test -p sge --bins`: 7 targets, all `ok` (live_gate 19+6+15 pass headless).
- `npx tsc --noEmit`: clean.
- Live-gate-shaped tests (`live_import_export_gate_skips_offline`, `live_idle_leg_skips_offline`): SKIP cleanly without `SGE_LIVE_IMAP_USER` — zero network traffic offline.

## Constraints Audit

- `source scripts/build-env.sh` before every cargo invocation; no global `LD_LIBRARY_PATH` (only file creation of the gitignored sidecar stub, uncommitted).
- BODY.PEEK-only: watcher issues no FETCH (only SELECT/CAPABILITY/IDLE/DONE/LOGOUT strings); UIDVALIDITY gating untouched in worker.
- Single-session invariant: no manager import in `idle.rs`.
- Forward-only migration M21 with preserve-rows test; `imap_outbox`/`send_queue` untouched (no diffs in those files).
- Redaction: no bodies/secrets in new code (rule metadata + ids only; wakeups carry no content).
- Zero new deps (Cargo.toml untouched; `futures`/`async-std` already direct deps).
- No SMTP/send changes; no `.live-test.env` created.

## Gaps / Human-Needed

None blocking. Manual UAT suggested (not required): with an IDLE-capable server (profile atom true), log in → sidebar dot turns green Push; send self-mail → wakeup syncs within seconds; Import in RulesPanel with a hand-edited invalid JSON → red errors, Accept disabled. Cannot be performed headlessly against `mail.utfpr.edu.br` (IDLE=false, operator creds required).
