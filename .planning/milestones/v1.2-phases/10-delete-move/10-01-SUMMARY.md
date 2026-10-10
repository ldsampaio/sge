# Plan 10-01 Summary: Transport — SyncSession delete/move verbs + MockSession arms

**Status:** Complete. All 4 tasks done. Full backend suite green, clippy clean vs baseline.
**Date:** 2026-10-06

## Commits

| Task | Commit | SHA |
|---|---|---|
| 1 + 2 (trait + helpers + BoxedSession impls + mod.rs tests; single commit — trait change cannot compile without impls) | `feat(10-01): SyncSession delete/move verbs + BoxedSession impls` | `8b3b09a` |
| 3 (MockSession arms) | `feat(10-01): MockSession delete/move arms + select recording` | `8ad3861` |
| 4 (worker.rs transport tests; mod.rs tests shipped in `8b3b09a`) | included in above two commits | — |

## Tasks completed

1. **`imap/mod.rs` trait additions** — `store_deleted`, `expunge`, `uid_expunge`,
   `uid_copy_to`, `uid_move_to`, `capabilities` (owned `Vec<String>`), `create_mailbox`,
   all `PinBox` object-safe. Plus `deleted_store_arg`, `MovePath`/`ExpungePath` +
   `choose_move_path`/`choose_expunge_path` (case-insensitive), `UID_SET_CHUNK_SIZE` (200)
   + `chunk_uid_set`. No untagged COPYUID/APPENDUID parsing added.
2. **`BoxedSession` impls** — copied the `set_seen` template verbatim per verb:
   owned Strings + `Box::pin(async move)`, `try_collect` drain-to-completion on every
   stream, `SyncError::Protocol("<VERB> …: {e}")` shapes. UID-only throughout
   (`uid_store`/`uid_expunge`/`uid_copy`/`uid_mv` — no seq `store`/`copy`/`mv`).
   `capabilities()` maps `Capability::{Imap4rev1→"IMAP4rev1", Auth→"AUTH=x", Atom→atom}`.
   Raw wire names passed through for dest (no display_name).
3. **`MockSession` arms (`sync/worker.rs`)** — `deleted_calls`, `copied_calls`,
   `moved_calls`, `expunged_sets`, `plain_expunge_calls`, `created_mailboxes`,
   `fail_deleted/fail_expunge/fail_copy/fail_move/fail_create`,
   `canned_capabilities` default `["IMAP4rev1","UIDPLUS","MOVE"]`, and `select_calls`
   recording (for the Plan 10-02 lease-hold test).
4. **Unit tests** — `deleted_store_arg` spelling/purity; `store_deleted` UID-addressing
   record test; capability matrix (MOVE+UIDPLUS/neither/mixed → `UidMove`/`FallbackCopy`,
   `UidExpunge`/`UnmarkDance`); chunking (450 UIDs → 3 chunks ≤200, `"1,2,3"` shape);
   mock arm record-and-fail coverage.

## Verify results

- `cargo test -p sge --lib imap::` — 53 passed, 0 failed.
- `cargo test -p sge --lib sync::worker::` — 25 passed, 0 failed.
- `cargo test -p sge --lib` (full) — **116 passed, 0 failed, 1 ignored** (Phases 6–9 green).
- `cargo clippy --all-targets` — 11 warnings, all pre-existing (verified identical count
  + locations on stashed baseline; zero new warnings from this plan).

## Files modified

- `src-tauri/src/imap/mod.rs` (trait + helpers + impls + tests)
- `src-tauri/src/sync/worker.rs` (MockSession arms + tests)
- `src-tauri/src/imap/bodies.rs`, `src-tauri/src/bin/sync_demo.rs` — **incidental
  compile fallout only**: stub no-op arms for the 7 new trait methods (body tests and
  demo harness never exercise delete/move verbs). No behavior change.

## Design notes for Plan 10-02

- `ExpungePath::Refuse` is never returned by `choose_expunge_path` (no-UIDPLUS →
  `UnmarkDance`); it is the runtime loud-failure upgrade when the dance cannot be
  verified on the live session.
- `expunge()` (bare) is exposed on the trait but has no caller; `uid_expunge` is the
  documented default. No STATE.md / ROADMAP.md changes (per instructions).

## Blockers

None. No task failed; no scope changes improvised.
