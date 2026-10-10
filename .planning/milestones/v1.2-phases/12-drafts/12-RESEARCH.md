# Phase 12: Drafts — Research

**Date:** 2026-10-07 | **Status:** Ready for planning
**Scope:** DRAFT-01 (local-first save/edit), DRAFT-02 (APPEND persist + expunge-old, exactly-one server copy). DRAFT-03 is Phase 13 scope — this phase only exposes `server_uid` + `dirty`.
**Method:** inline orchestrator research (no `gsd-phase-researcher` agent available in this runtime). Every claim below verified against vendored sources or repo files cited inline.
**Depends on:** Phase 11 (roles.rs `Role::Drafts` resolution, `create_folder` flow, `mutf7` helpers). Builds on: `SyncSession`/`SessionManager` (Phases 6/10/11), pre-sweep replay (Phase 10), command skeleton (Phase 6).

User decisions (binding, from `12-CONTEXT.md`): `drafts` table backs the editor (`id` compose-session, Drafts `mailbox_id`, subject/body/to/cc/bcc, `dirty`, `server_uid`, attachments JSON); explicit save + 30 s dirty-only autosave tick in the UI layer; every save = APPEND-new + expunge-old via Phase 10 machinery; `\Draft` flag (+`\Seen`); raw RFC 5322 bytes (full lettre MIME is Phase 14); Drafts role hard-coded from roles.rs, Missing → create behind confirmation (Phase 11 flow); offline saves stay `dirty=1` in the row, reconnect pass APPENDs newest + expunges superseded; last-writer-wins, no merge UI; discard = delete row + expunge tracked `server_uid` behind confirm-when-dirty.

---

## 1. async-imap 0.11 APPEND API (verified against vendored 0.11.3 source)

`~/.cargo/registry/src/.../async-imap-0.11.3/src/client.rs:1157-1191`:

```rust
pub async fn append(
    &mut self,
    mailbox: impl AsRef<str>,
    flags: Option<&str>,
    internaldate: Option<&str>,
    content: impl AsRef<[u8]>,
) -> Result<()>
```

- Wire form (vendored test `client.rs:2454-2471`): `APPEND "INBOX" (\Seen) {9}` → `+ OK` → literal bytes → tagged OK. Flags string passed through verbatim, e.g. `Some("(\\Draft \\Seen)")`.
- **Critical finding — APPENDUID is swallowed.** `append()` consumes the tagged response internally via `check_done_ok` and returns `Result<()>`; the `[APPENDUID <uidvalidity> <uid>]` response code never reaches the caller. There is no `append_full` variant in 0.11.3.
- **Consequence for the CONTEXT "APPENDUID when advertised" branch:** capturing APPENDUID would require re-implementing APPEND with raw `run_command` + literal write + `ResponseCode::AppendUid` parsing (imap-proto exposes it, but the plumbing is bespoke and untested upstream). **Recommendation (uniform path): always reconcile via `UID SEARCH HEADER Message-ID <id>` after APPEND.** One code path, no capability branch for the UID discovery; UIDPLUS still gates the *expunge* leg (`UID EXPUNGE` vs mark-deleted + plain `EXPUNGE` dance, existing `choose_expunge_path` in `imap/mod.rs:460`). The planner must NOT write an APPENDUID-parsing branch against `Session::append` — the API cannot provide it.
- `uid_search<S: AsRef<str>>(&mut self, query: S) -> Result<HashSet<Uid>>` (`client.rs:1254`) takes a raw query string, so `HEADER Message-ID <msg-id>` works with no new API. Returns set of **UIDs** (it issues `UID SEARCH`). New verb: `uid_search_header(field, value) -> PinBox<Result<Vec<u32>, SyncError>>` mirroring `search_uids` (`imap/mod.rs:525-535`).
- Quoting: APPEND mailbox goes through internal `validate_str` (CR/LF rejected, quoted) — pass the raw wire name resolved via roles (same rule as `uid_copy` dest).

## 2. Exactly-one-copy sequence (lease-hold rule applies)

`save = APPEND-new + expunge-old` must hold ONE `lease_for(drafts_wire)` guard and drive `lease.session()` directly for all three legs (append → uid_search_header → store_deleted + uid_expunge) — never call `self.mark_deleted_in` / `self.uid_expunge_in` re-entrantly while holding a lease (async mutex → deadlock; 10-PATTERNS §2 lease-hold rule). New manager method `save_draft_copy_in(&self, drafts_wire: &str, msg_id: &str, bytes: &[u8], old_uid: Option<u32>) -> Result<u32, SyncError>` with one reconnect-retry around the whole sequence (mirror `move_message_in`). Single-UID addressing throughout (no chunking — one draft = one message). Reconcile rule: SEARCH returns exactly one UID → use it; zero → `SyncError::Protocol` (append-then-invisible = server didn't persist, surface loudly, keep `dirty=1`); multiple → take max (newest wins, expunge-old still targets only the tracked `old_uid`).

UIDVALIDITY guard: read `get_sync_state` epoch for the Drafts mailbox before APPEND; on bump, the tracked `server_uid` is stale → pass `old_uid=None` (append fresh, skip expunge-old; the orphan is reaped by the next sweep's expunge-diff, same as Phase 9 semantics).

## 3. Raw RFC 5322 renderer (no new crates)

New pure module `src-tauri/src/drafts.rs` (renderer + Message-ID helper; store queries live in `store/queries.rs` per §4 pattern):
- `pub fn new_message_id(host: &str) -> String` — `<uuid@host>` (uuid crate already in tree? verify at plan time; else `format!("<{}@{}>", random hex, host)`). Stable per compose session: generated once at row creation, stored in the row (add `message_id TEXT NOT NULL` — required by the SEARCH-reconcile; CONTEXT field list omits it, planner records the addition).
- `pub fn render_draft_rfc5322(d: &DraftFields) -> Vec<u8>` — headers `From/To/Cc/Bcc/Subject/Date/Message-ID` + `MIME-Version: 1.0` + `Content-Type: text/plain; charset=utf-8`, CRLF, blank line, UTF-8 body. Non-ASCII display names / Subject → minimal `=?UTF-8?B?...?=` encoded-words (hand-rolled, ~20 lines; full encoder is Phase 14 lettre). No threading headers (CONTEXT: not needed yet). `Date` via `chrono` (already a transitive dep — confirm at plan time, else format from `std::time`).
- Addresses: as-typed strings joined with `, `; empty To/Cc/Bcc headers omitted (a draft with zero recipients must still APPEND — server accepts; send-time validation is Phase 13/14).
- Round-trip test with `mail-parser 0.11 full_encoding` (already in Cargo.toml:37): parse rendered bytes, assert subject/recipients/body survive, including `Assunto com acentuação` + `Lixeira & Cia` display names.

## 4. M9 `drafts` table (SCHEMA_VERSION 8 → 9)

Current: `SCHEMA_VERSION = 8`, migrations `schema.sql + M2..M8` (`store/mod.rs:19,83-90`). M9 DDL:

```sql
CREATE TABLE drafts (
  id            TEXT PRIMARY KEY,          -- compose-session id (UI uuid)
  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  message_id    TEXT NOT NULL UNIQUE,      -- stable per session, SEARCH-reconcile key (§1)
  subject       TEXT NOT NULL DEFAULT '',
  body          TEXT NOT NULL DEFAULT '',
  recipients_to TEXT NOT NULL DEFAULT '',
  recipients_cc TEXT NOT NULL DEFAULT '',
  recipients_bcc TEXT NOT NULL DEFAULT '',
  dirty         INTEGER NOT NULL DEFAULT 1,
  server_uid    INTEGER,                   -- NULL = never APPENDed
  attachments   TEXT NOT NULL DEFAULT '[]',-- staging refs only (picker UI is Phase 14)
  updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_drafts_mailbox ON drafts(mailbox_id);
CREATE INDEX idx_drafts_dirty ON drafts(dirty);
```

Forward-only + preserve-rows test copying `m7_adds_imap_outbox_and_pending_delete_preserving_rows` (`store/mod.rs:528-573`). Queries in `queries.rs` mirroring outbox templates: `upsert_draft` (`ON CONFLICT(id) DO UPDATE`, attempts-free), `get_draft`, `delete_draft`, `list_dirty_drafts` (reconnect pass), `mark_draft_clean(id, server_uid)`. Store lock never across `.await` (10-PATTERNS shared rule).

## 5. Reconnect drafts-sync pass (drafts are their own queue)

No separate outbox table (CONTEXT): the `dirty=1` rows ARE the queue. Worker addition `replay_dirty_drafts(session, drafts_mailbox_id, epoch)` called from the existing sync pass (post-sweep is fine — drafts APPENDs don't interact with the sweep's UID set; planner picks the call site next to `replay_imap_outbox`). Per dirty row: render from row → `save_draft_copy_in` with tracked `server_uid` → `mark_draft_clean`. Failure → row stays `dirty=1`, sync itself never fails (worker replay discipline, 10-PATTERNS §3). `sync_status.pending_count` adds dirty-draft depth (extend the BOTH-outboxes sum from Phase 10).

## 6. Command + UI contracts (frozen for parallel plans)

Tauri commands (`commands/sync.rs`, `set_seen` skeleton 179-276, `spawn_blocking`+`block_on`, lock never across `.await`):
- `save_draft(id: String, subject/body/to/cc/bcc: String, mailbox: Option<String>) -> Result<DraftSaveResult, String>` — resolve Drafts wire via roles (Missing → `Refused("drafts-missing")` so UI can run the Phase 11 create-confirm flow, then retry); upsert row `dirty=1` FIRST (local-first, no network wait); if online, APPEND+reconcile+expunge-old → `mark_draft_clean`; offline → return dirty with `acked:false`. `DraftSaveResult { id, dirty: bool, server_uid: Option<u32>, acked: bool, pending_count: usize }` — carries the Phase 13 handoff (`server_uid` + `dirty`).
- `get_draft(id) -> Result<DraftRow, String>` — local row; missing → rare server fallback (`fetch_body` + mail-parser extract, BODY.PEEK-only, never `\Seen`).
- `discard_draft(id) -> Result<_, String>` — delete row + scoped expunge of tracked `server_uid` (single UID, same lease-hold path); UI confirms only when dirty content exists (UI-side rule).
- UI (minimal — full compose UX is Phase 14): Drafts folder = existing message list filtered to Drafts wire name (no new list component); new `DraftEditor.tsx` (To/Cc/Bcc/Subject/body fields, Save button, Discard, "Salvo"/"Salvando…" indicator in `SyncStatus` tone); 30 s `setInterval` in the component firing save only when dirty (copy `SyncStatus.tsx:218` poll-timer pattern); "Novo rascunho" entry in the Drafts view (agent's discretion: reuse message list).

## 7. Live gate (CONTEXT §specifics)

One live gate in the server-sync plan: against `mail.utfpr.edu.br`, save draft with `SGE-Draft-Test-<ts>` subject → assert exactly one UID matches `UID SEARCH HEADER Message-ID` → re-save → assert exactly one UID again and old UID gone → `discard_draft` → assert zero. Cleanup on failure too. Manual (credentials + network).

## Validation Architecture

- **Unit (no network):** M9 forward migration preserves rows (template test exists); renderer round-trips through `mail-parser` (ASCII + `Assunto com acentuação` + multi-recipient + empty-To); MockSession arms (`appended_calls: Vec<(mailbox, flags, bytes)>`, `search_header_results`, `fail_append`) drive `save_draft_copy_in`: APPEND-then-reconcile returns searched UID; old UID marked+expunged; zero-result SEARCH → error + row stays dirty; multi-result → max UID.
- **Property under test (exactly-one-copy):** every save leaves ≤1 server copy per `message_id` — asserted in MockSession tests (old UID in expunged set, new UID returned) and in the live gate (SEARCH count == 1 after each save).
- **Regression:** full `cargo test -p sge` + `tsc --noEmit` green (Phases 6-11 unbroken); BODY.PEEK-only reads preserved (no `\Seen` set by draft paths except the APPEND `\Seen` flag itself, which is intentional per CONTEXT).
- **Live (manual, deferred-friendly):** the §7 gate; if credentials are unavailable it becomes a `/gsd-verify-work 12` item like the v1.1 deferred live items.
