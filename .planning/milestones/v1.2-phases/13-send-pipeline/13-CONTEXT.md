# Phase 13: Send Pipeline - Context

**Gathered:** 2026-10-08
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, auto-accepted)

<domain>
## Phase Boundary

User's outgoing mail is never lost and never double-sent — queued durably offline, retried with backoff, filed to Sent exactly once. Covers SEND-04, SEND-06, DRAFT-03 (send-transaction consuming Phase 12 drafts). Depends on Phase 12 (drafts rows feed the pipeline; local row + `server_uid`/`dirty` exposed). Full compose/reply/forward UI stays Phase 14 — this phase ships the pipeline plus minimal outbox badge + failed-retry surface.

</domain>

<decisions>
## Implementation Decisions

### Queue durability & state machine
- New `send_queue` table via forward-only M10 migration (preserve-rows test like M7/M9): `id TEXT PRIMARY KEY`, `message_id TEXT NOT NULL UNIQUE` (assigned at enqueue), envelope columns (`from`, `to`, `cc`, `bcc` JSON), `eml_path` (MIME rendered once to `<app_data>/outbox/<id>.eml`), `state TEXT (queued|sending|sent|failed|uncertain)`, `attempts INTEGER`, `next_retry_at`, `last_error`, `draft_id NULL` (DRAFT-03 link), `created_at`
- Crash recovery at launch: reset `sending` → `queued` before any flush (same discipline as prior outbox replays)
- Backoff: exponential with cap + jitter; attempts counter drives `next_retry_at`; `failed` is terminal-with-manual-retry (user taps retry → `queued`), never auto-dropped

### Exactly-once delivery
- Message-ID assigned once at enqueue (`<...@sge.local>` via `drafts::new_message_id` when no draft, else draft's Message-ID reused); the `.eml` bytes are immutable after render — retries resend identical bytes
- SMTP DATA-timeout / connection-drop-after-send → `uncertain` state: reconcile-not-resend (check Sent via SEARCH Message-ID before any re-SMTP; found → mark sent + file, else single requeue)
- Double-click / double-invoke single delivery: enqueue dedupes on `message_id` UNIQUE + in-memory SendGate serializes flush per queue (separate gate from SyncGate, never holding the IMAP session)
- BCC envelope-only (never in headers); 25 MB send cap enforced at enqueue with plain-language error

### Sent filing
- On SMTP success: APPEND verbatim `.eml` bytes to Sent with `\Seen` via the Phase 12 `append_message` verb under a manager lease; APPEND failure → `sent-unfiled` state with APPEND-only retry (never re-SMTP-send)
- Server auto-save probe: some servers auto-file to Sent on SMTP — probe Sent via SEARCH Message-ID first; hit with same Message-ID → dedupe (skip APPEND), miss → APPEND
- Sent folder resolved via existing roles (`Role::Sent`); Missing → create behind confirmation reusing Phase 11 flow

### Draft handoff + transport + minimal UI
- Send reads the local `drafts` row (send-while-dirty allowed: render from local content); DRAFT-03 same-transaction delete: on SMTP success delete local row + expunge tracked `server_uid` (reuse `discard_server_copy_in` scoped path) before marking `sent`
- Transport: `lettre 0.11` sync SMTP over `spawn_blocking` (never block the async runtime); per-account client pool cached; fail-closed STARTTLS (`Tls::Required` equivalent, never opportunistic); credentials from the existing keyring path, no new secret storage
- Minimal UI in this phase: outbox/pending badge + failed entries with retry button (SyncStatus copy tone, pt-BR); full compose editor ships Phase 14
- Errors mapped to plain language, no secret leakage (reuse Phase 10 error-mapping discipline)

### the agent's Discretion
- Exact backoff base/cap constants (recommend 30 s base, 15 min cap, ±20% jitter)
- Outbox badge placement (recommend beside SyncStatus) and failed-retry list affordance (recommend reuse message-list pending wash)
- `.eml` retention policy for `sent` rows (recommend keep 7 days for debugging, then GC)

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- Phase 12 `append_message` verb + `DRAFT_FLAGS` pattern (Sent APPEND reuses verb with `\Seen` flags); `save_draft_copy_in` UID-reconcile via SEARCH Message-ID (same-Message-ID dedupe probe)
- Phase 12 `discard_server_copy_in` scoped-expunge path (DRAFT-03 draft cleanup); `drafts` table queries (`get_draft`, `delete_draft`, `list_dirty_drafts`)
- Phase 10 `imap_outbox` replay discipline (epoch-drop, pre-sweep replay, `{ acked, pending_count, detail }` tone) — template for `send_queue` flush pass in `sync/worker.rs`
- Phase 10 error mapping (plain language, no secret leakage); keyring credential path (`creds.rs`)

### Established Patterns
- Forward-only migrations with preserve-rows test (M9 template); UID-only addressing; BODY.PEEK-only reads (untouched — send path never FETCHes)
- `SendGate` new (separate from `SyncGate`); `spawn_blocking` for sync SMTP; Tauri command shape + pending wash
- Live gate per phase against `smtp.utfpr.edu.br:587` (deferred without credentials, per precedent)

### Integration Points
- New `send_queue` table (M10); lettre 0.11 sync transport (new dep) over `spawn_blocking` with per-account pool
- `sync/worker.rs`: flush pass on reconnect (after `replay_dirty_drafts`); crash-recovery reset at launch
- New Tauri commands: `queue_send` / `retry_send` / `send_status` (exact names at planner discretion, frozen with 14-03 consumer before UI binds)
- Phase 14 consumes: queued-send from compose, reply/forward MIME render via lettre `Message` builder (this phase renders plain-text test MIME; full multipart ships Phase 14)

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches. ROADMAP backend-hardening paragraph is the normative spec (lettre 0.11, state machine, SendGate, crash recovery, 25 MB cap, BCC envelope-only).

</specifics>

<deferred>
## Deferred Ideas

- Full compose/reply/forward/attachments UI (Phase 14)
- IDLE push, CONDSTORE/QRESYNC (deferred since v1.1)
- `.eml` GC job + sent-history view (post-v1.2)

</deferred>
