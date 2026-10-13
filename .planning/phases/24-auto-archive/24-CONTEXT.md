# Phase 24: Auto-Archive - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The user retires old mail without fear: an age-based rule moves messages
older than N days into `Auto/Archive/`, executed as an explicit journaled
batch (resume + undo) reusing the PROVEN Phase 20 `batch_classify` machinery
with an archive-specific candidate selector. No classifier call, no delete
verb anywhere — MOVE only, reviewable in the archive folder, undo restores
originals. Covers ARCH-01/02. Requires Phase 20 (batch journal/resume/undo)
+ Phase 19 (taxonomy, ID-stable). UI: yes (rule config + run progress +
review). Zero new IMAP verbs (MOVE via `execute_filing_move` /
`move_message_in` + `imap_outbox` replay, CREATE via `ensure_auto_tree`).

</domain>

<decisions>
## Implementation Decisions

### Archive rule configuration
- Default age threshold 90 days (Date header age, not arrival/sync time),
  user-tunable in options UI (number input, days; min 7, max 730,
  plain-language validation reusing Phase 16/19 style) — consistent with the
  Phase 18 threshold-slider pattern (sensible default, future-only effect).
- Per-folder override: rule applies per-folder with include list (default:
  INBOX + all `Auto/<Top>/...` filed mail); `A Classificar`, Sent, Drafts,
  and `Auto/Archive/` itself are always excluded (Sent/Drafts mirrors the
  Phase 17 SIDE-03 exclusion seed; archiving unreviewed `A Classificar` mail
  would hide it from review).
- Configuration persisted in SQLite (`archive_settings` row or settings
  key-value: threshold_days + included-folders JSON, following the
  `auto_root` setting precedent) — NOT localStorage, so it survives restart
  (mirrors Phase 23 `ui_state`/SRCH-03 persistence discipline).
- Trigger is MANUAL only (explicit "Arquivar agora" button + options-page
  rule editor): no scheduler/daemon in this phase — same trust posture as
  Phase 20 (explicit unconfirmed-by-design op with report + undo, never
  silent background moves). Scheduled/auto-run is deferred to Phase 25+
  rules territory.

### Archive destination & taxonomy interplay
- Destination layout `Auto/Archive/<YYYY>/<original-top>/` (year bucket +
  original top-level preserved, e.g. `Auto/Archive/2024/Financeiro`): keeps
  the Phase 16 ≤8-tops shape readable at scale, preserves category context
  for review, and avoids one giant flat folder. Year from the message Date
  header (same clock as the age rule).
- `Auto/Archive/` subtree is created via the existing `ensure_auto_tree`
  path (delimiter-aware, `\Noselect` respected, `auto_root` marker +
  confirm-then-nest collision handling from Phase 18 reused verbatim) —
  zero new CREATE logic; planner extends `FilePlan`/tree-ensure with an
  archive variant or a pre-create pass like `batch::ensure_full_tree`.
- Archive destinations are taxonomy-VERSION-INDEPENDENT (plain dated
  folders, no category-ID references): taxonomy version bumps / Phase 19
  renames/merges/deletes never invalidate archive paths (ID-stable
  requirement RULE-02 satisfied by construction — archive stores no label
  pointers as paths).
- Collision with user folders: if the user owns `Auto/Archive` (or any
  prefix level), the Phase 18 confirm-then-nest dialog applies unchanged
  ("usar Auto existente?"), decision recorded per account — no archive-
  specific collision UI.

### Journal / resume / undo semantics
- REUSE the `batch_runs` + per-message journal machinery verbatim (same
  `journal_intent`/`journal_moved`/`journaled_ids` rows, `running → done |
  interrupted | undone` state machine, reverse-order `undo_batch`): archive
  runs ARE batch runs with an archive-specific candidate selector (age ≥
  threshold instead of classifier suggestion) — no archive-specific journal
  table (single journal invariant, same idempotent-resume interplay with
  `imap_outbox` verified in Phase 20 tests).
- Archive runs are tagged in the run record (`totals_json` carries
  `{"kind":"archive","threshold_days":N}` or equivalent) so the report UI
  and `batch_runs_list` can label them distinctly from classify runs; a
  shared `undo_batch(run_id)` reverses either kind (to→from in reverse
  order, marks run undone, report survives undo — Phase 20 semantics).
- Progress UI reuses the Phase 20 BatchProgress surface (bar + counts +
  cancel via the cooperative `batch_cancel` flag; chunk size stays 25,
  `BATCH_CHUNK`, tuned against the throttling UTFPR server in Phase 20) —
  extended with an archive label and threshold echo, not a new component.
- Resume: crash/kill mid-run ⇒ `batch_resume(run_id)` continues after the
  last journaled row (no dupes/skips); per-chunk UIDVALIDITY re-check
  (shift ⇒ skip/abort-to-resync for that folder, run marked interrupted)
  applies unchanged — archive moves are equally UID-gated.

### Safety rails
- Never-delete is STRUCTURAL, not UI copy: the archive path uses only MOVE
  (`execute_filing_move` / `move_message_in` + `imap_outbox`); no EXPUNGE /
  STORE \Deleted verb exists on the archive path (assert in tests: archive
  command imports no delete/expunge function — mirrors the Phase 17 "zero
  new IMAP verbs" and Phase 18 "secondary labels produce zero IMAP traffic"
  assertion style). Body reads (if any, for Date fallback) are BODY.PEEK-only
  (M1 invariant).
- UIDVALIDITY epoch gating per chunk (Phase 20 pattern: epochs captured at
  run start, re-checked per chunk) + UID-only addressing; age computed from
  the local `date_utc` column (sync-populated), never from a fresh FETCH —
  archive never triggers a full sync or body download.
- `A Classificar` interaction: archive NEVER sources from `A Classificar`
  (excluded by default, no opt-in in this phase) and NEVER files INTO it —
  worst case stays `A Classificar` (Phase 18 trust-gate philosophy extends:
  unreviewed mail must stay visible, not buried in the archive).
- Threshold reuse: the Phase 18 confidence threshold is IRRELEVANT here
  (archive needs no classifier call — age is the only gate); the only
  threshold is `threshold_days`, stored separately from the confidence
  setting so tuning one never perturbs the other (mirrors Phase 18
  "changing it re-routes future suggestions only").

### the agent's Discretion
- Exact options-UI layout for the rule editor (number input + folder
  checklist vs master-detail — follow existing options/TaxonomyEditor patterns)
  and exact pt-BR copy for the archive button/report/undo affordances.
- Year-bucket granularity if Date parsing forces it (year vs year-month —
  year recommended; planner picks if Date-header edge cases demand finer buckets).
- Whether the archive candidate scan pages by folder or by global date order
  within the chunk-25 discipline.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/commands/classify.rs:912` — `batch_classify(state, app,
  scope, resume_run)` (pre-creates tree via `batch::ensure_full_tree`,
  journal-skip resume, epoch capture, spawned chunk loop); archive adds an
  age-selector variant, not a new runner.
- `src-tauri/src/classify/batch.rs` — `BATCH_CHUNK = 25`, `journal_intent` /
  `journal_moved` / `journaled_ids`, `undo_run`, `collect_candidates`;
  archive reuses all four journal APIs with a new `collect_archive_candidates`.
- `src-tauri/src/commands/classify.rs:1210` — `undo_batch(run_id)` +
  `batch_runs_list` + `batch_report` + `cancel_batch`; shared by both run kinds.
- `src-tauri/src/classify/filing.rs:128` — `ensure_auto_tree` (delimiter-aware,
  `auto_root` marker, confirm-then-nest collision, CREATE via
  `prepare_create_wire` + `create_mailbox_in`); archive subtree rides this path.
- `src-tauri/src/store/queries.rs:2001` — `create_batch_run` + run-state update;
  `get_setting`/`set_setting` (`auto_root` precedent for `threshold_days`).

### Established Patterns
- Forward-only migrations + preserve-rows test per schema change (M2–M13 template).
- BODY.PEEK-only reads; UID-only addressing with UIDVALIDITY epoch gating;
  optimistic-local + durable `imap_outbox` queue + event-notify; cooperative
  cancel (`sync_cancel` / `batch_cancel`).
- Backend-enforced gates with plain-language errors (Phases 11/16/18/19); UI
  restraint never the only guard — never-delete asserted in tests.
- Redaction sanitizer (`classify/redact.rs`, Phase 16): no secrets in DB/logs;
  labels store pointers, never content.
- `Auto` root reserved (`taxonomy.rs:23` validation); taxonomy IDs stable across
  renames, version++ on structural ops (Phase 19).

### Integration Points
- New commands: `archive_run` (or `batch_classify` with archive params —
  planner picks), `get_archive_settings`/`set_archive_settings`; `undo_batch`,
  `batch_resume`, `cancel_batch`, `batch_report`, `batch_runs_list` shared.
- `classify/batch.rs` — new `collect_archive_candidates(conn, threshold_days,
  included_folders)` alongside `collect_candidates`.
- `classify/filing.rs` — archive destination variant of tree-ensure
  (`Auto/Archive/<YYYY>/<Top>`).
- Frontend: options-page rule editor + archive run button/progress (extends
  Phase 20 BatchProgress) + archive folder review (existing folder view).

</code>

<specifics>
## Specific Ideas

- ARCH-01/02 verbatim: user configures age rule (e.g. >30 days → `Auto/Archive/`);
  batch job journaled per message with resume/undo; uses `batch_classify`
  machinery with archive-specific threshold. Never deletes — only MOVE; user
  reviews archive folder; undo-batch restores originals.
- Default threshold proposed at 90 days (safer than the 30-day example for a
  first-run whole-account op with undo as the backstop); tunable 7–730.
- No scheduler in this phase — manual trigger only; scheduled auto-run belongs
  with Phase 25 smart rules, not here.

</specifics>

<deferred>
## Deferred Ideas

- Scheduled/automatic archive runs (daemon/cron trigger — Phase 25+ rules
  territory; this phase is manual-trigger only).
- Archive dry-run + retry-failed (same deferral family as Phase 20 batch
  dry-run + retry-failed, v1.3.x).
- Per-category retention policies (different ages per top-level — natural
  follow-up once the single-threshold path is proven).
- Server-side (Sieve) archive rule export (out of scope per REQUIREMENTS.md —
  a second product).

</deferred>
