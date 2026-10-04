# Roadmap: SGE — Linux IMAP Desktop Client

## Overview

M1 shipped a read-only INBOX viewer (Phases 1–5, archived). v1.1 Triage & Folders promotes it into a triage-capable multi-folder client: first the read/unread flag sync that ends the read-only era (with reconcile + durable outbox landing alongside the first STORE), then per-folder state and folder-tree browsing, then poll + manual refresh over one single-flight code path, and finally UID gap backfill as a convergence property of incremental sync. Each phase is a user-visible triage capability — backend hardening (SessionManager, single-flight, tombstoning) ships inside the phase that needs it, never as a standalone infra phase.

**Build-order decision (researcher disagreement resolved):** Position A — flags-first wins. Rationale: (1) it matches the research-suggested phase structure (flags → folders → poll → backfill), which all four researchers agreed on as components; (2) the first STORE is the moment M1's "server overwrites local" assumption breaks, so the flag-integrity contract (UID-only addressing, pending-wins merge, durable outbox) must be designed alongside it, not retrofitted after multi-folder complexity multiplies the blast radius; (3) every phase still delivers observable user value in dependency order — per-folder state lands in Phase 7 before poll (Phase 8) and backfill (Phase 9) need it, and the INBOX regression suite stays green throughout as the `mailbox="INBOX"` case. Position B's foundation concern is honored by putting SessionManager ownership + extended SyncSession trait inside Phase 6 (flag writes need mailbox-scoped leases from day one).

**Hard constraints honored across phases:**
- All IMAP traffic flows through one SessionManager-owned session (single-flight guard ships in Phase 8, before any poll timer fires).
- BODY.PEEK audit on all fetch paths ships in Phase 6, at the moment the read-only era ends.
- Every sync-state field keyed per folder (Phase 7); first STORE ships with reconcile + durable outbox (Phase 6).
- Poll timer and manual refresh share one `request_sync()` entry (Phase 8). IDLE/CONDSTORE/delete/move stay out of scope.

## Milestones

- ✅ **v1.0 Read-only Viewer** - Phases 1-5 (shipped 2026-10-03, archived under `.planning/milestones/archived-20261004-phases/`)
- 🚧 **v1.1 Triage & Folders** - Phases 6-9 (in progress)

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

<details>
<summary>✅ v1.0 Read-only Viewer (Phases 1-5) - SHIPPED 2026-10-03</summary>

- [x] **Phase 1: Scaffold + Connection** - Tauri shell that logs in and SELECTs INBOX on a real server
- [x] **Phase 2: Sync Engine + Local Store** - Headers-first INBOX sync into SQLite with status
- [x] **Phase 3: Mailbox UI Shell + Search** - Gmail-like three-pane UI with offline FTS search
- [x] **Phase 4: Reader + Attachments** - Sanitized message reading with attachment download
- [x] **Phase 5: Keyring + Packaging** - Secure auto-login and installable Linux bundle

Full phase details archived under `.planning/milestones/archived-20261004-phases/`.

</details>

- [ ] **Phase 6: Flag Sync + Outbox** - Mark read/unread with server-synced Seen flags and offline queue
- [ ] **Phase 7: Folders + Per-Folder Sync** - Browse Sent/Drafts/custom folders with unread counts
- [ ] **Phase 8: Poll + Manual Refresh** - Periodic and on-demand refresh over one single-flight path
- [ ] **Phase 9: UID Backfill** - No silent gaps; missed UIDs converge on incremental sync

## Phase Details

### Phase 6: Flag Sync + Outbox
**Goal**: User can triage read/unread state and trust it survives offline and server round-trips
**Mode:** mvp
**Depends on**: Phase 5 (M1 complete)
**Requirements**: FLAG-01, FLAG-02
**Success Criteria** (what must be TRUE):
  1. User can mark a message read/unread and see the toggle apply instantly, with the Seen flag confirmed on the server after sync
  2. User can toggle flags while offline and see them replay to the server on reconnect with a pending indicator until acknowledged
  3. A flag toggle never flaps or lands on the wrong message when a sync runs concurrently (UID-only STORE, pending-wins reconcile)
  4. No fetch path in the app sets \Seen as a side effect (BODY.PEEK audit holds — read-only-era regression class closed)
**Plans**: 3 plans

Plans:
- [ ] 06-01-PLAN.md — Backend STORE + SessionManager + outbox + reconcile
- [ ] 06-02-PLAN.md — Frontend toggle UX + pending states
- [ ] 06-03-PLAN.md — BODY.PEEK audit + hardening + phase gate
**UI hint**: yes

### Phase 7: Folders + Per-Folder Sync
**Goal**: User can browse every mailbox, not just INBOX, with per-folder unread triage signals
**Mode:** mvp
**Depends on**: Phase 6
**Requirements**: FOLD-01, FOLD-02, FOLD-03
**Success Criteria** (what must be TRUE):
  1. User sees the real server folder tree (Sent, Drafts, custom folders) in the sidebar, matching LIST discovery
  2. User can open any folder and browse its messages from local cache (headers-first, same fast list as INBOX)
  3. User sees an unread count badge per folder sourced from STATUS (UNSEEN)
  4. A UIDVALIDITY change in one folder triggers resync of only that folder — other folders' caches are untouched
**Plans**: TBD
**UI hint**: yes

### Phase 8: Poll + Manual Refresh
**Goal**: User's mail stays fresh without thinking about sync and never corrupts from overlapping syncs
**Mode:** mvp
**Depends on**: Phase 7
**Requirements**: SYNC-03
**Success Criteria** (what must be TRUE):
  1. User sees new INBOX mail arrive on the poll interval (configurable, default 5–10 min) without manual action
  2. User can trigger a manual refresh that runs through the same code path as the poll timer
  3. A poll firing mid-sync (or mid flag-STORE) never overlaps — one sync runs at a time on the single session
  4. User sees a honest "reconnecting…" state (not a fatal disconnect) when the session expires, and sync resumes after keyring re-read + re-SELECT
**Plans**: TBD
**UI hint**: yes

### Phase 9: UID Backfill
**Goal**: User never silently misses mail that arrived between syncs
**Mode:** mvp
**Depends on**: Phase 8
**Requirements**: SYNC-04
**Success Criteria** (what must be TRUE):
  1. User sees messages that arrived between two syncs appear after the next incremental sync (gap detected via UID range-diff, not just UIDNEXT walk)
  2. Expunged-on-server UIDs stop being re-requested (tombstoned after empty results — no infinite backfill loop)
  3. Double-poll-zero-FETCH convergence holds: two consecutive polls with no server change issue no message FETCHes
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 6 → 7 → 8 → 9

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Scaffold + Connection | v1.0 | 3/3 | Complete | 2026-10-02 |
| 2. Sync Engine + Local Store | v1.0 | 3/3 | Complete | 2026-10-03 |
| 3. Mailbox UI Shell + Search | v1.0 | 3/3 | Complete | 2026-10-03 |
| 4. Reader + Attachments | v1.0 | 3/3 | Complete | 2026-10-03 |
| 5. Keyring + Packaging | v1.0 | 3/3 | Complete | 2026-10-03 |
| 6. Flag Sync + Outbox | v1.1 | 0/TBD | Not started | - |
| 7. Folders + Per-Folder Sync | v1.1 | 0/TBD | Not started | - |
| 8. Poll + Manual Refresh | v1.1 | 0/TBD | Not started | - |
| 9. UID Backfill | v1.1 | 0/TBD | Not started | - |
