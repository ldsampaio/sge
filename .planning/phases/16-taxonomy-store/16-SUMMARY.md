# Phase 16: Taxonomy + Store — Summary

**Status:** complete (2026-10-10, autonomous run)
**Tests:** 21 classify + 56 store incl. preserve-rows — all green

## What shipped

- `classify/taxonomy_default.json` — UTFPR pt-BR taxonomy v1: 5 tops
  (Acadêmico, Administrativo, Financeiro, Oportunidades, Comunidade) + 18
  children with keywords + rules. Embedded via `include_str!` (offline).
- `classify/taxonomy.rs` — validation (cycles, dup IDs/siblings, depth ≤3,
  tops ≤8, leaf keywords, `Auto` reserved, self-parent, missing parent),
  `top_level`/`children_of`, diacritic-folding `match_keywords`. 13 tests
  incl. ID-snapshot (23 stable ids).
- `store` M12 — `taxonomy`, `labels` (pointer-only: ids+confidence, NO
  content columns exist), `label_overrides` (append-only),
  `classify_queue` (UIDVALIDITY epoch hygiene), `batch_runs`. Preserve-rows
  test (v11 seed → M12 upgrade intact). M13 — `classify_excluded_folders`.
- `store/queries.rs` — install/get taxonomy, upsert/get label,
  mark_stale_unknown, log/latest override, enqueue/dequeue/set_state,
  epoch-drop, queue_depth, batch lifecycle, exclusions + Sent/Drafts seed.
  6 round-trip tests.
- `classify/redact.rs` — `redact_input` (labeled secrets) + `filter_output`
  (+paranoid digit backstop) + `looks_clean`. 9 fixture tests incl.
  password-reset mail, boleto barcode, CPF/phone/RA, API keys, benign-text
  preservation (`código de conduta` survives).

## Design records

- Bare-space values require digit-shape (`código 739201` redacted,
  `código de conduta` kept) — precision/recall tradeoff documented in code.
- Input keeps transient digit signal (never persisted); output gets the
  backstop. Privacy is schema-structural (labels CANNOT hold content).
- Threshold-per-label (`threshold` column) makes future tuning auditable.

## Verified requirements

- TAX-01 ✓ (shipped versioned ID-stable taxonomy), CLS-06 ✓ (sanitizer +
  pointer-only schema + no-justification-persistence by construction).
