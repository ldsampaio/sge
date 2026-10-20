---
phase: "42"
slug: "threading-backend-lane"
# status lifecycle: draft (seeded by plan-phase) → validated (set by validate-phase §6)
# audit-milestone §5.5 distinguishes NOT-VALIDATED (draft) from PARTIAL (validated + nyquist_compliant: false) (#2117)
status: draft
nyquist_compliant: false
wave_0_complete: false
created: "2026-10-20"
---

# Phase 42 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust inline `#[test]` — 579 fns, package `sge` (counted 2026-10-20); NO TS test runner exists (`package.json:8`) — TS side gated by `tsc` + `eslint` + `prettier` only |
| **Config file** | none needed (cargo built-in); TS: `package.json` scripts + `eslint` config |
| **Quick run command** | `cargo fmt --check && cargo test -p sge <filter>` (run from `src-tauri/`) |
| **Full suite command** | `cargo test` (all ~579+ inline tests, offline) |
| **Estimated runtime** | ~90 s full suite; ~5–20 s per filter |

---

## Sampling Rate

- **After every task commit:** Run `cargo fmt --check && cargo test -p sge <task filter>`
- **After every plan wave:** Run `cargo fmt --check && cargo test` (full suite) + the `git grep` tripwires in the Per-Task map
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 90 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 42-01-01 | 01 | 1 | THR-01, THR-04 | T-42-02 | M26 adds columns via ALTER only + one CREATE INDEX; SCHEMA_VERSION 25→26 in the same commit | unit pair | `cd src-tauri && cargo fmt --check && cargo test -p sge m26 && cargo test -p sge schema_version` | ❌ W0 | ⬜ pending |
| 42-01-02 | 01 | 1 | THR-01 | T-42-02 | UPSERT_MESSAGE_SQL carries + round-trips in_reply_to/references_json (13 params) | unit | `cd src-tauri && cargo fmt --check && cargo test -p sge upsert && cargo test -p sge m26` | ❌ W0 | ⬜ pending |
| 42-02-01 | 02 | 1 | THR-01 | T-42-02 | Msg-id tokens verbatim (never `decode_header_words`), unfolded first, deduped, capped 100/32 KiB | fixture suite | `cd src-tauri && cargo fmt --check && cargo test -p sge references_` | ❌ W0 | ⬜ pending |
| 42-02-02 | 02 | 1 | THR-01 | — | `pub mod references;` registered, imap layer builds | build + fixture | `cd src-tauri && cargo fmt --check && cargo test -p sge references_folded_two_line && cargo build -p sge` | ❌ W0 | ⬜ pending |
| 42-03-01 | 03 | 2 | THR-01, THR-04 | — | `in_reply_to` copied on BOTH parse paths, identical shape, no RFC 2047 pass on msg-id bytes | unit | `cd src-tauri && cargo fmt --check && cargo test -p sge in_reply_to && cargo test -p sge parse_fetch` | ❌ W0 | ⬜ pending |
| 42-03-02 | 03 | 2 | THR-01, THR-04 | T-42-01 | PEEK-only REFERENCES lane in separate `FETCH_REFS_ATTRS`; `peek_audit` path 5 same commit | unit + tripwire | `cd src-tauri && cargo fmt --check && cargo test -p sge peek_audit && cargo test -p sge fetch_attrs_are_parenthesized && cargo test -p sge fetch_refs_batch` | ❌ W0 | ⬜ pending |
| 42-04-00 | 04 | 2 | THR-02 | — | Checkpoint: user confirms mail-threading crate identity before Cargo.toml pin (blocking-human) | human gate | n/a — blocking-human gate; resume only on explicit confirm | — | ⬜ pending |
| 42-04-01 | 04 | 2 | THR-02, THR-04 | T-42-02, T-42-04 | Folder-scoped epoch: idempotent, (date_utc, uid) tiebreak, zero-row early exit, thread_key scoping | unit | `cd src-tauri && cargo fmt --check && cargo test -p sge recompute_ && cargo test -p sge thread_key && cargo test -p sge tiebreak` | ❌ W0 | ⬜ pending |
| 42-05-01 | 05 | 3 | THR-01, THR-04 | T-42-01, T-42-04 | Backfill resumable/idempotent/yield-disciplined per folder (SQL via store/threads.rs helpers) | unit (mock session) | `cd src-tauri && cargo fmt --check && cargo test -p sge backfill_` | ❌ W0 | ⬜ pending |
| 42-05-02 | 05 | 3 | THR-01, THR-02, THR-04 | T-42-04 | Epoch at sync-end/backfill-complete/three APPEND-success arms; sent reply joins its thread | unit (mock session) | `cd src-tauri && cargo fmt --check && cargo test -p sge send_file_join && cargo test -p sge recompute_early_exit && cargo test -p sge backfill_` | ❌ W0 | ⬜ pending |
| 42-06-01 | 06 | 3 | THR-03 | T-42-03 | `list_messages` frozen; additive fields; TS mirror same commit | unit + type gate | `cd src-tauri && cargo fmt --check && cargo test -p sge message_row && cargo test -p sge list_messages && cd .. && node_modules/.bin/tsc -p . && npm run lint && npm run format:check` | ❌ W0 | ⬜ pending |
| 42-06-02 | 06 | 3 | THR-03 | T-42-03 | `get_thread` header-only members; serde round-trip both directions; registered in generate_handler! | unit + type gate | `cd src-tauri && cargo fmt --check && cargo test -p sge get_thread && cargo test -p sge message_row && cd .. && node_modules/.bin/tsc -p .` | ❌ W0 | ⬜ pending |
| 42-07-01 | 07 | 4 | THR-01..04 | T-42-01 | live-42.sh EXIT 0/1/2 convention assertable: offline (no creds) MUST print exit=2 | script preflight | `cd src-tauri && cargo fmt --check && cd .. && bash -n scripts/live-42.sh; parse=$?; scripts/live-42.sh; code=$?; echo "live-42 parse=$parse exit=$code"` | ❌ W0 | ⬜ pending |
| 42-07-02 | 07 | 4 | THR-01..04 | — | Full offline suite green + tripwires + honest exit-state record (no live claims) | full sweep | `cd src-tauri && cargo fmt --check && cargo test -p sge` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

Re-derived from the plans' fixture names (each plan creates its own tests; Wave 0 scaffolds are the new/renamed test homes the phase's `<automated>` filters run against):

- [ ] `src-tauri/src/store/mod.rs` — M26 fresh/upgrade fixture pair `m26_fresh_install_adds_thread_columns`, `m26_upgrade_from_v25_preserves_rows` + renamed `schema_version_is_26_with_m26_threading` (42-01 Task 1)
- [ ] `src-tauri/src/imap/references.rs` — parser + 7-fixture suite `folded_two_line`, `framed_multi`, `bare_and_case`, `malformed_or_empty`, `duplicate_ids`, `chain_cap_overflow`, `encoded_shaped_ids` + `decode_header_words` contrast probe (42-02 Task 1)
- [ ] `src-tauri/src/imap/headers.rs` — `peek_audit` path-5 + extended `fetch_attrs_are_parenthesized` + shape-equality test for `in_reply_to` (42-03 — MODIFY existing tests, same commit as the lane)
- [ ] `src-tauri/src/store/threads.rs` — epoch fixtures `recompute_epoch_idempotent`, `recompute_early_exit_zero_rows`, `tiebreak_date_uid`, `thread_key_folder_scoping` (42-04 Task 1)
- [ ] `src-tauri/src/sync/worker.rs` — `MockSession.fetch_refs_batch` mirror beside `fetch_envelopes` (:1894) + `backfill_once_per_folder`, `backfill_resume_partial`, `backfill_done_folder_skipped`, `backfill_yields_between_ranges`, `backfill_strike_on_empty_stream`, `send_file_join_sent_thread` (42-05)
- [ ] `src-tauri/src/store/queries.rs` + `src-tauri/src/commands/sync.rs` — `message_row_byte_compat`, `message_row_thread_fields`, `get_thread` serde round-trip pair both directions (Rust side — no TS runner; `vitest` NOT introduced) (42-06)
- [ ] `scripts/live-42.sh` — preflight + assertable EXIT 0/1/2 convention (offline path exits 2; deferred live legs close later with creds) (42-07 Task 1)

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| As-received `HEADER.FIELDS (REFERENCES)` bytes from the real server (fold/case quirks on Zimbra) | THR-02 | No credentials in this environment (keyring-only, `PROJECT.md`); live leg deferred-with-reason per D-05 + `scripts/live-*` precedent | Run `scripts/live-42.sh` section A once creds exist; append captured bytes verbatim to the fixture corpus; re-run `cargo test -p sge references_` |
| Server refuses the parenthesized `HEADER.FIELDS (REFERENCES)` section (Zimbra BAD response) | THR-01 | Same credential gap; fallback behavior must be pre-declared, not discovered live | Run `scripts/live-42.sh` section B; confirm `References = In-Reply-To` shallow-chain fallback engages (`src/types.ts:40-:42` convention) |
| UI renders identically before/after the lane | THR-01 | No frontend test runner; the invariant is "unchanged payload consumption" | Manual: launch the app against a local cache and compare the flat list render against pre-phase screenshots; no code change in `src/components/**` is permitted this phase |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 90s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
