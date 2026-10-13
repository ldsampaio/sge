# Phase 18: Confirm + Trust UX - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The user stays in control of every move: confirm before filing, one-click
override, visible badges, redacted justifications, tunable threshold.
Backend-enforced confirm gate (direct-IPC bypass refused), moves reuse
Phase 10 `move_message_in` + `imap_outbox` — zero new IMAP verbs.

</domain>

<decisions>
## Implementation Decisions

### Confirm gate (backend-enforced, not UI-only)
- `confirm_suggestion(message_id)` moves to `Auto/<Top>/<Child>` ONLY from
  an existing `labels` row (suggest→confirm→MOVE). No label row ⇒ refuse.
  Direct IPC with forged args cannot file unclassified mail.
- `ensure_auto_tree` pre-creates missing levels via Phase 11 CREATE guards
  (delimiter-aware, `\Noselect` respected); root ALWAYS `Auto`.
- `Auto`-root collision (user owns `Auto`): confirm-then-nest — first
  confirm when `Auto` exists but lacks our marker flag asks once
  ("usar Auto existente?"), then nests `Auto/<Top>/...` under it. Decision
  recorded per account.
- Offline confirms queue in `imap_outbox` and replay on reconnect (Phase 10
  machinery); `A Classificar` rows are un-confirmable (refuse as destination).

### Override (one-click correct)
- `override_label(message_id, to_category)` → MOVE to corrected folder +
  append `label_overrides` row + pin (future syncs never re-suggest the
  overridden label for that message).
- Override of a secondary-only disagreement still moves to the chosen
  primary (mail lives in ONE folder; secondary stays local-only).

### Trust surface
- Message-list badges: primary chip (category name) + distinct secondary
  chip style; queue-depth badge; `classifying` skeleton state on pending rows.
- Confirm dialog shows: destination path, confidence %, REDACTED
  justification (transient string via `filter_output`, never persisted),
  buttons Mover / Corrigir (dropdown of tops) / Dispensar.
- Threshold setting (0–1 slider, default 0.6): routes to `A Classificar`;
  changing it re-routes future suggestions only (no relabeling history).

### Agent Discretion
- Exact dialog copy (pt-BR, plain language) and badge styling within the
  existing design system.
- Marker mechanism for ours-vs-theirs `Auto` (folder flag vs local registry
  — planner picks; must survive folder LIST refresh).

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- Phase 10 `move_message_in` + `imap_outbox` + undo window; Phase 11 CREATE
  + roles/delimiter guards; Phase 16 labels/overrides/taxonomy; Phase 17
  suggest + events.
- `filter_output` for justifications; `match_keywords` for the Correct
  dropdown ordering.

### Integration Points
- New commands: `confirm_suggestion`, `override_label`, `dismiss_suggestion`,
  `set_confidence_threshold`, `ensure_auto_tree` (internal).
- Frontend: suggestion chip, confirm dialog, badges, threshold slider,
  `A Classificar` review list view.

</code>

<specifics>
## Specific Ideas

- Trust gate philosophy: worst case is `A Classificar`, never delete;
  secondary labels produce zero IMAP traffic (assert in tests).
- Live gate: one real confirm→MOVE→undo round-trip against
  mail.utfpr.edu.br (or documented deferral with the same rigor as v1.2).

</specifics>

<deferred>
## Deferred Ideas

Override history VIEW (log exists; UI later — already deferred).
</deferred>
