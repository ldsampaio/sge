---
phase: 24-auto-archive
plan: 01
subsystem: archiving
tags: [imap, sqlite, tauri, batch-journal, uidvalidity, move-only]

# Dependency graph
requires:
  - phase: 20-batch-reorganization
    provides: batch_runs journal/resume/undo machinery (journal_intent/journal_moved/journaled_ids/undo_run, chunk-25 loop, UIDVALIDITY re-check)
  - phase: 18-confirm-trust
    provides: ensure_auto_tree CREATE path + confirm-then-nest collision handling + move_one_uid/imap_outbox replay verbs
  - phase: 16-taxonomy-store
    provides: classify_settings key-value table + redact sanitizer + date_utc column
provides:
  - archive_run/get_archive_settings/set_archive_settings Tauri commands
  - classify::archive module (settings, age selector, Auto/Archive/YYYY/Top destinations, tree-ensure, MOVE runner)
  - BatchReport.totals_json (kind tag for report labeling)
affects: [24-auto-archive-plan-02, 25-smart-rules, 26-live-validation]

# Actuals (#2632)
actuals:
  tokens: 14300
  tasks: 3
  commits: 3

# Tech tracking
tech-stack:
  added: []
  patterns: [verbatim batch-machinery reuse with kind tag, structural never-delete test via include_str, display-vs-wire scope matching for mutf7 tops]

key-files:
  created:
    - src-tauri/src/classify/archive.rs
  modified:
    - src-tauri/src/commands/classify.rs
    - src-tauri/src/classify/batch.rs
    - src-tauri/src/store/queries.rs

key-decisions:
  - "Archive destinations resolve folder truth first (Auto/<Top> segment), label top name second, source leaf last — category IDs never enter paths"
  - "undo_run reused verbatim including its {} totals overwrite; post-undo archive labeling reads per-category label id, not totals kind"
  - "Empty include scope is a structural no-op, never a whole-account op"
  - "BatchReport.totals_json added (additive field) so 24-02 parses kind client-side with zero backend change"

patterns-established:
  - "Structural invariant tests: include_str!(file!()) split at #[cfg(test)] asserting no removal verb in production code"
  - "Undatable mail is never archived (conservative skip, stays put)"

requirements-completed: [ARCH-01, ARCH-02]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Archive rule persisted in SQLite (90d default, 7-730 backend validation, INBOX + Auto-tops default scope)"
    requirement: "ARCH-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/classify/archive.rs#settings_round_trip_with_backend_validation"
        status: pass
    human_judgment: false
  - id: D2
    description: "Age-based selector moves only old mail from included scope; A Classificar/Sent/Drafts/Archive-subtree structurally excluded; undatable mail stays"
    requirement: "ARCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/classify/archive.rs#selector_moves_old_only_from_included_scope"
        status: pass
      - kind: unit
        ref: "src-tauri/src/classify/archive.rs#selector_never_sources_excluded_folders"
        status: pass
    human_judgment: false
  - id: D3
    description: "One archive MOVE end-to-end (old mail to Auto/Archive/<YYYY>/<Top>, young stays) with journal intent/move ordering over a fake IMAP session"
    requirement: "ARCH-01"
    verification:
      - kind: integration
        ref: "src-tauri/src/classify/archive.rs#tracer_old_mail_archives_young_stays"
        status: pass
    human_judgment: false
  - id: D4
    description: "archive_run journals per message with kind=archive tag, resumes via journaled_ids, shared undo_batch restores originals"
    requirement: "ARCH-01"
    verification:
      - kind: integration
        ref: "src-tauri/src/classify/archive.rs#shared_undo_reverses_archive_runs_and_kind_tag_labels"
        status: pass
      - kind: unit
        ref: "cargo test -p sge batch (5 passed)"
        status: pass
    human_judgment: false
  - id: D5
    description: "MOVE-only is structural (no removal verb in production code), destinations taxonomy-independent, details redacted, chunk-25 shared"
    requirement: "ARCH-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/classify/archive.rs#archive_path_imports_no_removal_verb"
        status: pass
      - kind: unit
        ref: "src-tauri/src/classify/archive.rs#destination_shape_year_top_taxonomy_independent"
        status: pass
      - kind: unit
        ref: "src-tauri/src/classify/archive.rs#archive_detail_is_redacted"
        status: pass
      - kind: unit
        ref: "src-tauri/src/classify/batch.rs#batch_chunk_stays_25_shared_with_archive"
        status: pass
    human_judgment: false
  - id: D6
    description: "archive_run against a live throttling server (chunk/epoch/cancel behavior under real IMAP)"
    requirement: "ARCH-01"
    verification: []
    human_judgment: true
    rationale: "Offline suite uses a fake session; live-server behavior (throttling, UIDVALIDITY shifts, MOVE capability fallback) needs the Phase 26 sandboxed live bundle with human credentials"

# Metrics
duration: 55min
completed: 2026-10-13
status: complete
---

# Phase 24 Plan 01: Archive Backend Tracer Summary

**Age-based archive backend on verbatim Phase-20 batch machinery: SQLite-persisted 90d rule, date_utc-only selector, Auto/Archive/YYYY/Top MOVE-only runner with kind-tagged journal, and test-asserted never-delete**

## Performance

- **Duration:** ~55 min
- **Started:** 2026-10-13T14:30Z (approx)
- **Completed:** 2026-10-13T15:25Z
- **Tasks:** 3
- **Files modified:** 7 (1 created, 6 touched incl. 3 one-line wiring touches)

## Accomplishments

- Archive rule persisted in SQLite via `classify_settings` keys (no new tables): 90d default, backend-enforced 7–730 with plain-language errors, include scope defaulting to INBOX + all Auto tops
- `collect_archive_candidates` selects solely on the local `date_utc` column (no FETCH, no body download), structurally excluding A Classificar/Sent/Drafts/the archive subtree/bare Auto plus the shared exclusion table; undatable mail conservatively stays put
- `archive_run` command mirrors `batch_classify` line-for-line (cancel reset, subtree pre-create, epoch capture, chunk-25 loop, per-chunk UIDVALIDITY re-check, journal-before-move, resume skip) finishing with `{"kind":"archive","threshold_days":N}` totals; shared `undo_batch` reverses archive runs verbatim
- Full suite green: 472 passed, 0 failed (10 new archive/batch tests)

## Task Commits

Each task was committed atomically:

1. **Task 1: Tracer: settings + age selector + one archive MOVE end-to-end** - `24b2573` (feat)
2. **Task 2: Archive run command on verbatim batch machinery with kind tag** - `ab91d89` (feat)
3. **Task 3: Structural never-delete + redaction + live-gate sizing tests** - `246d977` (feat)

## Files Created/Modified

- `src-tauri/src/classify/archive.rs` - Archive engine: settings resolution, age/date helpers, candidate selector, destination resolver, tree-ensure, MOVE runner, 9 tests incl. fake-session tracer e2e
- `src-tauri/src/commands/classify.rs` - `archive_run` + `get/set_archive_settings` commands; `batch_report` now returns `totals_json`
- `src-tauri/src/classify/batch.rs` - `BatchReport.totals_json` field; BATCH_CHUNK=25 pin test
- `src-tauri/src/store/queries.rs` - `archive_threshold_days` / `archive_included_folders` key-value helpers with validation
- `src-tauri/src/classify/mod.rs` - `pub mod archive;` (1 line)
- `src-tauri/src/lib.rs` - 3 command registrations (3 lines)
- `src-tauri/src/imap/manager.rs` - `for_test_session` widened to `pub(crate)` (1 word)

## Decisions Made

- **Destination resolution order:** folder truth (Auto/\<Top\> segment, mutf7-decoded) → label top display name → source leaf fallback. Filed mail survives taxonomy renames (folder truth wins over possibly-stale labels); category IDs never enter paths (taxonomy-version-independent by construction).
- **undo verbatim, kind labeling two-sourced:** `undo_run`'s `{}` totals overwrite is shared Phase-20 behavior — left untouched. UI labels archive runs via `totals_json` kind pre-undo and via per-category `label_id="archive"` post-undo (both asserted in tests).
- **Empty scope = no-op:** an empty include list returns zero candidates rather than risking a silent whole-account op; `set_archive_settings` rejects empty scopes backend-side.
- **Display-vs-wire scope matching:** default scope uses display tops (`Auto/Acadêmico`) while disk holds mutf7 wire names — the selector matches either form so non-ASCII tops are archivable.
- **Empty-delimiter normalization:** pre-first-sync stores carry `""` delimiters; tree-ensure normalizes to `/` (matches the batch runner's inline assumption) instead of gluing parent+leaf.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Module declaration + command registration + test-session visibility**
- **Found during:** Task 1 (new module would not compile/register without wiring)
- **Issue:** Plan's `files_modified` omits the three one-line wiring points a new Tauri command requires: `pub mod archive;` in `classify/mod.rs`, 3 invoke-handler entries in `lib.rs`, and `pub(crate)` on the test-only `SessionManager::for_test_session` (private to `imap::manager`, needed by the cross-module tracer test)
- **Fix:** Minimal touches only (1 line + 3 lines + 1 word), no behavior change
- **Files modified:** `src-tauri/src/classify/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/imap/manager.rs`
- **Verification:** Full suite green; commands invocable via IPC
- **Committed in:** `24b2573` (mod/manager), `ab91d89` (lib.rs)

**2. [Rule 1 - Bug] Empty-delimiter parent+leaf gluing in tree-ensure**
- **Found during:** Task 1 (tracer e2e produced `AutoArchive` instead of `Auto/Archive`)
- **Issue:** `prepare_create_wire` concatenates `parent + delimiter + leaf`; pre-first-sync stores carry `""` delimiters, gluing levels together
- **Fix:** Normalize empty delimiter to `/` in `ensure_archive_tree` with rationale comment
- **Files modified:** `src-tauri/src/classify/archive.rs`
- **Verification:** Tracer e2e asserts exact `Auto/Archive` CREATE; passes
- **Committed in:** `24b2573`

**3. [Rule 2 - Missing Critical] BatchReport dropped totals_json, hiding the kind tag from readers**
- **Found during:** Task 2 (24-02 must parse kind client-side with no backend change)
- **Issue:** `batch_report` read `totals_json` then discarded it (`let _ = totals`), making the plan's "parse kind from existing payloads" impossible
- **Fix:** Additive `totals_json: String` field on `BatchReport` (existing readers ignore it; frontend TS cast is unaffected)
- **Files modified:** `src-tauri/src/classify/batch.rs`, `src-tauri/src/commands/classify.rs`
- **Verification:** `cargo test batch` green; kind round-trip asserted in `shared_undo_reverses_archive_runs_and_kind_tag_labels`
- **Committed in:** `ab91d89`

---

**Total deviations:** 3 auto-fixed (1 blocking-wiring, 1 bug, 1 missing-critical)
**Impact on plan:** All required for the plan's own must-haves (compilable/registerable commands, correct tree CREATE, UI-parseable kind tag). No scope creep — no new tables, no new IMAP verbs, no scheduler.

## Issues Encountered

- Concurrent Phase 26-01 executor active in the repo (untracked `live_gate_26*` files + modified `src-tauri/Cargo.toml` appeared mid-run). Left untouched; all commits staged explicit file lists only. No interference (cargo locking held; suite green).
- `undo_run` overwrites `totals_json` with `{}` — initially looked like a report-survival violation, but it is verbatim Phase-20 behavior and `batch_report` still returns per-category + undone counts. Kept verbatim per CONTEXT single-journal invariant; documented the two-source labeling contract instead.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- 24-02 (Archive UI) unblocked: `get/set_archive_settings`, `archive_run(resumeRun)`, `cancel_batch`, `batch_resume`-equivalent resume, `undo_batch`, `batch_report` (with `totals_json.kind`), `batch_runs_list` all registered; progress reuses the `batch-progress` event + `BatchReport` shape the existing `BatchPanel` already consumes.
- Live-server archive behavior deferred to the Phase 26 sandboxed bundle (coverage D6, human judgment required).

---
*Phase: 24-auto-archive*
*Completed: 2026-10-13*

## Self-Check: PASSED

- Files: `src-tauri/src/classify/archive.rs`, `src-tauri/src/commands/classify.rs`, `src-tauri/src/store/queries.rs`, `src-tauri/src/classify/batch.rs` all present
- Commits `24b2573`, `ab91d89`, `246d977` all ancestors of HEAD
- Claims verified: `cargo test` 472 passed / 0 failed; structural grep clean outside tests; `totals_json` kind round-trip asserted
