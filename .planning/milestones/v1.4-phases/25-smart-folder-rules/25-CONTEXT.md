# Phase 25: Smart Folder Rules - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The user defines deterministic filing rules (keyword / sender pattern /
subject regex) that file mail into user-selected folders. Rules are
validated backend-side before anything is stored; they apply at sync
(behind-sync, never inline) and on demand; any conflict (rule-vs-rule,
rule-vs-classifier, rule-vs-taxonomy overlap) resolves through user
confirmation reusing the Phase 18 `confirm_suggestion` path; all moves reuse
the proven Phase 10 MOVE + `imap_outbox` machinery with UID-only addressing
and UIDVALIDITY gating; rules persist in SQLite and survive taxonomy version
bumps via ID-stable references. Rules NEVER auto-delete — only MOVE.
Covers RULE-01/02/03. No new IMAP verbs; no server-side (Sieve) export.

</domain>

<decisions>
## Implementation Decisions

### Rule expressiveness & validation
- Match fields: sender substring/domain match + subject keyword + subject
  regex + body-snippet keyword (over already-cached sanitized text only —
  rules NEVER trigger a BODY fetch; BODY.PEEK-only invariant holds). AND
  within a rule (all conditions must match), OR across rules (first match
  wins by priority — see conflict area). No full-body regex, no
  attachment-content matching in this phase.
- Subject regex uses the `regex` crate, compiled at rule-create time;
  invalid regex rejected with a plain-language reason (reuses Phase 16/19
  validation style). Regex size/length capped (pattern ≤200 chars, no
  catastrophic-backtracking constructs enforced by size cap + timeout-free
  bounded input ≤1000 chars).
- Creation-time validation (backend-enforced, not UI-only): no empty
  patterns (at least one non-empty condition required); destination folder
  must exist or be creatable via Phase 11 CREATE guards; reserved names
  rejected (`Auto` root case-insensitive, `INBOX`, role folders unless
  explicitly picked, `\Noselect`); delimiter/`/` and `.` injection in
  rule names rejected (mirrors Phase 19 folder-injection guard).
- UI-only rule authoring in this phase (no import/export file format) —
  rules CRUD lives in options UI next to the Phase 19 taxonomy editor;
  export/import deferred (taxonomy JSON round-trip is the portability story
  for now; rules are small enough to recreate).

### Rule application point
- At-sync hook mirrors the Phase 17 behind-sync discipline: post-sync hook
  enqueues rule evaluation (fire-and-forget, 3–5 lines in `sync/worker.rs`);
  drain happens on a detached task under the existing `ClassifyGate`
  (single-flight, no new gate type). Sync NEVER awaits rules; rules NEVER
  block sync completion or UI paint.
- Evaluation order per message: override-pin check first (Phase 18 pinned
  overrides always win) → user rules by priority → classifier suggestion.
  A rule hit produces a rule-sourced suggestion row that flows through the
  SAME confirm path (suggest → confirm → MOVE); rules do NOT move mail
  directly in the default single-message flow.
- On-demand application: `apply_rules(scope)` command (whole account or
  per-folder) that evaluates rules over already-labeled/unlabeled mail and
  surfaces hits as confirmable suggestions — dialog ON for single/few hits,
  reusing `confirm_suggestion`; bulk mode (whole-account) follows the Phase
  20 batch pattern (journaled, progress, undo) with planner picking reuse
  depth.
- Per-folder enablement: rules can be scoped to folders (default: all
  folders); folders excluded from auto-classify (Phase 17 `SIDE-03`:
  Sent/Drafts) are ALSO excluded from automatic rule evaluation by default,
  while manual/on-demand `apply_rules` still works on them (user intent
  overrides, same semantics as manual `classify_message`).

### Conflict resolution
- Rule-vs-rule precedence: explicit integer priority (lower number = higher
  precedence, user-reorderable); ties broken by creation order (oldest
  wins); first match wins, evaluation stops. Priority is a column, edited
  by move-up/move-down in the rules UI.
- Rule-vs-classifier disagreement: rule hit NEVER silently overrides a
  classifier label — the confirm dialog shows BOTH (rule destination vs
  suggested category + confidence) and the user picks; dismissal keeps
  mail in place. Below-threshold (`A Classificar`) mail: rules MAY still
  propose a destination (rules are deterministic user intent, unlike model
  guesses) but ALWAYS behind the same confirm dialog.
- Rule-vs-taxonomy overlap (rule destination inside `Auto/` tree or a
  taxonomy-managed folder): resolved by user confirmation reusing
  `confirm_suggestion` (RULE-01 verbatim) — the dialog names the overlap
  ("esta pasta é gerenciada pelo classificador") with options Mover /
  Escolher outra pasta / Dispensar. `ensure_auto_tree` collision semantics
  (confirm-then-nest) apply unchanged when the destination is under `Auto/`.
- Never-auto-delete enforcement is structural (RULE-03 verbatim,
  backend-side): rule destinations are MOVE-only; any rule whose action
  resolves to delete/expunge is rejected at validation; tests assert no
  rule path emits EXPUNGE/`\Deleted` (mirrors the Phase 18 "secondary
  labels produce zero IMAP traffic" assertion style).

### Persistence & evolution
- New forward-only migration table `folder_rules` (M-next, following the
  M2–M12 template + preserve-rows test): id (stable UUID/slug), name,
  priority int, conditions JSON (sender/subject/regex/snippet fields +
  match kinds), destination folder name (display form) + destination kind
  (plain folder vs taxonomy category id), scope JSON (folder set, default
  all), enabled flag, created/updated_at. Conditions stored as data, never
  evaluated SQL (no injection surface).
- ID-stable taxonomy references: when a rule targets a taxonomy category,
  it stores the category ID (never the display name); renames never break
  rules; merge remaps rule destination ids (same migration as Phase 19
  label remap); delete reassigns via the same orphan-prompt (rules pointing
  at a deleted category fall back to the reassigned id or disable with a
  plain-language notice). Taxonomy version bumps stale-flag labels only —
  rules are revalidated, never silently dropped.
- List/edit/delete UI in options (next to TaxonomyEditor): enable/disable
  toggle (no confirm), edit (re-validates; invalid edit rejected, old rule
  kept), delete (immediate, no confirm — rules hold no mail, mirrors Phase
  23 filter-delete semantics). Rule names validated backend-side
  (non-empty, ≤80 chars, unique; plain-language errors).
- Redaction applies to any rule justification/logs (RULE-03 verbatim):
  rule-match explanations shown at confirm time are transient UI strings
  through `filter_output`, never persisted; logs carry rule-id + message-id
  only, never subject/sender content beyond what the rule itself names
  (Phase 16 sanitizer reused, same fixture-suite discipline).

### Agent Discretion
- Exact regex size-cap numbers within the stated bounds and excerpt lengths
  for rule-match explanations.
- Bulk `apply_rules` reuse depth (thin loop over `confirm_suggestion` vs
  full Phase 20 journal/resume/undo machinery — planner picks cheapest
  consistent option).
- Exact options-UI layout for the rules list (table vs card list — follow
  existing TaxonomyEditor/options patterns) and pt-BR dialog copy.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/commands/classify.rs:199` — `confirm_suggestion` (suggest →
  confirm → MOVE, label-row-required gate, `allow_nest` for `Auto`
  collision); rule hits flow through this same call.
- `src-tauri/src/classify/filing.rs:128` — `ensure_auto_tree` (delimiter-aware
  CREATE, `\Noselect` respected, user-owned-`Auto` confirm-then-nest);
  `filing.rs:216` gated MOVE (optimistic hide + `enqueue_imap_outbox`).
- `src-tauri/src/classify/taxonomy.rs:76` — `validate()` (cycles, dup IDs,
  `Auto` reserved, depth/width caps, plain-language errors) + `taxonomy.rs:58`
  reserved-name error; rule validation mirrors this style.
- `src-tauri/src/classify/taxedit.rs` — ID-stable edit/merge/delete + remap
  pattern for rule destination-id migration.
- `src-tauri/src/classify/worker.rs` — `ClassifyGate` single-flight drain +
  `classify_queue` staleness pattern; rule evaluation drains under this gate.
- `src-tauri/src/classify/redact.rs` (Phase 16) — `redact_input` /
  `filter_output` for rule explanations and logs.

### Established Patterns
- Forward-only migrations (M2–M12 template) + preserve-rows test for every
  schema change; pointer/ID-only storage (labels store IDs, rules store
  category IDs + condition data, never mail content).
- Behind-sync, never inline (Phase 17): post-sync hook enqueues, detached
  task drains; sync never awaits; sidecar/rules down ⇒ pending, sync
  unaffected.
- Backend-enforced validation with plain-language errors (Phases 11/16/18/19);
  UI restraint never the only guard; destructive ops refused backend-side.
- BODY.PEEK-only reads; UID-only addressing with UIDVALIDITY epoch gating;
  single SessionManager-owned session (`lease_for("INBOX")` default in
  `imap/manager.rs`); moves via `move_message_in` + `imap_outbox`.
- Worst case is `A Classificar`, never delete; `A Classificar` rows are
  un-confirmable as classifier destinations; rules never emit EXPUNGE.

### Integration Points
- `sync/worker.rs` post-pass hook site (3–5 line fire-and-forget enqueue,
  mirrors Phase 17 classify hook).
- New commands: `create_rule`, `list_rules`, `update_rule`,
  `delete_rule`, `set_rule_priority`/`reorder_rules`, `apply_rules`
  (on-demand); confirm path reuses `confirm_suggestion` unchanged.
- Store: new `folder_rules` table migration; taxonomy merge/delete
  migrations extend to remap rule destination ids.
- Frontend: rules section in options UI (next to TaxonomyEditor);
  confirm dialog extended with rule-vs-classifier comparison;
  existing MessageList badges render unchanged.

</code>

<specifics>
## Specific Ideas

- RULE-01/02/03 verbatim: validated keyword/sender/subject-regex rules;
  taxonomy conflicts resolved by user confirmation reusing
  `confirm_suggestion`; applies at sync or on-demand with UID-only +
  UIDVALIDITY gating; SQLite-persisted surviving taxonomy bumps via
  ID-stable references; list/edit/delete; never auto-delete (MOVE only);
  redaction on justifications/logs.
- Phase 23 CONTEXT explicitly defers rule-based auto-filing here ("saved
  filters are static stored queries, never auto-move") — no overlap:
  filters query, rules file.
- Sieve/server-side rule export stays out of scope (PROJECT.md Out of
  Scope: "Server-side filters/rules (Sieve) — local classification only").

</specifics>

<deferred>
## Deferred Ideas

- Rules import/export file format (UI-only authoring this phase;
  taxonomy JSON round-trip is the portability story).
- Learning rule suggestions from overrides / classifier agreement stats
  (cf. deferred Laya fine-tuning from overrides).
- Per-category auto-confirm ("always file X without asking" — already
  deferred in REQUIREMENTS.md Future Requirements).
- Server-side (Sieve) rule export — out of scope (a second product).

</deferred>
