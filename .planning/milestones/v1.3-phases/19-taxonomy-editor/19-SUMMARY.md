# Phase 19: Taxonomy Editor + Import — Summary

**Status:** complete (2026-10-10, autonomous run)
**Tests:** full suite 325 green (10 taxedit incl. malicious fixtures)

## What shipped (backend)

- `classify/taxedit.rs` — pure ID-stable ops: `apply_add` (slug ids,
  depth≤3/tops≤8/Auto-reserved), `apply_rename` (names only),
  `apply_merge` (siblings, children re-parent, ids kept), `apply_delete`
  (children must go first), `apply_keywords` (no version bump),
  `sanitize_import` (length/charset/dot-slash guards + validate).
- Commands: `list_taxonomy` (+label counts), `add_category` (+CREATE),
  `rename_category` (guard_rename + verb + cache migrate + re-LIST),
  `merge_categories` (move all mail + DELETE folder + remap + version++),
  `delete_category` (reassign-first + guards + remap + orphan assert),
  `update_category_keywords`, `import_taxonomy`, `export_taxonomy`.
- `remap_label_ids` + `labels_using` (orphan=0 asserted post-migration);
  overrides survive (untouched rows, ids remapped with labels).
- M15 `dismissed` (review hides, confirm stays possible).

## What shipped (UI)

- `TaxonomyEditor` modal (sidebar "Categorias"): tree + counts, add (top
  + sub), edit (rename + keywords/rules), delete with reassign dropdown,
  import picker (plain-language rejections) + export download.

## Verified requirements

- TAX-02 ✓ (add/rename/delete + keywords/rules; renames keep ids, merges
  remap, deletes reassign; overrides survive)
- TAX-03 ✓ (import validates cycles/dups/Auto/charset/depth/delimiter;
  invalid rejected with reason; export round-trips clean)

## Notes

- Merge stays backend-available (`merge_categories`); the editor exposes
  delete-with-reassign which covers the user need with fewer steps.
- Live folder-RENAME/DELETE vs real server: same standing deferral as
  Phase 18 (mock-level guards + verb reuse covered).
