# Phase 12: Drafts - Context

**Gathered:** 2026-10-06
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, auto-accepted)

<domain>
## Phase Boundary

User can save and edit drafts locally and trust exactly one server copy exists per compose session. Covers DRAFT-01, DRAFT-02 (DRAFT-03 send-transaction lands in Phase 13). Depends on Phase 11 (Drafts folder role resolved by folder CRUD — roles.rs already resolves `\Drafts`).

</domain>

<decisions>
## Implementation Decisions

### Local-first editing
- `drafts` table is the editor backing store: one row per compose session (`id`, `mailbox_id` of Drafts, `subject/body/to/cc/bcc` fields, `dirty` flag, `server_uid` of last APPENDed copy, `attachments` JSON staging refs)
- Explicit save + 30 s dirty-only autosave (timer in UI layer like the poll timer, not Tauri runtime); clean (non-dirty) ticks never touch network or disk-write beyond the local row
- Opening a draft loads from local row instantly; server copy fetched only if local row missing (fallback, rare)

### Exactly-one server copy
- Every save = APPEND-new + expunge-old reusing Phase 10 machinery (mark_deleted + UID EXPUNGE scoped to the single old `server_uid`); `server_uid` updated to the APPENDUID-returned UID, or reconciled via post-APPEND UID SEARCH on Message-ID when server lacks UIDPLUS
- APPEND carries `\Draft` flag (and `\Seen`); MIME rendered minimal (plain text + headers) — full lettre MIME render ships in Phase 14, this phase builds raw RFC 5322 bytes sufficient for round-trip parse
- Drafts role hard-coded from roles.rs (no user-mappable folder paths); Drafts folder auto-resolved, Missing → create behind confirmation reusing Phase 11 flow

### Offline behavior
- Offline saves queue in the `drafts` row itself (`dirty=1`, `server_uid` stale marker) — no separate outbox; on reconnect the drafts sync pass APPENDs newest local content and expunges superseded server copies
- Send-while-dirty (Phase 13 handoff): send pipeline reads the local row, DRAFT-03 transaction defined in Phase 13; this phase exposes `server_uid` + `dirty` so send can consume them
- Conflict rule: server copy edited elsewhere (another client) is overwritten by next local save with no merge — last-writer-wins, documented; no conflict UI in MVP

### Discard
- Discard draft = delete local row + expunge tracked `server_uid` (single UID, same scoped-expunge path); behind a lightweight confirm only when dirty content exists

### the agent's Discretion
- Exact autosave tick copy/indicator (subtle "Salvo" vs "Salvando…" states following SyncStatus tone)
- Drafts list affordance: reuse message list filtered to Drafts vs dedicated drafts strip (recommend reuse message list — Drafts is a synced folder)

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- Phase 10 `mark_deleted_in` / `uid_expunge_in` + scoped-expunge discipline for APPEND-new + expunge-old
- `roles.rs` `Role::Drafts` resolution + `Role::Drafts => "Rascunhos"` display mapping (commands/sync.rs 1143)
- `flag_outbox`/`imap_outbox` replay pattern (worker.rs) for the reconnect drafts-sync pass
- Poll-timer-in-UI pattern for the 30 s dirty-only autosave tick
- `mutf7` helpers + `SYSTEM_ORDER` for Drafts display plumbing (already wired)

### Established Patterns
- UID-only addressing; BODY.PEEK-only reads; store mutex never across `.await`
- Forward-only migrations with preserve-rows test (M9 for `drafts` table)
- Tauri command shape + `{ acked, pending_count, detail }` result tone
- Optimistic UI + pending wash; plain-language errors, no secret leakage

### Integration Points
- `SyncSession` trait: add `append_message` verb (APPEND with `\Draft` flag + literal bytes; drain stream; map APPENDUID or return none for SEARCH-reconcile)
- New `drafts` table (M9 migration); Drafts folder `mailbox_id` resolved via roles at save time
- Full MIME via lettre ships Phase 14 — this phase's raw-bytes renderer must produce output `mail-parser` can round-trip (threading headers not needed yet)

</code>

<specifics>
## Specific Ideas

- One live gate: APPEND + expunge-old round-trip against mail.utfpr.edu.br with cleanup (delete test drafts after), verifying exactly-one-copy invariant
- DRAFT-03 (send deletes draft in same transaction) is Phase 13 scope — this phase only exposes the state send needs

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>
