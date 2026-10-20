---
phase: 39-llm-settings-foundation
plan: 02
subsystem: security
tags: [rust, keyring, secret-service, ammonia, prompt-rendering, prompt-injection]

# Dependency graph
requires:
  - phase: 39-llm-settings-foundation
    provides: Plan 39-01 LlmSettings type + DEFAULT_TEMPLATE + M23 store (key.rs/prompt.rs build on the lane root)
provides:
  - llm/key.rs — KEYRING_LLM_API_KEY const + save/load/presence/clear helpers (T-39-01 mitigated)
  - llm/prompt.rs — BODY_CHAR_LIMIT + strip_hidden + flatten + truncate + single-pass render_prompt (T-39-02 mitigated)
affects: [39-03-settings-commands, phase-40-generation-flow]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 7296
  tasks: 3
  commits: 3

# Tech tracking
tech-stack:
  added: []
  patterns: [quote-aware tag scanner for hidden-content removal, single-pass positional template substitution, fragment-built self-scan needles]

key-files:
  created:
    - src-tauri/src/llm/key.rs
    - src-tauri/src/llm/prompt.rs
  modified:
    - src-tauri/src/llm/mod.rs

key-decisions:
  - "Trait-injected LlmKeyStore (KeyringLlmKeyStore + MemoryLlmKeyStore) so unit tests prove the contract without Secret Service; presence helper fail-closed to false"
  - "Hand-written tag scanner instead of the research regex (regex crate has no backreferences) — nesting-counted, void-aware, unclosed-hidden drops the tail"
  - "Single-pass positional substitution so values containing placeholder syntax are never re-expanded"

patterns-established:
  - "Presence-only secret exposure: has_llm_api_key -> bool, no getter for convenience, no Debug on secret holders"
  - "Renderer purity backstop: include_str self-scan test with fragment-built needles fails the build on future network/fs imports"

requirements-completed: [LLM-02, LLM-03]

coverage:
  - id: D1
    description: "API key round-trips through separate sge-llm-api-key keyring entry; presence surfaces as boolean only"
    requirement: "LLM-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/key.rs#save_load_roundtrip_reports_presence"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/key.rs#double_save_leaves_one_working_entry"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/key.rs#clear_when_absent_succeeds"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/key.rs#error_strings_name_operation_not_secret"
        status: pass
    human_judgment: false
  - id: D2
    description: "Template renders exactly {sender} {subject} {body}; unknown {tokens} stay literal; hidden HTML removed with content; body fenced as untrusted data with truncation marker"
    requirement: "LLM-03"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#unknown_tokens_stay_literal"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#hidden_div_removed_with_content"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#display_none_case_variants_removed_with_content"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#long_body_truncates_with_marker"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#output_contains_untrusted_data_fence"
        status: pass
    human_judgment: false
  - id: D3
    description: "Equal/adjacent/repeated placeholders positional; 8000/8001 boundary exact; multibyte bodies stay valid UTF-8"
    requirement: "LLM-03"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#equal_sender_subject_land_in_own_positions"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#truncation_boundary_marks_8001_not_8000"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/prompt.rs#multibyte_body_truncates_at_char_boundary"
        status: pass
    human_judgment: false

# Metrics
duration: 35min
completed: 2026-10-15
status: complete
---

# Phase 39 Plan 02: Keyring Custody + Fenced Renderer Summary

**Separate `sge-llm-api-key` keyring entry with presence-bool helpers plus a pure strip-flatten-truncate-fence prompt renderer — 21 unit tests green (4 key + 17 prompt), zero network I/O**

## Performance

- **Duration:** 35 min
- **Started:** 2026-10-15T15:30:00Z
- **Completed:** 2026-10-15T16:05:00Z
- **Tasks:** 3
- **Files modified:** 3

## Accomplishments

- `llm/key.rs`: `KEYRING_LLM_API_KEY` (`sge-llm-api-key`, never-changes) with plain-string `set_password`, `NoEntry`-tolerant load/clear, fail-closed presence-bool; secret literals appear only at save-call bindings, errors name the operation never the value
- `llm/prompt.rs`: hidden-content scanner → empty-tag ammonia flatten → 8000-char scalar-boundary truncate with disclosure marker → single-pass three-token substitution → explicit untrusted-data fence; unknown `{tokens}` stay literal
- Edge hardening: equal/adjacent/repeated placeholders positional, placeholder-free templates unchanged, exact 8000/8001 boundary, multibyte char-boundary safety, plus an `include_str!` self-scan backstop against future network/fs imports

## Task Commits

Each task was committed atomically:

1. **Task 1: Separate keyring entry with presence-bool** - `f82d871` (feat)
2. **Task 2: Fenced prompt renderer pipeline** - `8e585ad` (feat)
3. **Task 3: Renderer edge hardening — ordering, adjacency, empty probes** - `19b75c8` (feat)

## Files Created/Modified

- `src-tauri/src/llm/key.rs` - Keyring custody: const, LlmKeyError, LlmKeyStore trait, keyring + memory stores, save/load/presence/clear + 4 tests + ignored live test
- `src-tauri/src/llm/prompt.rs` - Renderer: BODY_CHAR_LIMIT, hidden scanner, flatten, truncate, single-pass substitute, render_prompt + 17 tests
- `src-tauri/src/llm/mod.rs` - Registered `pub mod key;` + `pub mod prompt;` (2 lines)

## Decisions Made

- Trait-injected `LlmKeyStore` (`KeyringLlmKeyStore` + `MemoryLlmKeyStore`, one instance per slot): unit tests prove the round-trip/presence/idempotency contract with no Secret Service; `has_llm_api_key` fail-closed to `false` on backend errors so the UI prompts for a key instead of crashing on a locked keyring.
- Quote-aware tag scanner over the RESEARCH regex: the prescribed pattern uses a backreference, which the `regex` crate rejects at compile time. The scanner counts nested same-name openers, cuts void elements alone, and drops the tail on unclosed hidden openers (fail-closed); the fence remains the second layer.
- Single-pass left-to-right substitution (earliest-token scan, values never re-scanned): chained `.replace()` would re-expand a sender literally containing `{body}` — the plan's "positional replace, no merge" language requires one pass.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] RESEARCH Pattern 3 regex does not compile (backreference unsupported)**
- **Found during:** Task 2 (Fenced prompt renderer pipeline)
- **Issue:** The prescribed `(?is)<([a-z][a-z0-9]*)...>\1` pattern panics at `LazyLock` init: `regex parse error: backreferences are not supported`. A flat no-backref regex would mis-handle nesting (leak inner text or eat following content).
- **Fix:** Hand-written quote-aware tag scanner (`find_hidden_opener` + `find_matching_close` with depth counting, void-element list, ASCII case-insensitive matching). Same observable contract the plan specifies (hidden-div + display:none incl. case/whitespace variants removed with content).
- **Files modified:** src-tauri/src/llm/prompt.rs
- **Verification:** `hidden_div_removed_with_content`, `display_none_case_variants_removed_with_content` pass; nested-markup behavior covered by scanner design + fence layer
- **Committed in:** 8e585ad (Task 2 commit)

**2. [Rule 2 - Missing Critical] Chained `.replace()` re-expands placeholder syntax inside values**
- **Found during:** Task 2 verification (`braces_in_values_render_literally` failed: sender `{body}` was replaced by the fence)
- **Issue:** Three chained replacements re-scan inserted values, so attacker-influenced sender/subject text containing `{body}` would be expanded — violating the plan's "positional replace" requirement and weakening the injection posture.
- **Fix:** Single-pass `substitute()` scanning for the earliest of the three tokens left-to-right; inserted values never re-enter the scan.
- **Files modified:** src-tauri/src/llm/prompt.rs
- **Verification:** `braces_in_values_render_literally` passes; all Task 3 ordering tests pass on top
- **Committed in:** 8e585ad (Task 2 commit)

**3. [Rule 1 - Bug] `include_str!` self-scan test matched its own needle literals**
- **Found during:** Task 2 verification (`renderer_performs_no_network_or_file_io` failed on its own comments)
- **Issue:** The structural test's comments and assert strings literally contained `reqwest`/`std::fs`/`std::net`, so scanning its own source always failed.
- **Fix:** Needles built at runtime from fragments (`["req", "west"].concat()`), documented with a NOTE explaining why.
- **Files modified:** src-tauri/src/llm/prompt.rs
- **Verification:** Test passes; manually confirmed a planted `reqwest` token would trip it (needle construction is exact)
- **Committed in:** 8e585ad (Task 2 commit)

---

**Total deviations:** 3 auto-fixed (2 bugs, 1 missing-critical)
**Impact on plan:** All three required for correctness/security of the shipped primitives; no scope creep — no IPC, no commands, no dependency changes (zero-new-deps honored: `regex` is used nowhere new, `ammonia` was already pinned).

## Issues Encountered

None beyond the deviations above — both were caught by the plan's own acceptance tests during task verification, fixed inline, and committed within the owning task.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Plan 39-03 can build `save/clear` Tauri commands on `save_llm_api_key`/`clear_llm_api_key` with `KeyringLlmKeyStore::new()` (remember `spawn_blocking` — helpers block) and `has_llm_api_key` for the `has_api_key: bool` view field; `render_prompt` is ready for the Phase 40 chat-completions payload builder.
- Pre-existing repo state untouched: the `cargo test` (full-suite) `live_gate_21_safety.rs` compile failure from uncommitted v1.7-era `drafts.rs` work remains out of scope; verification scope `cargo test -p sge --lib llm` is fully green (38 passed, 1 ignored live-keyring).

## Self-Check: PASSED

- All created files exist: `src-tauri/src/llm/key.rs`, `src-tauri/src/llm/prompt.rs` — FOUND
- All commits exist: `f82d871`, `8e585ad`, `19b75c8` — FOUND (`git log --oneline` verified)
- No file deletions in any task commit — verified via commit stat (all insertions)
- No stub patterns (`TODO`/`FIXME`/`placeholder`/`unimplemented`) in new files — verified via grep
- `cargo test -p sge --lib llm` → 38 passed, 1 ignored; `llm::key` 4 passed; `llm::prompt` 17 passed
- No `reqwest::Client` construction or `.send()` in `src-tauri/src/llm/` code (doc-comment mentions only)
- Secret literals (`sk-test-*`) appear only bound to `saved` at save-call sites + one negative assertion

---
*Phase: 39-llm-settings-foundation*
*Completed: 2026-10-15*
