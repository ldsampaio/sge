---
phase: 12-drafts
plan: 12-01
subsystem: email-drafts
tags: [sqlite, rusqlite-migration, rfc5322, mail-parser, async-imap, append, uid-search]

# Dependency graph
requires:
  - phase: 11-folder-crud
    provides: Role::Drafts resolution, M8 migration + preserve-rows pattern, create/rename/delete manager verbs
provides:
  - M9 `drafts` table + draft queries (upsert/get/delete/list-dirty/mark-clean)
  - Pure RFC 5322 draft renderer with Message-ID helper (mail-parser round-trip proven)
  - `append_message` + `uid_search_header` SyncSession verbs, `DRAFT_FLAGS`, FakeSession/MockSession arms
affects: [12-02 (save/expunge-old sequence), 12-03 (draft commands + UI), 13-send, 14-mime]

# Actuals (#2632) — session diff only (tasks 1-3 implemented in prior wip 7cc7209, verified + fixed here)
actuals:
  tokens: 1697
  tasks: 4
  commits: 2

# Tech tracking
tech-stack:
  added: []
  patterns: [forward-only-migration-with-preserve-rows-test, pure-codec-with-roundtrip-proof, fake-session-recorded-calls, lease-scoped-status-reads]

key-files:
  created:
    - src-tauri/src/drafts.rs
  modified:
    - src-tauri/src/store/mod.rs
    - src-tauri/src/store/queries.rs
    - src-tauri/src/imap/mod.rs
    - src-tauri/src/imap/manager.rs
    - src-tauri/src/lib.rs

key-decisions:
  - "Zero-hit post-APPEND reconcile surfaces Refused (never retried): a blind re-APPEND could duplicate the server copy"
  - "rename_mailbox_in reads pre/post UIDVALIDITY STATUS on its own lease — STATUS never disturbs SELECT, so no INBOX bounce"
  - "System pacman rustc was ABI-broken (librustc_driver/LLVM symbol mismatch); installed user rustup stable 1.99.0 to run verification"

patterns-established:
  - "Draft reconcile rule: exactly one SEARCH hit → use it; zero → loud Refused (stay dirty); multiple → max"
  - "Probe-guard discipline in FakeSession tests: scope every MutexGuard so it drops before any re-lock on the same mutex"

requirements-completed: []

coverage:
  - id: D1
    description: "M9 drafts table exists with all 12 columns and forward migration preserves existing rows"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#m9_adds_drafts_preserving_rows"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#schema_version_is_9_with_drafts"
        status: pass
    human_judgment: false
  - id: D2
    description: "Rendered draft bytes parse with mail-parser preserving subject, recipients, and body (incl. non-ASCII, empty-To, header-injection)"
    verification:
      - kind: unit
        ref: "src-tauri/src/drafts.rs#renders_ascii_simple_round_trip + 5 sibling tests"
        status: pass
    human_judgment: false
  - id: D3
    description: "append_message sends literal bytes with (\\Draft \\Seen) flags; UID discovery goes through uid_search_header, never APPENDUID parsing"
    verification:
      - kind: unit
        ref: "src-tauri/src/imap/mod.rs#draft_flags_spelling"
        status: pass
      - kind: unit
        ref: "src-tauri/src/imap/manager.rs#save_draft_appends_new_and_expunges_old + 7 sibling tests"
        status: pass
    human_judgment: false
  - id: D4
    description: "Full backend suite green (Phases 6-11 unbroken)"
    verification:
      - kind: unit
        ref: "cargo test -p sge → 211 passed, 0 failed, 1 ignored"
        status: pass
    human_judgment: false

# Metrics
duration: ~3h (dominated by toolchain repair + hung-suite diagnosis)
completed: 2026-10-08
status: complete
---

# Phase 12 Plan 01: Drafts foundation Summary

**M9 drafts store + RFC 5322 renderer + APPEND/SEARCH-HEADER verbs, proven by 211 green backend tests**

## Performance

- **Duration:** ~3h wall (toolchain repair + deadlock diagnosis dominated; code fixes are small)
- **Started:** 2026-10-07T23:30Z (approx, continuation session)
- **Completed:** 2026-10-08
- **Tasks:** 4 / 4
- **Files modified:** 1 this session (`imap/manager.rs`); tasks 1-3 implemented in prior wip `7cc7209`

## Accomplishments

- M9 `drafts` table (12 columns, 2 indexes) migrates forward from v8 preserving mailbox/message rows; `upsert_draft`, `get_draft`, `delete_draft`, `list_dirty_drafts`, `mark_draft_clean` queries live in `store/queries.rs`
- Pure `drafts.rs` renderer (`DraftFields`, `new_message_id` → `<compose_id@sge.local>`, hand-rolled encoded-word helper, CR/LF sanitization) round-trips through mail-parser incl. `Assunto com acentuação` and a `Subject\r\nBcc:` injection attempt
- `SyncSession::append_message` + `uid_search_header` with `DRAFT_FLAGS = "(\\Draft \\Seen)"`; FakeSession/MockSession record-and-replay arms drive the exactly-one-copy sequence tests
- `cargo test -p sge` fully green: **211 passed, 0 failed, 1 ignored**

## Task Commits

Tasks 1-3 were implemented by the prior (interrupted) session as `7cc7209`; this session verified each against its acceptance criteria and fixed what blocked Task 4:

1. **Task 1: M9 drafts table + queries** - `7cc7209` (wip, prior session; verified here: `store::` green)
2. **Task 2: drafts.rs renderer** - `7cc7209` (wip, prior session; verified here: `drafts::` 6/6 green)
3. **Task 3: imap verbs + fake arms** - `7cc7209` (wip, prior session; verified here: `imap::` green after fixes)
4. **Task 4: Full backend suite** - `4c81836` (fix(12-01): three suite-green fixes)

**Plan metadata:** this SUMMARY (`12-01-SUMMARY.md`, commit follows)

## Files Created/Modified

- `src-tauri/src/drafts.rs` (created, prior session) - Pure RFC 5322 renderer + Message-ID helper + 6 round-trip/injection tests
- `src-tauri/src/store/mod.rs` (prior session) - `SCHEMA_VERSION` 8→9, `M9_DRAFTS_SQL`, preserve-rows test
- `src-tauri/src/store/queries.rs` (prior session) - `DraftRow`, draft CRUD + dirty-queue queries
- `src-tauri/src/imap/mod.rs` (prior session) - `append_message`/`uid_search_header` trait + impls, `DRAFT_FLAGS`, spelling test
- `src-tauri/src/imap/manager.rs` (prior session + this session's fixes) - `save_draft_copy_in`/`save_draft_on_session`, FakeSession arms, 3 fixes below
- `src-tauri/src/lib.rs` (prior session) - `pub mod drafts` registration

## Decisions Made

- Zero-hit reconcile is `SyncError::Refused`, not `Protocol`: the copy may have persisted invisibly, so the reconnect-retry must not blindly re-APPEND (duplicate risk). Precedent: the unverifiable unmark dance already uses `Refused` for "can't verify, don't retry". Command layer handles all variants uniformly (stays dirty), so no caller changes.
- `rename_mailbox_in` pre/post STATUS runs on its own lease (SELECTs `old` once); retry path re-leases INBOX since `old` may be gone after an ambiguous failure. `mailbox_status()` itself unchanged (external callers unaffected).
- Toolchain: system pacman `rust` 1.99.0 is ABI-broken (`librustc_driver` wants `std::string::_M_mutate@LLVM_23.1`, libLLVM only exports the wchar variant — partial-upgrade state, no sudo to repair). Installed user-space rustup stable 1.99.0 (`~/.cargo`, `--profile minimal`) purely to run verification; no project files reference it.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] FakeSession probe-guard deadlock hung the suite**
- **Found during:** Task 4 (full suite hung >60s on `save_draft_append_failure_errors_before_any_delete`)
- **Issue:** Test held `let f = probe.lock().unwrap()` across asserts, then called `probe.lock()` again to toggle `fail_append` — same-thread re-lock of a non-reentrant std Mutex deadlocked; the guard's borrow extended past its last use under this toolchain, so NLL did not release it in time.
- **Fix:** Scoped the guard in an explicit block so it drops before the re-lock (matches the scoped pattern already used elsewhere in these tests).
- **Files modified:** `src-tauri/src/imap/manager.rs` (test only)
- **Verification:** Test passes in 0.00s; full suite completes.
- **Committed in:** `4c81836`

**2. [Rule 1 - Bug] `rename_mailbox_in` bounced through INBOX (4 SELECTs, expected 2)**
- **Found during:** Task 4 (`rename_and_delete_pass_wire_names_byte_identical` FAILED: `["INBOX","Pai","INBOX","Velha"]` vs `["Pai","Velha"]`)
- **Issue:** Pre/post UIDVALIDITY checks called `self.mailbox_status()`, which leases INBOX — two spurious SELECTs around the RENAME.
- **Fix:** Both STATUS reads run on the rename lease itself (STATUS never disturbs SELECT state); retry path re-leases INBOX since `old` may be gone.
- **Files modified:** `src-tauri/src/imap/manager.rs`
- **Verification:** `imap::manager::tests::` 31/31 green, incl. retry + UIDVALIDITY-bump tests.
- **Committed in:** `4c81836`

**3. [Rule 1 - Bug] Zero-hit reconcile retried through reconnect, masking the loud error**
- **Found during:** Task 4 (`save_draft_invisible_copy_is_loud_error_without_expunge` FAILED: got reconnect error, expected "persisted copy not found")
- **Issue:** `save_draft_copy_in` retried every non-Refused error; the zero-hit `Protocol` error triggered `reconnect()` (which fails offline), hiding the real cause — and a blind re-APPEND retry risks duplicating the server copy.
- **Fix:** Zero-hit returns `SyncError::Refused` (never retried, row stays `dirty=1`); doc comments on `save_draft_on_session`/`save_draft_copy_in` updated.
- **Files modified:** `src-tauri/src/imap/manager.rs`
- **Verification:** Test green; command layer treats all variants uniformly (no caller change needed).
- **Committed in:** `4c81836`

---

**Total deviations:** 3 auto-fixed (all Rule 1 bugs blocking Task 4's green suite)
**Impact on plan:** All fixes required for correctness of the transport slice (deadlock, SELECT discipline, no-duplicate invariant). No scope creep — no new features, no API changes.

## Issues Encountered

- System Rust toolchain broken (see Decisions); installed rustup stable to `~/.cargo` — environment repair, not a project dependency. If the orchestrator's environment differs, `cargo test -p sge` from `src-tauri/` reproduces verification with any working ≥1.8x toolchain.
- Full suite initially hung indefinitely (deadlock above) and was killed twice; orphaned test binaries had to be cleaned with PID-targeted `kill` (`pkill -f` matched the agent's own shell — avoided thereafter with bracket-pattern `pgrep`).
- Threat-model items verified: T-12-01 (forward-only M9, no ALTER/DROP of pre-M9 tables — confirmed by diff; preserve-rows test green), T-12-02 (`header_injection_newlines_stripped` green — injected `Bcc:` header does not survive the round-trip).

## Known Stubs

None introduced by this plan. (Note: `sync/worker.rs` carries MockSession draft arms + `dirty_draft_count` query wiring from the wip commit, but the `replay_dirty_drafts` flush itself is Plan 12-02 scope — intentionally not built here.)

## Threat Flags

None — no new network surface (renderer output is stored/tested only; APPEND verbs are exercised through fakes, no live traffic).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- 12-01 tracer complete: store → render → APPEND → reconcile proven against fakes. Plans 12-02 (save/expunge-old sequence + reconnect flush) and 12-03 (commands + UI) can build on `save_draft_copy_in`, `DraftSaveResult`, and the dirty-queue queries.
- Known limitation for 12-02 (not fixed here, no failing test): if the *expunge-old leg* fails after a successful reconcile, the reconnect-retry re-runs the whole sequence including APPEND, which could leave a duplicate server copy (SEARCH takes max, expunge targets only the tracked old UID). 12-02 should consider resuming at the expunge leg when the new UID is already known.
- Live gate (RESEARCH §7, mail.utfpr.edu.br round-trip) remains deferred — manual, needs credentials.

---
*Phase: 12-drafts*
*Completed: 2026-10-08*
