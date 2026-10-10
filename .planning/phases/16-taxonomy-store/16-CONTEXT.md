# Phase 16: Taxonomy + Store - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The app owns a versioned category system and a classification store — every
later phase reads from this foundation. Headless: shipped UTFPR pt-BR
default taxonomy JSON + `classify/taxonomy.rs` validation + M12 migration
(`taxonomy`, `labels`, `label_overrides`, `classify_queue`, `batch_runs`) +
shared redaction sanitizer. `cargo test`-only; no UI, no IMAP verbs, no
sidecar calls.

</domain>

<decisions>
## Implementation Decisions

### Default taxonomy shape (UTFPR pt-BR)
- 5 top-level categories + `A Classificar` fallback bucket (fallback is a
  review state, NOT a classifiable category — never offered as a choice
  option, never a MOVE destination):
  1. `Academico` — aulas, provas, notas, TCC, estagio, pesquisa, extensao, biblioteca
  2. `Administrativo` — documentos, requerimentos, protocolos, RH, TI, comunicados oficiais
  3. `Financeiro` — boletos, bolsas, auxilios, reembolsos, pagamentos
  4. `Oportunidades` — estagios, empregos, bolsas de estudo, eventos, cursos
  5. `Comunidade` — listas, diretorio academico, avisos gerais, divulgacao
- Each top-level has 3–5 children with pt-BR keywords + one-line rule hints
  (exact keyword lists at implementer discretion; planner seeds from
  representative UTFPR mail kinds).
- Category display names in pt-BR with accents (`Acadêmico`); IDs are
  stable ASCII slugs/UUIDs (see below), never the display name.
- Top-level count ≤ 8 hard cap (flat-choice accuracy cliff); children are
  keyword-resolved AFTER the top-level choice, never as extra options.

### Identity + versioning (P7 mitigation from day one)
- Every category has a stable UUID `id` + `version: 1` taxonomy version row;
  labels reference IDs, never names — renames never orphan labels.
- Taxonomy JSON shipped at `src-tauri/src/classify/taxonomy_default.json`
  (embedded via `include_str!`, always available offline).
- Future imports (Phase 19) bump `version` and stale-flag labels whose
  category id vanished — the `stale` column exists from day one.

### Store schema (M12, forward-only)
- Migration M12 following the M2–M11 template + preserve-rows test
  (existing rows survive migrate-up on a seeded M11 db).
- Tables: `taxonomy` (id, version, json, installed_at), `labels`
  (message_id → primary_category_id + secondary_category_id + confidence +
  threshold_at_classify + stale flag; POINTERS ONLY, never snippet/body),
  `label_overrides` (message_id, from_id, to_id, at — append-only log from
  day one, UI later), `classify_queue` (message_id, folder, status
  pending/processing/done/failed, attempts, enqueued_at; UIDVALIDITY epoch
  column for the Phase 17 worker's staleness drop), `batch_runs` (id,
  started_at, finished_at, state, totals json — journal rows land in
  Phase 20; table exists now so the migration is one-shot).
- FTS: no label indexing in this phase (Phase 20+ polish decision).

### Redaction sanitizer (designed once, reused everywhere)
- New module `classify/redact.rs` with TWO entry points: `redact_input`
  (before inference/evidence building) and `filter_output` (before any
  justification display/log line). Both must pass the same fixture suite.
- Patterns (pt-BR first): senhas (`senha`, `password`, `passwd`), códigos
  (`codigo`, `código`, `token`, `OTP`, 4–8 digit standalone codes),
  pessoais (`CPF` digits, matrícula RA patterns, telefone), chaves
  (`api[_-]?key`, `secret`, `bearer`). Replacement: `[REDACTED:<kind>]`
  (kind preserved so the classifier still sees a code-exists signal shape).
- Invariants enforced by tests: password-reset fixture mail → zero secret
  substrings in redacted output; justification strings are NEVER persisted
  (only transient UI in Phase 18); logs carry IDs/scores only.
- `Auto` root name reserved: taxonomy validation rejects any category whose
  name/path collides with `Auto` (case-insensitive).

### Validation rules (`taxonomy.rs`)
- Reject: cycles, duplicate IDs, duplicate sibling names, depth > 3,
  top-level count > 8, empty keywords on leaf children, `Auto` collision.
  Plain-language errors (no jargon) for the Phase 19 importer to reuse.
- Keyword matching helper (`match_keywords`) ships here for the child
  keyword-resolution step — pure function, unit-tested, reused by
  Phase 17 `suggest.rs`.

### Agent Discretion
- Exact keyword lists per child; exact regex list beyond the mandated kinds.
- Whether UUIDs are random-v4 generated once at implementation or
  deterministic slugs — either is fine as long as stable across edits.
- `classify_queue` index set (folder+status composite recommended).

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/store/` — M2–M11 migration template (`schema.sql` +
  queries + preserve-rows test pattern); single-SQL-module invariant.
- `src-tauri/src/sidecar.rs` (Phase 15) — key-redaction test pattern
  (`describe_and_errors_never_leak_key`) to mirror for justification hygiene.
- `src-tauri/src/imap/manager.rs` — UIDVALIDITY epoch-drop pattern for the
  queue's staleness column.

### Established Patterns
- Forward-only migrations, each with a preserve-rows test.
- thiserror typed errors → plain-language strings; secrets never in Display.
- Pointer-only storage (cf. drafts table stores local editor state, server
  copies by UID — labels likewise store IDs, never content).

### Integration Points
- `src-tauri/src/classify/` (new module dir; `mod.rs` + `taxonomy.rs` +
  `redact.rs`); registered in `lib.rs` as `pub mod classify`.
- No commands yet (Phase 17 adds `classify_message`/`classify_status`).
- No frontend changes in this phase.

</code>

<specifics>
## Specific Ideas

- Research SUMMARY.md Phase 2 spec is the blueprint: taxonomy JSON (5 top +
  children + fallback + rules), M12 tables, pointer-only labels, sanitizer
  before any justification exists. Do not re-litigate.
- Threshold default tuning is Phase 17's job (needs pt-BR sample); store the
  threshold used per label (`threshold_at_classify`) so later tuning is
  auditable.
- Override history VIEW is deferred (log table exists now, UI later).

</specifics>

<deferred>
## Deferred Ideas

None — Phase 5 editor, Phase 20 journal rows, and FTS polish are separate
phases, already roadmapped.

</deferred>
