# Phase 17: Classify Engine (No Moves) - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

Mail gets labeled automatically after sync — the user SEES the classifier
work before anything is ever allowed to move. Behind-sync queue, manual
single classify, exclusions, confidence gate → `A Classificar`, runner-up →
local-only secondary. Zero new IMAP verbs; the classifier never touches
IMAP. No MOVE in this phase (Phase 18 owns the confirm path).

</domain>

<decisions>
## Implementation Decisions

### Triggering: behind-sync, never inline
- Post-sync hook: 3–5 line fire-and-forget enqueue in `sync/worker.rs`
  after a folder pass completes (new UIDs since last classify watermark).
- Drain under a dedicated `ClassifyGate` (single-flight, mirrors SyncGate);
  worker loop lives in `classify/worker.rs` on a detached task.
- Sync NEVER awaits classification; classify NEVER blocks sync completion
  or UI paint. Sidecar down → rows stay `pending`, sync unaffected.

### Client shape (`bridge.rs`)
- Single-flight `reqwest` client (Mutex around inference call — one forward
  pass at a time; matches the server's single-worker pool).
- Reuses Phase 15 supervisor config (port + key); 10 s inference timeout
  (vs 2 s health); `POST /v1/systemone` per CONTRACT.md; 500 → `failed`
  with backoff, never panic.
- `classify_status` reports queue depth + sidecar state for the status
  surface; `ClassificationReady` Tauri events per labeled message.

### Question shaping (single-pass, hierarchical)
- ONE `choice` question per mail at TOP level only (5 options —
  `Acadêmico…Comunidade`; far below the 11-option overconfidence cliff).
- Child resolved by `match_keywords` (Phase 16) over redacted evidence —
  no second model call. No `score`, no batch route in this phase.
- State text: subject (≤200 chars) + snippet (≤1000) + signal tokens
  (sender domain, `tem anexo`, `é resposta`); quote-strip (`^>`, `-- `,
  `Em … escreveu:`) before budget-fit. NEVER full bodies.
- `redact_input` BEFORE inference (evidence), `filter_output` on any
  generated justification (none displayed yet — recorded for Phase 18).

### Confidence gate + fallback
- Shipped default threshold 0.6 (tuned on pt-BR sample in plan verification;
  exposed as setting in Phase 18). Below threshold OR model abstain →
  `A Classificar` review state (NOT a category, NOT a folder yet).
- Runner-up (second-highest top-level) recorded as local-only secondary
  label — zero IMAP traffic, badged distinctly in Phase 18 UI.
- `A Classificar` mail never moves on its own (structural; Phase 18's
  confirm path refuses it as a destination).

### Exclusions (SIDE-03)
- Per-folder opt-out: new M13 migration `classify_excluded_folders(folder
  TEXT PK)`; defaults seeded `Sent`, `Drafts` (role-resolved names at seed
  time; match by mailbox role AND name for robustness).
- Excluded folders: automatic pass skips them entirely (no queue rows);
  MANUAL `classify_message` still works on them (user intent overrides).
- Management UI is Phase 19's editor; this phase seeds + enforces.

### Manual path
- `classify_message(message_id)` command: redacted evidence → bridge →
  gate → label upsert → `ClassificationReady` event. Works on excluded
  folders. Reclassify = same path (overwrite label, no move).
- Errors in plain language; key material never in messages (Phase 15
  invariant extends here).

### Agent Discretion
- Exact evidence budget split (subject vs snippet vs signals) within the
  ≤200/≤1000 caps; exact queue drain batch size and poll interval.
- `min_confidence` request field vs client-side gate: prefer CLIENT-side
  (server field is a second opinion; our gate is authoritative and logged).
- Watermark storage (new `classify_watermark(folder, uidvalidity, last_uid)`
  vs reusing sync state — planner picks cheapest consistent option).

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/classify/` — taxonomy, redact, match_keywords (Phase 16).
- `src-tauri/src/sidecar.rs` — supervisor (port/key/probe), `MAX_RESTARTS`
  policy; `sidecar_status` command.
- `src-tauri/src/sync/worker.rs` — post-pass hook site; `SyncGate` pattern
  to mirror as `ClassifyGate`; `imap_outbox` UIDVALIDITY epoch-drop pattern
  for queue staleness.
- `sidecar/CONTRACT.md` — verified `/v1/systemone` shapes (choice/score/
  noul, usage, routing, `x_jev_confidence`).

### Established Patterns
- Forward-only migrations (M13 next); preserve-rows test per migration.
- thiserror → plain language; secrets never in Display/logs.
- Optimistic-local + durable-queue + event-notify (sync/flag pattern).

### Integration Points
- New: `classify/bridge.rs`, `classify/worker.rs`, `classify/suggest.rs`,
  `classify/evidence.rs`; commands `classify_message`, `classify_status`.
- Modified: `sync/worker.rs` (hook only), `lib.rs` (module cmds), store
  M13 (exclusions + optional watermark).
- Frontend: minimal status surface (queue depth + per-message label chip
  data via events); full badges/confirm UI is Phase 18.

</code>

<specifics>
## Specific Ideas

- CONTRACT.md is authoritative for the wire shape; the planner must NOT
  re-derive from older `/predict` docs.
- Accuracy gate (plan verification): stratified mini-eval on ~20 pt-BR
  mails (5 tops × 4) asserting ≥15/20 top-level agreement before calling the
  engine "sane" — threshold default adjusts from the misses.
- FTS over labels: explicitly NOT this phase.

</specifics>

<deferred>
## Deferred Ideas

None — confirm/override/badges (Phase 18), editor/import (Phase 19), batch
(Phase 20) are roadmapped.

</deferred>
