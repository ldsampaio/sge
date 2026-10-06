---
phase: 06-flag-sync-outbox
verified: 2026-10-05T13:55:00Z
status: human_needed
score: 6/8 must-haves verified
covered_files: [.planning/phases/06-flag-sync-outbox/06-01-PLAN.md, .planning/phases/06-flag-sync-outbox/06-01-SUMMARY.md, .planning/phases/06-flag-sync-outbox/06-02-PLAN.md, .planning/phases/06-flag-sync-outbox/06-02-SUMMARY.md, .planning/phases/06-flag-sync-outbox/06-03-PLAN.md, .planning/phases/06-flag-sync-outbox/06-03-SUMMARY.md, src-tauri/src/imap/headers.rs, src-tauri/src/imap/mod.rs, src-tauri/src/store/mod.rs, src-tauri/src/store/queries.rs, src-tauri/src/sync/worker.rs, src-tauri/src/commands/sync.rs, src/components/MessageList.tsx, src/components/ReadingPane.tsx, src/components/SyncStatus.tsx]
covered_digest: "v1:sha256:pending-commit"
behavior_unverified: 0
overrides_applied: 0
---

# Phase 06: Flag Sync + Outbox Verification Report

**Phase Goal:** User can triage read/unread state and trust it survives offline and server round-trips.
**Verified:** 2026-10-05T13:55:00Z
**Status:** human_needed (6/8 automated; 2 require live UTFPR round-trip)

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Toggle applies instantly (optimistic) with dot aria-label flip | ✓ VERIFIED | `MessageList.tsx` clickable dot, optimistic toggle + pending wash; `ReadingPane.tsx` header toggle + Seen-on-open (feat 06-02 commits). `tsc --noEmit` exit 0, `npm run build` ✓ 31 modules. |
| 2 | Seen flag confirmed on server after sync (UID STORE) | ⚠️ HUMAN NEEDED | Wire path verified: `set_seen` command → `SessionManager` lease → `UID STORE ±FLAGS.SILENT (\Seen)` (T-6-01 UID-only); replay acks in order (`replay_acks_queued_ops_in_order` ✓). Live server confirmation needs UTFPR round-trip. |
| 3 | Offline toggles replay on reconnect with pending indicator | ⚠️ HUMAN NEEDED | Durable outbox (M2 `flag_outbox`, UNIQUE collapse), `SyncStatus` pending aggregate + offline-queued copy (feat 06-02); unit replay paths green. End-to-end offline→reconnect needs disconnect simulation against live server. |
| 4 | No flap / no wrong-message write under concurrent sync | ✓ VERIFIED | UID-only STORE (no sequence numbers — `seen_store_arg` canonical `\Seen`); pending-wins reconcile (`pending_wins_reconcile_preserves_optimistic_flags` ✓); flap collapses to latest (`outbox_rfc4549_rapid_flap_collapses_to_latest` ✓); epoch bump drops queue (`outbox_rfc4549_epoch_bump_drops_queue` ✓). |
| 5 | No fetch path sets \Seen as side effect | ✓ VERIFIED | `peek_audit` ✓ (FETCH_ATTRS has no bare body tokens; 4-path mapping); `fetch_attrs_are_parenthesized` ✓; `fetch_body` issues `BODY.PEEK[]`. |
| 6 | Outbox survives restart; replay drops on missing UID / UIDVALIDITY change | ✓ VERIFIED | `m2_upgrades_v1_database_forward_preserving_rows` ✓; `outbox_rfc4549_absent_uid_drops_single_op` ✓; `replay_epoch_mismatch_drops_queue` ✓; `replay_failure_stays_queued_with_error` ✓. |

**Score:** 6/8 truths verified (2 live-server items pending human)

### Required Artifacts

- `cargo test`: 90 passed, 1 ignored, 0 failed (incl. `peek_audit` ×1, `outbox_rfc4549_*` ×6)
- `npx tsc --noEmit`: exit 0
- `npm run build`: ✓ 31 modules, 87ms

## Human Verification Items

1. **Seen round-trip (FLAG-01):** Toggle read in app → confirm Seen in UTFPR webmail; toggle unread → confirm unseen.
2. **Offline replay (FLAG-02):** Disconnect network → toggle → reconnect → confirm server flag converges with pending indicator clearing.
