---
phase: 42-threading-backend-lane
plan: 04
subsystem: store/* (backend data lane)
tags: [threading, thr-02, jwz, mail-threading, epoch, d-03, d-04]
requires: [42-01 (M26 columns in_reply_to/references_json/thread_id), 42-03 (wire lane populates references_json), D-03/d-04 decisions]
provides: [store/threads.rs recompute_threads epoch, thread_key (mailbox_id, thread_id) helper, epoch fixtures x4, mail-threading 0.1.3 in lock]
affects: [42-05 (wires epoch hooks at sync-end/backfill/send-file), 42-06 (get_thread consumes thread_id), THR-02]
tech_stack:
  added: ["mail-threading = \"0.1\" (resolves 0.1.3; serde feature OFF; dev tree = chrono-only marginal)"]
  patterns: ["one-tx batched UPDATE (rules_io.rs:438 shape)", "scoped SELECT with exact list_messages predicate fragment", "dirty-set epoch (never per-insert)"]
key_files:
  created: ["src-tauri/src/store/threads.rs"]
  modified: ["src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/src/store/mod.rs"]
decisions:
  - "mail-threading 0.1.3 pinned caret-free; serde OFF — Thread output mapped manually (u64<->String caller-id map)"
  - "persisted member order is ours: (date_utc, uid) DESC re-sort before any write; crate order never trusted"
  - "no-chain single-member threads keep NULL thread_id (own-thread sentinel, D-04); UPDATEs diff against stored value so reruns write zero rows"
metrics:
  duration: ~40
  completed: 2026-10-20
status: complete
actuals:
  tokens: 9200
  tasks: 1
  commits: 1
plan_head_before: 56c4326
estimate_ref:
  tokens: 52000
---

# Phase 42 Plan 04: Per-Folder Threading Epoch (mail-threading wrapper) Summary

**One-liner:** `store/threads.rs::recompute_threads` — per-folder JWZ epoch via `mail-threading` 0.1.3 (`subject_merge: false`) with folder-scoped SELECT, `(date_utc, uid)` member re-sort, `thread_key = (mailbox_id, thread_id)` identity, and idempotent one-transaction batched `thread_id` UPDATEs.

## Checkpoint (Task 1) — CLEARED, records verified

The `checkpoint:human-verify` for the mail-threading crate pin was **already cleared in-session
2026-10-20** (user answer: "Approve mail-threading 0.1 pin"); the resolution is recorded in
42-04-PLAN.md `<checkpoint-resolution at="2026-10-20" decision="approved">` (committed). This run
verified the element is present and marked the task done — no re-ask, no plan-file edit needed.
The in-house ~250-LOC JWZ fallback stays **GATE-ONLY** (documented in threads.rs module docs and
the Cargo.toml pin comment; never engaged — no behavioral gate failure occurred).

## Task 2 — store/threads.rs epoch (commit 810a3b6)

What landed:

- **Cargo pin** (`src-tauri/Cargo.toml`): `mail-threading = "0.1"` → 0.1.3 verified in Cargo.lock;
  serde feature OFF; pin comment restates the legitimacy evidence.
- **`recompute_threads(conn, mailbox_id, newly_seen_uids)`**: empty dirty set → early exit
  (zero rows, zero transactions); scoped SELECT reusing the exact `list_messages` predicate
  fragment (`WHERE mailbox_id = ? AND pending_delete = 0`); u64 `messages.id` ↔ String
  caller-id mapping; `references_json` decode with the pre-declared shallow-chain fallback
  (References = [In-Reply-To]); `ThreadingOptions { subject_merge: false, ..default }`; members
  re-sorted `(date_utc DESC, uid DESC)` (never the crate's unspecified order); diff against
  stored `thread_id` → idempotent zero-write reruns; changes written in ONE rusqlite transaction.
- **`thread_key(mailbox_id, thread_id)`** — D-04 grouping identity exported; bare `thread_id`
  is never a key outside folder scope.
- **`pub mod threads;`** wired in `store/mod.rs` after `pub mod queries;`.

## Verification

Per the plan gates (`cd src-tauri`):

- `cargo fmt --check` → clean at the commit.
- `cargo test -p sge recompute_` → `recompute_epoch_idempotent`, `recompute_early_exit_zero_rows` green.
- `cargo test -p sge thread_key` → `thread_key_folder_scoping` green.
- `cargo test -p sge tiebreak` → `tiebreak_date_uid` green.
- Full `cargo test -p sge` → **547 passed (+4 new), 0 failed, 2 credential-gated ignored** (live legs remain deferred-with-reason per project convention — no live claims made).

Fixture semantics proven:

| Fixture | Truth it locks (must-haves) |
|---|---|
| `recompute_epoch_idempotent` | rerun on unchanged inputs is byte-stable (2nd pass `rows_written == 0`) |
| `recompute_early_exit_zero_rows` | empty dirty set + unchanged folder touch zero rows; no-chain row stays NULL sentinel |
| `tiebreak_date_uid` | equal date → uid DESC; equal uid → date DESC; same key pair as `list_messages` |
| `thread_key_folder_scoping` | duplicate root Message-ID seeded in two folders → two groups; no cross-folder merge (closes the RESEARCH duplicate-ID knowledge gap) |

## Deviations from Plan

None — plan executed exactly as written. (Design note kept inside plan bounds: the crate
returns flat `Vec<Thread>` membership, so per-thread plans carry both member order and the
diff-based UPDATE list; no architectural change.)

## Known Stubs

None. No placeholder code, TODO/FIXME, or unwired data sources introduced. (`files_modified`
untracked bystanders found in the tree — `.planning/milestone.lock`, `github-release-notes.md` —
predate this plan and were left untouched.)

## Threat Model Status

- T-42-02 (chain-graph aliasing): mitigated — scoped SELECT re-validates folder boundary; dup-ID
  cross-folder behavior closed by `thread_key_folder_scoping` fixture.
- T-42-04 (WAL churn under single-writer mutex): mitigated — dirty-set guard + zero-row early exit +
  ONE transaction per epoch; hook sites in 42-05 run once per pass, never per-insert.

## Threat Flags

None — no new network endpoints, auth paths, or trust-boundary surface introduced by this plan.

## Commits

| Task | Commit | Files |
|---|---|---|
| Task 1 (checkpoint, pre-resolved) | — | resolution already inside 42-04-PLAN.md |
| Task 2 (epoch + pin) | 810a3b6 | src-tauri/Cargo.toml, Cargo.lock, src/store/mod.rs, src/store/threads.rs |

## Self-Check: PASSED

- FOUND: src-tauri/src/store/threads.rs, src/store/mod.rs wiring, Cargo.toml pin (0.1.3 in lock).
- FOUND: commit 810a3b6 (measured `commits: 1` from ledger base 56c4326).
- FOUND: `<checkpoint-resolution ... decision="approved">` element in the committed plan file (2 occurrences incl. close tag) — Task 1 done state verified.
- Deferred to 42-05: epoch hook call sites (`sync/worker.rs` / backfill / send-file) — intentionally untouched per lane boundary.



