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
| 42-01-01 | 01 | 1 | THR-01 | T-42-01 (server-state write) | No fetch path may set `\Seen`; every `HEADER.FIELDS` fetch is `.PEEK` | unit + tripwire | `cargo test -p sge peek_audit` | ❌ W0 | ⬜ pending |
| 42-01-02 | 01 | 1 | THR-01 | T-42-01 | References lane is a separate named constant, never an extension of `FETCH_ATTRS` | grep gate | `git grep -n 'BODY\[HEADER.FIELDS' src-tauri/src` ⇒ every hit `.PEEK`-prefixed | ❌ W0 | ⬜ pending |
| 42-02-01 | 02 | 1 | THR-02 | — | M26 adds columns only (`ALTER … ADD COLUMN`); `SCHEMA_VERSION` 25→26 in the same commit | unit | `cargo test -p sge m26` (fresh + upgrade pair) | ❌ W0 | ⬜ pending |
| 42-02-02 | 02 | 1 | THR-02 | — | Stale-version assertion cannot survive the bump | unit | `cargo test -p sge schema_version` | ❌ W0 | ⬜ pending |
| 42-03-01 | 03 | 2 | THR-02 | T-42-02 (parser injection) | Msg-id tokens never routed through `decode_header_words` | fixture suite | `cargo test -p sge references_` | ❌ W0 | ⬜ pending |
| 42-04-01 | 04 | 2 | THR-03 | — | Per-folder dirty-set epoch; idempotent; `(date_utc, uid)` tiebreak | unit | `cargo test -p sge recompute_` | ❌ W0 | ⬜ pending |
| 42-04-02 | 04 | 2 | THR-03 | — | Sent reply joins its folder thread via the send-file hook | unit | `cargo test -p sge send_file_join_sent_thread` | ❌ W0 | ⬜ pending |
| 42-05-01 | 05 | 3 | THR-01, THR-04 | T-42-03 (payload desync) | `list_messages` name/signature frozen; fields additive only | unit | `cargo test -p sge thread_pair` + `cargo test -p sge list_messages` | ❌ W0 | ⬜ pending |
| 42-05-02 | 05 | 3 | THR-04 | T-42-03 | `get_thread` header-only members; serde round-trip proven both directions | unit | `cargo test -p sge get_thread` | ❌ W0 | ⬜ pending |
| 42-05-03 | 05 | 3 | THR-04 | — | TS mirror matches Rust payload (no silent key drop) | type gate | `npm run lint && npm run format:check && tsc` | ❌ W0 | ⬜ pending |
| 42-06-01 | 06 | 3 | THR-01 | — | Backfill resumable/idempotent per folder; yields between ranges | unit (mock session) | `cargo test -p sge backfill_` | ❌ W0 | ⬜ pending |
| 42-06-02 | 06 | 3 | THR-01..04 | — | Live legs NOT claimed; offline-green exit state | capture script | `scripts/live-42.sh` (deferred — no creds) | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `src-tauri/src/imap/references.rs` — References byte parser + fixture suite (folded continuation, `<...>` framed multi-link, no-bracket/case variant, malformed/empty, duplicate ids, chain-cap overflow) — NEW file
- [ ] `src-tauri/src/store/mod.rs` — M26 fresh-install + 25→26 upgrade fixture pair, extended stale-version test — new tests in existing test block
- [ ] `src-tauri/src/store/threads.rs` — epoch recompute, idempotency, `(date_utc, uid)` tiebreak, zero-row early-exit, send-file join tests — NEW file
- [ ] `src-tauri/src/store/queries.rs` — byte-compat sentinel (existing rows unchanged except additive fields) + member-predicate tests
- [ ] `get_thread` serde round-trip tests (Rust side — no TS runner exists; `vitest` NOT introduced, zero-new-deps rule)
- [ ] `peek_audit` path-5 + `FETCH_REFS_ATTRS` guard extensions — MODIFY existing tests in the **same commit** as the lane
- [ ] `worker.rs` References-lane mock mirror beside `fetch_envelopes` (`worker.rs:1894`) + backfill resumability test

*If none: "Existing infrastructure covers all phase requirements." — NOT applicable here: Wave 0 has real gaps.*

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
