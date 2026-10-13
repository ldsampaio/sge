# Phase 19: Taxonomy Editor + Import - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The user shapes the category system: add/rename/delete categories and edit
keywords/rules in options UI, plus JSON import (validated) / export.
ID-stable edits with migration — no orphaned labels, overrides survive.

</domain>

<decisions>
## Implementation Decisions

### Edit operations (all ID-stable)
- ADD: new UUID/slug id under a chosen parent (top or child, depth ≤3
  enforced); keywords required for leaves.
- RENAME: display-name only → folder RENAME via Phase 11 guards + relabel
  rows keep id (no label touch); sidebar re-LISTed.
- MERGE (rename onto existing sibling): MOVE all mail old→new folder +
  remap label ids + keep overrides (rewritten to new id).
- DELETE: orphan-prompt (reassign to… dropdown incl. `A Classificar`) →
  folder DELETE guard (non-empty blocked until empty) + remap + stale-flag
  affected overrides (kept for audit, never applied).
- Keywords/rules edits: in-place, version UNCHANGED (no relabel needed);
  structural ops bump taxonomy `version` + stale-flag rows whose id
  vanished (none should — merge remaps; delete reassigns).

### Import/export
- `import_taxonomy(file)`: parse → `validate()` (Phase 16: cycles, dup IDs,
  `Auto` reserved, depth/delimiter/charset sanitization) → invalid REJECTED
  with plain-language reason (first error + count); valid → version++ +
  install + stale-flag unknown old ids.
- Malicious-taxonomy sanitization: name length ≤80, charset strip control
  chars, delimiter `/` and `.` in ids rejected (folder-injection guard),
  depth/width caps.
- `export_taxonomy()` → JSON download, same schema as shipped default
  (round-trip: export→import validates clean).

### Agent Discretion
- Exact options-UI layout (tree editor vs master-detail — follow existing
  options patterns).
- Whether rename triggers immediate folder RENAME or lazy (planner picks;
  immediate is simpler to reason about).

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `classify/taxonomy.rs` validate + load; Phase 11 CREATE/RENAME/DELETE +
  roles/delimiter/mutf7 guards; Phase 18 `ensure_auto_tree` + move path;
  labels/overrides schema (Phase 16).

### Integration Points
- New commands: `list_taxonomy`, `add_category`, `rename_category`,
  `merge_categories`, `delete_category`, `update_keywords`,
  `import_taxonomy`, `export_taxonomy`.
- Frontend: TaxonomyEditor in options.

</code>

<specifics>
## Specific Ideas

- Editor arrives AFTER the engine it edits (roadmap order) — reuse proven
  move/rename paths, zero new IMAP verbs.
- Round-trip test: default export → import → version++ → labels intact.

</specifics>

<deferred>
## Deferred Ideas

Cross-device taxonomy sync (explicitly deferred, v2+).
</deferred>
