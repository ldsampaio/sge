# Phase 23: Search + Filters - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 23 upgrades the existing Phase 3 / store-level FTS search into a full
user-facing capability: SQLite search over subject, body snippet, sender,
attachment name, and folder scope with BM25 ranking; user-saved filter queries
(CRUD, applied at sync time or query time, persisted across restarts); and
multi-folder selectable search whose state (query + folder scope + sort)
survives app restart and reconnect. Covers SRCH-01/02/03. No new IMAP verbs —
search is local-SQLite only; no server-side SEARCH. No classifier/rules
interaction (Phase 25 owns rules).

</domain>

<decisions>
## Implementation Decisions

### Search scope & ranking
- FTS index extended to cover body text + attachment filenames: add `body_text`
  (from `message_bodies`, sanitized text only) and attachment filename column
  to the `messages_fts` table (new forward-only migration + trigger updates),
  keeping the existing subject/from_addr/preview columns — closes the SRCH-01
  gap (today only subject/from/preview are indexed).
- Default scope is whole-account (all folders, `mailbox = None` in
  `fts_search`); single-folder scoping is a user-selectable narrowing, and
  multi-folder selection (checkbox set of folders) filters results by folder
  name set — consistent with the existing `fts_search` None-vs-Some contract.
- Ranking stays BM25 (`ORDER BY rank`) as today; tie-break by `date_utc DESC`
  so recent mail surfaces first on equal relevance.
- Snippet highlighting: return the matched preview/body excerpt with match
  offsets (FTS5 `highlight()`/`snippet()`) for the reading-pane/list bolding;
  raw body HTML is never returned — sanitized text only (redaction-sanitizer
  invariant from Phase 16 extends here).

### Filter persistence & application
- Saved filters are CRUD rows in a new forward-only migration table
  `saved_filters` (id, name, query text, folder scope JSON, sort, created/
  updated_at) following the M2–M12 template + preserve-rows test.
- Filters apply at query time (stored query re-executed against FTS on demand,
  including from the filter list and on startup restore) — NOT at sync time;
  sync never awaits or branches on filters (mirrors the Phase 17 behind-sync
  discipline: sync path stays untouched).
- Filter names validated backend-side (non-empty, ≤80 chars, unique;
  plain-language errors reusing the Phase 16/19 validation style); delete is
  immediate with no confirm for user-created filters (they hold no mail).
- Search state (query + folder scope + sort + active filter id) persisted in
  SQLite (new `ui_state` key-value row or equivalent, not localStorage) and
  restored on launch and after reconnect; empty query restores the normal
  folder list view.

### UI surface
- Search box stays where it is: the existing `SearchBar.tsx` (300 ms debounce
  kept) at the top of the mailbox view; folder-scope selector (All / current
  folder / multi-select dropdown) added adjacent to it, following existing
  `MailboxView`/`MessageList` patterns — no new top-level route or pane.
- Results render in the existing `MessageList` (same row component + category
  badges from Phase 18), with a results header showing match count + scope +
  clear-search; selecting a cross-folder result switches to that folder
  (existing MailboxView jump behavior kept).
- Empty states: no-results shows the query + scope with a clear action;
  empty-filter-list shows a create affordance — plain pt-BR copy consistent
  with existing dialogs.
- Saved-filter management lives as a small dropdown/panel off the search bar
  (save current, apply, rename, delete) — NOT a separate options page.

### Performance & limits
- Result cap 200 rows per search (LIMIT), newest/ranked first, with a
  "showing first 200" hint; no pagination UI in this phase.
- Debounce stays 300 ms (existing `SearchBar` constant); each keystroke pause
  fires one local FTS query — no IPC storm risk since search is local SQLite.
- FTS index maintenance rides the existing triggers (`msg_ai`/`msg_ad` +
  equivalents for body/attachment updates on sync insert, body fetch, expunge,
  UIDVALIDITY wipe); backfill of body text for already-cached messages happens
  lazily at query time or via a one-shot migration re-index — planner picks
  cheapest consistent option.
- `pending_delete` rows stay hidden from search (existing `AND
  m.pending_delete = 0` predicate extends to the new columns).

### Agent Discretion
- One-shot vs lazy body-text backfill mechanism for already-cached rows.
- Exact `snippet()`/`highlight()` excerpt lengths and match-bolding markup.
- Exact folder multi-select widget (dropdown checkboxes vs chip row) within
  the existing design system.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/store/queries.rs:867` — `fts_search(conn, mailbox, query)`
  (BM25 `ORDER BY rank`, whole-account when `mailbox = None`, hides
  `pending_delete`); extend, don't replace.
- `src-tauri/src/store/schema.sql:67-76` — `messages_fts` FTS5 table (subject,
  from_addr, preview) + `msg_ai`/`msg_ad` triggers; new migration adds
  body/attachment columns + trigger coverage.
- `src-tauri/src/commands/sync.rs` — `search_messages` Tauri command (wired in
  `lib.rs:499`); new filter/state commands follow this pattern.
- `src/components/SearchBar.tsx` — 300 ms debounced search input with clear
  button (placeholder "Buscar em todas as pastas…"); scope selector attaches
  here.
- `src/components/MailboxView.tsx:40,99` — `searchQuery` state + cross-folder
  result jump; `src/components/MessageList.tsx` — result row rendering.

### Established Patterns
- Forward-only migrations (M2–M12 template) + preserve-rows test for every
  schema change; `mailbox.role` bookkeeping for folder identity.
- BODY.PEEK-only reads (M1 invariant, all fetch paths); UID-only addressing
  with UIDVALIDITY epoch gating; `pending_delete` hidden state until acked.
- Backend-enforced validation with plain-language errors (Phases 11/16/18/19);
  UI restraint never the only guard.
- Redaction sanitizer (`classify/redact.rs`, Phase 16): no secrets in DB/logs;
  labels store pointers, never content — search snippets return sanitized
  text only.

### Integration Points
- `fts_search` in `store/queries.rs` — scope/ranking/snippet extension point.
- `search_messages` command in `commands/sync.rs` + `lib.rs` invoke wiring —
  filter CRUD + state commands attach here.
- `SearchBar` → `MailboxView` → `MessageList` prop chain — scope selector,
  results header, filter panel attach here; Phase 18 badges render unchanged.
- `message_bodies.body_text` + `attachment_parts.filename` — new FTS sources;
  sync worker insert paths must keep triggers/index fresh.

</code>

<specifics>
## Specific Ideas

- SRCH-01/02/03 verbatim: local SQLite by subject, body snippet, sender,
  attachment name, folder scope; ranked; multi-folder selectable / save-edit-
  delete filters applied at sync or query time, persisted / query + folder
  scope + sort survives restart and reconnect.
- Whole-account default scope matches the existing `SearchBar` placeholder
  ("Buscar em todas as pastas") — keep that copy direction.

</specifics>

<deferred>
## Deferred Ideas

- Server-side IMAP SEARCH (local SQLite is the requirement; server search
  stays out of scope).
- Smart/rule-based auto-filing (Phase 25 owns rules; saved filters are static
  stored queries, never auto-move).
- Search pagination beyond the 200-row cap; FTS over classification labels
  (deferred since Phase 16/17).

</deferred>
