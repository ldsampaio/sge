# Plan 10-03 Summary: imap_outbox (M7) + pre-sweep replay + delete/move/expunge commands

**Status:** Complete. All Wave 1 (3 tasks) + Wave 2 (2 tasks) + Wave 3 (1 task) done. Full backend suite green, clippy identical to baseline, tsc clean.
**Date:** 2026-10-06

## Commits

| Task | Commit | SHA |
|---|---|---|
| W1 (M7 migration + outbox queries + optimistic state) | `feat(10-03): M7 imap_outbox migration + queries + pending_delete` | `b3f3644` |
| W2 (replay + convergence + SyncSummary + cleanup) | `feat(10-03): pre-sweep imap replay + convergence gate + SyncSummary counters` | `4037511` |
| W3 (four Tauri commands + Trash resolve + sync_status) | `feat(10-03): delete/move/expunge/undo Tauri commands + Trash resolve` | `5c0a008` |

## Tasks completed

1. **M7 migration** — `SCHEMA_VERSION` 6 → 7: `imap_outbox(id, mailbox_id FK CASCADE,
   uid, op CHECK delete|move, dest_mailbox NULL, seen_intent NULL, uid_validity,
   created_at, attempts, last_error, UNIQUE(mailbox_id,uid))` + index +
   `messages.pending_delete` (hidden flag; FTS `msg_ai`/`msg_ad` triggers intact).
   `flag_outbox` contract untouched (asserted by the M7 preserve-rows test).
2. **Outbox queries** — `enqueue_imap_outbox` (latest-wins upsert + consumes the
   same-key `flag_outbox` row, capturing Seen intent only for `move`),
   `list/pending/delete/drop/record_error/count` mirrors, `set/is_pending_delete`,
   `undo_pending_op` (valid iff still queued), `expunge_uids_local` (explicit
   bodies/parts deletes — `PRAGMA foreign_keys` is OFF, verified — + queued-op
   drops, returns removed UIDs). `delete_missing_uids` wipe branch drops the imap
   queue (RFC 4549); all branches delete bodies/parts explicitly. `list_messages`
   + `fts_search` filter `pending_delete` rows.
3. **Pre-sweep replay** — `replay_imap_outbox` (copy of `replay_outbox` shape):
   epoch check first (whole-mailbox drop + flags cleared so the sweep reconciles
   those rows as ordinary mail), absent-UID single drop, per-op dispatch as a
   server-side move via shared `run_move_on_session` (new `pub` wrapper over the
   manager's `run_move_sequence` — one implementation, two callers). `delete`
   replays as move-to-stored-Trash (no STORE-\Deleted path exists — delete IS
   move-to-Trash). Move-carries-toggle resolves the dest UID by Message-ID
   SEARCH and applies Seen, always re-SELECTing src after; unresolvable intents
   log-and-drop. `flag_outbox` deliberately stays post-sync (disjoint queues —
   enqueue consumes same-key flag ops — so toggles never race moves).
4. **Refusal cleanup (Wave-B residual)** — `SyncError::Refused` after the COPY leg
   drops the op + clears the hidden flag with a log, never retries (retrying
   would re-COPY duplicates only to refuse again). Covered by
   `imap_replay_refusal_drops_with_log` (new `MockSession.fail_move_refused` arm).
5. **Convergence + counters + cleanup** — pending gate and pending-wins both union
   flag + imap depths; `SyncSummary.moved` (every acked imap op moves server-side)
   + `expunged` (reserved 0 — the expunge command deletes rows directly, no pass
   attribution yet); `BatchCompleted` carries both (frontend destructures — tsc
   clean, UI wires in 10-04). `total()` unchanged. `expunge_absent` choke point
   for all three expunge sites (bump wipe, empty shortcut, Step 6): DB rows +
   FTS ghosts + bodies/parts + best-effort `<app_data>/attachments/<uidv>/<uid>/`
   removal (fs errors log, never fail sync; `None` root skips). Empty shortcut
   now reports `deleted` (was discarded).
6. **Commands** (`set_seen` skeleton verbatim: INBOX-default mailbox,
   `load_account_config` → `manager_for` → `spawn_blocking`+`block_on`, lock never
   across await, `{ acked, pending_count, detail }` with both-queue depth):
   `delete_message` (confirm-gated CREATE: `need_trash_confirm:` error first,
   `create_trash=true` retry CREATEs once; per-account `trash_cache`),
   `move_message` (raw wire dest, src==dest guard, restore-out-of-Trash is
   `dest=INBOX`), `expunge_messages` (UID-scoped chunked `uid_expunge_in` only —
   never bare `expunge()`; local delete + attachment cleanup only on success, no
   optimistic write so no rollback), `undo_queued_op` (store-only, `restored`
   false once replayed). `sync_status.pending_count` sums both queues. All four
   registered in `lib.rs`; `AppState.app_data` threaded to worker + expunge.

## Verify results

- `cargo test -p sge --lib store::` — 33 passed (M7 preserve-rows + 6 query tests).
- `cargo test -p sge --lib sync::` — 45 passed (9 imap-replay/cleanup tests).
- `cargo test -p sge --lib commands::` — 2 passed (incl. both-queues pending test).
- `cargo test -p sge --lib` (full) — **156 passed, 0 failed, 1 ignored** (was 139; +17 new).
- `cargo clippy --all-targets` — 11 warnings, all pre-existing (same set as baseline).
- `npx tsc --noEmit` — clean (additive backend only; `BatchCompleted` extension is
  destructure-safe).

## Design notes for Plan 10-04

- Pending rows stay hidden after ack until the sweep's expunge-diff removes them
  the same pass; undo is valid iff the op is still queued (`undo_queued_op`).
- `delete_message` without Trash returns `need_trash_confirm:`-prefixed error —
  UI matches the prefix for the confirm modal, then retries with
  `create_trash: true`. Note Tauri arg naming: `create_trash` (snake_case, like
  existing `mailbox`).
- `SyncSummary.expunged` stays 0 until a pass-attributed expunge exists; `moved`
  counts every acked imap op. `BatchCompleted` now carries both.
- `reconnect()` capability-cache invalidation remains untestable offline (Wave-B
  residual, carried).
- No live-gate run in this plan (MockSession only); 10-04 owns the UTFPR
  round-trips (move/expunge/offline-replay/foreign-`\Deleted` survival).

## Blockers

None. No task failed; one deliberate deviation documented: `delete` replay is a
move-to-stored-Trash (plan text mentioned `mark_deleted_in`, which would skip
Trash and break DEL-01 semantics). No STATE.md / ROADMAP.md changes.
