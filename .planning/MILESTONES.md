# Milestones

## v1.1 Triage & Folders (Shipped: 2026-10-05)

**Phases completed:** 4 phases (6–9), 8 plans, ~24 tasks

**Key accomplishments:**

- Read/unread triage with server-synced Seen flags: optimistic toggle, UID-only STORE through a single-session SessionManager, durable SQLite outbox with RFC 4549 replay, pending-wins reconcile, BODY.PEEK audit (Phase 6).
- Multi-folder client: LIST-discovered folder tree, per-folder sync state + STATUS UNSEEN caching (M3), mailbox-aware commands, unread badges with server-datum fallback, per-folder UIDVALIDITY isolation (Phase 7).
- Freshness without overlap: 5-min poll timer + manual refresh through one shared entry, backend single-flight SyncGate, honest reconnecting state with auto-retry, cooperative cancel (Phase 8).
- No silent gaps: in-pass range-diff re-fetch, 3-strike tombstoning with prune/recovery, double-poll-zero-FETCH convergence with periodic full sweep (Phase 9).
- Suite: 102 backend tests green (63 → 102 across the milestone), tsc + vite build clean.

**Deferred:** 6 live-validation items (Seen round-trip, offline replay, folder tree/​badges vs webmail, poll arrival, reconnect recovery) — resume with `/gsd-verify-work N`. See `.planning/v1.1-MILESTONE-AUDIT.md`.

## v1.0 Read-only Viewer (Shipped: 2026-10-03)

Gmail-like three-pane INBOX viewer: scaffold + connection, sync engine + local store, mailbox UI + search, reader + attachments, keyring + Linux bundle.
