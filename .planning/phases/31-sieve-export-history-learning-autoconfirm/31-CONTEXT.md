# Phase 31: Sieve Export + History + Learning v1 + Auto-Confirm - Context

**Gathered:** 2026-10-14
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, batch proposals accepted)

<domain>
## Phase Boundary

The classifier compounds local value with its governor attached as one inseparable unit: (1) smart-folder rules + taxonomy filing exported as RFC 5228 Sieve text — preview, copy, `.sieve` download, golden-file tested, `fileinto`-only subset, non-expressible predicates as comments with a warning count (SIEVE-01); (2) override-history browser with per-category stats — the visible control surface and the dataset learning consumes (HIST-01); (3) keyword-weight + confidence-threshold adaptation from EXPLICIT override signals only — silence never approval, interpretable + reversible, no fine-tuning runtime (LEARN-01); (4) per-category auto-confirm on earned precision — opt-in per category, default off, visible precision display, override auto-disengages, journaled MOVEs with digest + one-click undo (CONF-01); (5) local auto-file and Sieve export coexist without flapping via "managed by Sieve" mutual-exclusion badges. Depends on Phase 29 (versioned rule envelope = predicate model the generator consumes) and Phase 28 (`override_events` dataset). Headless-first testing; UI hint yes. Phase 33 (ManageSieve upload) consumes the export text only if the port-4190 probe succeeded — no uploader code here.

</domain>

<decisions>
## Implementation Decisions

### Sieve Export Generator (SIEVE-01)
- Pure offline string generator: new `classify/sieve.rs` module consuming the Phase 29 rule envelope (`RuleExportItem`) + taxonomy category→folder mapping (`Auto/<Top>/<Child>` via `ensure_auto_tree` semantics) — zero IMAP traffic, zero new deps (no sieve-rs/sieve-generator: AGNOut-of-scope per REQUIREMENTS, one AGPL-tainted, one dead since 2022)
- `fileinto`-only subset: `sender_substring`/`sender_domain` → `address :contains/:is "From"`; `subject_keyword` → `header :contains "Subject"`; `snippet_keyword` → `body :contains` (guarded — body test only where the server advertises it, else comment + warning); `subject_regex` → comment + warning (RFC 5228 has no regex test; `regex` extension deliberately NOT required — keeps the export runnable on minimal servers); `scope.folders` restrictions → comments + warnings (Sieve has no local-scope concept); rule priority order → top-down `if/elsif` chain preserving first-match-wins (matches `evaluate_rule` OR-across-rules semantics)
- Non-expressible accounting: every skipped predicate emits a `# WARNING: <rule> — <reason>` comment inline AND increments a returned `warnings_count` surfaced in the UI next to preview/copy/download — success criterion's "warning count" is a first-class return value, not parsed from text
- Golden-file tests: 2-3 checked-in `.sieve` fixtures (rules-only, taxonomy-filing, mixed with warnings) byte-compared headless; generator is a total function `(&[RuleExportItem], &Taxonomy) -> (String, u32)` so tests need no DB and no network
- UI surface: RulesPanel gains an Export-Sieve section — text preview (read-only, redaction-filtered), [Copy] button, [Download .sieve] (frontend Blob download, no backend file write); export carries rule metadata only, never message content (Phase 29 T-29-03 precedent)
- Taxonomy filing leg: each leaf category maps to one `fileinto "Auto/<Top>/<Child>"` clause keyed by the same keyword set the Phase 17 child keyword-resolution uses — generator reuses `match_keywords` keyword lists, never a second keyword source

### Override History Browser (HIST-01)
- Read-only browser over the Phase 28 `override_events` table (actions confirm/override/dismiss/batch, `from_category/to_category/confidence/threshold`, `batch_run_id`) — new `history_list` + `history_stats` Tauri commands with pagination (limit/offset, newest-first), no new table, forward-only migration only if an index is missing
- Per-category stats computed in SQL (`GROUP BY to_category` / `from_category`): confirms, overrides, dismisses, batch counts + derived precision (`confirms / (confirms + overrides)`) per category — the SAME query feeds the visible stats UI and the CONF-01 precision display and the LEARN-01 signal aggregation (one dataset, three consumers, no divergent computations)
- Visible control surface: new `HistoryPanel.tsx` (or RulesPanel tab — planner's choice) showing event rows (timestamp, action, from→to, confidence) + per-category stat cards; every row shows IDs only, redaction sanitizer reused on any derived string (Phase 16 invariant)
- Learning-dataset surfacing: the panel shows the exact signal counts the learner will consume (explicit overrides per category) so the user can see what adaptation is based on — history + auto-confirm + learner ship as one visible unit, never a hidden model changing behavior
- Test strategy: headless `cargo test` over seeded `override_events` rows — pagination order, per-category counts, precision math (including divide-by-zero on zero-override categories); no fake-server transcripts needed (pure local reads)

### Learning v1 Adaptation (LEARN-01)
- Explicit-signals-only: learner consumes ONLY `action="override"` rows (correction from→to) and `action="confirm"` rows (endorsement) — `dismiss` and silence are training-neutral by construction (the aggregation query selects `WHERE action IN ('confirm','override')`, so neutrality is structural, tested by a `training_neutrality` test: dismiss-heavy + silent-majority histories produce zero adaptation)
- Adaptation mechanism: per-category keyword-weight adjustments + per-category confidence-threshold nudges stored as a small `learning_adjustments` table (or JSON sidecar — planner's choice, forward-only migration with preserve-rows test): override from A→B decrements the misleading keyword's weight for A and increments for B; repeated confirms lower the category threshold slightly, repeated overrides raise it — all bounded (caps prevent runaway: weights within [floor, ceiling], thresholds within [min, max], planner sets exact bounds)
- Interpretable + reversible: every adjustment row records `(category, keyword_or_threshold, delta, reason_event_id)` — the UI shows "porque N overrides de X→Y" per adjustment, and a [Reset learning] command deletes all adjustments returning to shipped defaults (reversibility is one command, tested)
- No fine-tuning runtime: zero sidecar/model changes — adaptation touches only the keyword-scoring path (`match_keywords` weight parameter) and the confidence-gate threshold lookup (`suggest.rs` threshold source); `LAYA_MAX_LOADED`, checkpoint, and bridge contract untouched
- Batch rows count: `action="batch"` moves log as weak confirms (they were user-initiated runs, not silent) — but at reduced weight vs explicit single confirms (prevents a single large batch from dominating the adaptation; exact discount factor at planner's discretion)

### Auto-Confirm Governor + Sieve Coexistence (CONF-01 + criterion 5)
- Opt-in per category, default OFF: new per-category `auto_confirm` flag (taxonomy sidecar table or settings JSON — planner's choice); enabling requires earned precision — UI shows the Phase 31 precision (`confirms/(confirms+overrides)`, minimum-sample floor e.g. N≥10 decisions, exact floor at planner's discretion) and refuses enable below the bar with a plain-language reason
- Override-flips-to-manual: any `override_label` against an auto-confirmed category immediately clears that category's `auto_confirm` flag (backend-enforced in the `override_label` command, not UI-only — direct-IPC override also disengages, tested by `override_flips_to_manual`); re-enable requires re-earning (precision re-check, not a toggle restore)
- Journaled MOVEs with digest + one-click undo: auto-confirmed moves ride the EXISTING Phase 20 journal (`batch_items` with a distinct `kind:"auto-confirm"` tag or `batch_run_id` per digest window — planner's choice) — the user gets a periodic digest (counts per category, planner's choice of cadence/surface) with the existing `undo_batch` one-click path; auto-confirm never invents a second journal
- Backend enforcement: auto-confirm moves go through the unmodified `confirm_suggestion` path (same MOVE verb + outbox machinery as Phase 18/20); the governor only decides WHETHER to call it without a dialog — no new MOVE verb, no bypass
- Sieve mutual exclusion (criterion 5): per-rule/per-category "managed by Sieve" badge — when the user exports Sieve text, the UI offers marking the exported rules/categories as Sieve-managed, which disables local auto-file/auto-confirm for exactly those predicates (local matcher skips Sieve-managed rules; auto-confirm skips Sieve-managed categories) so server-side `fileinto` and local MOVE never flap over the same mail; unmarking restores local handling; badges visible in RulesPanel + HistoryPanel precision cards
- Test strategy: headless `cargo test` — precision-gate refuse-below-bar, `override_flips_to_manual` (including direct-IPC path), digest-tagged journal rows undoable via existing `undo_batch`, Sieve-managed skip (managed rules never match locally); sandbox contract + exit-2 SKIP for any live leg

### the agent's Discretion
- Exact new-module filenames (`sieve.rs` vs `sieve_export.rs`, learner module name) and command names (`export_sieve`, `history_list`, `set_auto_confirm`, etc.) — planner's choice
- `learning_adjustments` storage shape (new table vs JSON sidecar) and exact bound values (weight floor/ceiling, threshold min/max, batch-discount factor, precision minimum-sample floor) — planner's choice within the bounded + reversible contract
- HistoryPanel as standalone component vs RulesPanel tab; digest cadence/surface (per-run card vs daily summary) — planner's choice
- Golden-file fixture count and exact `.sieve` formatting details (indentation, header comment block, `require` line set) — planner's choice (must stay RFC 5228 `fileinto`-only + optional `body` with capability note)
- Auto-confirm journal tagging (`kind:"auto-confirm"` vs per-window `batch_run_id`) and `auto_confirm` flag storage (sidecar table vs settings) — planner's choice

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/classify/rules.rs` — `RuleConditions` (the 5-field predicate model the generator consumes), `evaluate_rule` (AND-in-rule / first-match-wins semantics the `if/elsif` chain mirrors), `RuleDestination` MOVE-only enum, `is_destructive_destination`
- `src-tauri/src/classify/rules_io.rs` — versioned envelope (`RuleExportItem`, `EXPORT_VERSION=1`), `export_rules` byte-identical fixpoint, dry-run/commit parity pattern, warn-but-allow precedent (Sieve warnings mirror it)
- `src-tauri/src/classify/taxonomy.rs` — `Taxonomy`/`Category`, `match_keywords` (keyword lists the taxonomy-filing leg reuses), `fold` accent-folding, `Auto`-root reservation, leaf-keyword invariant
- `src-tauri/src/classify/suggest.rs` — confidence gate (`confidence >= threshold → routable`, `A Classificar` fallback) whose threshold lookup the learner nudges; `set_confidence_threshold` command precedent
- `src-tauri/src/store/queries.rs` — `log_override_event`, `OverrideEventRow` (M20 schema: `action[confirm|override|dismiss|batch], message_id, from_category, to_category, confidence, threshold, keywords, batch_run_id`), label threshold persistence
- `src-tauri/src/commands/classify.rs` — `confirm_suggestion` / `override_label` / `dismiss_suggestion` (fire-and-forget log hooks Phase 28; override command is where flips-to-manual enforcement lands), `batch_classify` per-message batch logging, `undo_batch`
- `src-tauri/src/classify/batch.rs` — journal-before-move intent-vs-fact `moved` flag, `BATCH_CHUNK=25`, per-chunk UIDVALIDITY re-check (auto-confirm journal reuses verbatim)
- `src/components/RulesPanel.tsx` + `src/components/BatchPanel.tsx`/`ArchivePanel.tsx` + `BatchPreviewTable.tsx` — existing rule CRUD + import/export buttons (29-03) the Sieve section extends; preview-table pattern the digest/history UI mirrors

### Established Patterns
- Forward-only migrations with preserve-rows tests; BODY.PEEK-only reads (generator touches no mail text at all — metadata only)
- Dry-run/commit parity + `dry_run_writes_nothing` DB-diff tests (Sieve preview writes nothing by construction — pure function)
- Golden-file / transcript tripwire tests (Phases 28/29/30) — Sieve fixtures follow the same pattern
- Backend-enforced guards, never UI-only (Phase 18 direct-IPC refusal precedent → override-flips-to-manual enforced in `override_label`)
- Fire-and-forget `override_events` logging (history/learner are read-side consumers, zero write-path changes)
- Redaction sanitizer on every new surface (Phase 16); sandbox contract + exit-2 SKIP for live legs; capability gating via `capability-profile.json`
- Feedback loop ships with its governor as one unit (ROADMAP binding: history + auto-confirm + learner inseparable — no plan may ship one without the others)

### Integration Points
- Phase 29 envelope → Phase 31 generator input (predicate model settled; generator must handle all 5 condition fields + both dest kinds + scope)
- Phase 28 `override_events` → Phase 31 history browser + learner aggregation + precision display (single dataset, three consumers)
- Phase 20 journal + `undo_batch` → auto-confirm MOVE journaling (distinct tag, same table, same undo)
- Phase 30 preview/retry → auto-confirm digest reuses the per-item plan shape for reviewability
- Phase 33 (conditional) consumes the Phase 31 export text — generator output must be upload-ready (`require "fileinto"` header, validated `fileinto` targets against the folder tree per criterion)

</code>

<specifics>
## Specific Ideas

- Generator signature `(&[RuleExportItem], &Taxonomy) -> (String, u32)` (text + warning count) keeps it a pure total function — golden-file tests with zero fixtures beyond inline rules
- `subject_regex` and `scope.folders` are the two guaranteed comment+warning cases; `snippet_keyword → body :contains` is conditional on server capability and commented when unsafe — planner confirms the exact `require` header set
- Precision formula `confirms / (confirms + overrides)` per category with a minimum-sample floor; `dismiss` excluded from numerator AND denominator (training-neutrality is arithmetic, not just query filtering)
- Sieve-managed exclusion is a skip-check preceding the verb/matcher (same structural position as Phase 30's retry skip-check), never a post-hoc filter
- HistoryPanel rows double as the learner's explanation surface ("N overrides X→Y drove this adjustment") — one query, two renderings

</specifics>

<deferred>
## Deferred Ideas

- ManageSieve upload mechanics (backup-then-diff, append/replace choice, `io-managesieve` dep) — strictly Phase 33, conditional on the Phase 27 port-4190 probe; Phase 31 produces upload-ready text only
- QRESYNC / Laya fine-tuning runtime — out of scope per REQUIREMENTS (tombstoning covers the gap; logging continues, no training runtime in v1.5)
- Multi-account Sieve namespacing — single-account invariant holds; generator assumes one folder tree

</deferred>
