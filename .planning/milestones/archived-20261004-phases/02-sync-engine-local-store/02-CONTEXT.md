# Phase 2: Sync Engine + Local Store - Context

**Gathered:** 2026-10-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Headers-first INBOX sync engine: UIDVALIDITY-guarded incremental fetch (200-UID ENVELOPE batches) into canonical SQLite store (WAL + FTS5 table created now), bodies + attachments on demand, file-backed attachments, progress over typed Channel events with offline badge. Manual refresh + 5-min poll. User accepted all recommended answers (no changes).

</domain>

<decisions>
## Implementation Decisions

### Sync Algorithm
- Fetch in 200-UID ENVELOPE batches per sweep
- Refresh via manual Refresh trigger + 5-minute poll while running
- Server-deleted mail expunges locally on UID diff (mirror server)
- IMAP flags (Seen etc.) ignored in M1; read/unread stays display-only (Phase 4)

### SQLite Store
- Canonical schema now: accounts, mailboxes + sync_state (uidvalidity/uidnext), messages, bodies, attachments metadata
- FTS5 virtual table created now and populated by the sync pipeline; search UI reads it in Phase 3
- Attachments file-backed under app-data dir with DB metadata — never SQLite blobs
- Cached bodies capped (e.g. 256 KB); oversized bodies fetch live on open

### Progress & Status
- Progress over typed ordered `Channel<SyncEvent>` (n/total + state), not plain emit
- Offline rule: failed poll/tick → offline badge reading cache; auto-resume on next tick
- Sync errors plain-language per pipeline (auth/TLS/protocol) with retry; protocol error = reconnect, never in-place retry (Phase 1 finding: poisoned sessions must not be retried)
- Launch freshness: quick incremental sync when session creds present; no auto-login UI bypass (lands on login until Phase 5)

### the agent's Discretion
- Exact body-size cap value, poll tick implementation detail, FTS tokenizer choice.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/imap/` session core (3-mode transports, 30s timeout, typed errors, `session_err(op)` naming) — sync engine builds on this session, must not fork a second IMAP path
- `src-tauri/src/creds.rs` KeyringStore/MemoryStore — sync reads creds from the same stores
- `.planning/phases/01-scaffold-connection/fixtures/` probe transcript (SOURCE: MIXED, Dovecot/993-only/AUTH=PLAIN) + `stub_server.py` — fixtures for sync tests
- Frozen IPC contract pattern (`connect_account`) — extend with `start_sync`/`sync_status`-style commands, same style

### Established Patterns
- `thiserror` typed errors → plain-language frontend mapping (reuse for sync errors)
- `spawn_blocking` for blocking calls (keyring); dedicated-thread discipline for sync I/O
- Grep gates (no auto-login, BODY.PEEK-only, no plaintext) — extend to sync code (no STORE/EXPUNGE writes, no full-body-first sync)

### Integration Points
- Tauri command/event bridge in `lib.rs` — add sync commands + Channel events here
- LoginForm connect flow — sync starts after successful INBOX SELECT

</code>

<specifics>
## Specific Ideas

- Phase 1 proven live facts: Dovecot server, 993-only, AUTH=PLAIN; STARTTLS path untested live — sync must work on the 993 path first, keep STARTTLS compatible
- Bodies on demand must use BODY.PEEK (never set \Seen) — AGENTS.md constraint
- Prior decision: protocol errors are fatal-session → reconnect (never retry in place)

</specifics>

<deferred>
## Deferred Ideas

- Auto-connect on launch → Phase 5
- Server flag writes (mark read) → post-M1
- Full-body backfill/idle task → post-M1
- Search UI → Phase 3 (FTS table ready)

</deferred>
