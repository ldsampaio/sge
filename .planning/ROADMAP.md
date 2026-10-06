# Roadmap: SGE — Linux IMAP Desktop Client

## Overview

v1.0 shipped a read-only INBOX viewer (Phases 1–5, archived). v1.1 Triage & Folders promoted it into a triage-capable multi-folder client (Phases 6–9, shipped 2026-10-05). v1.2 Compose & Organize ends the read-only era: the user creates, organizes, and sends mail — delete/move with Trash semantics and offline queue, IMAP folder CRUD, local-first drafts synced to the server, a durable SMTP send pipeline, and a compose UI with reply/forward and attachments. Each phase is a user-visible capability — backend hardening (manager-lease unification, SyncSession verbs, new outbox tables) ships inside the phase that needs it, never as a standalone infra phase.

**Build-order decision:** Organize-before-compose wins. Rationale: (1) delete/move is the moment destructive IMAP verbs (STORE \Deleted, EXPUNGE, MOVE/COPY fallback) enter the system, so the manager-lease unification precondition and the `imap_outbox` replay discipline must land alongside the first destructive op, not retrofitted after compose multiplies the blast radius; (2) folder CRUD reuses the same verb/manager surface the delete/move phase proves, so it follows immediately; (3) drafts are local-first editing plus APPEND reuse of the same machinery, and the send pipeline consumes drafts (DRAFT-03) — so drafts precede SMTP; (4) compose UI comes last because it is pure frontend over two already-proven pipelines (drafts + send queue), which keeps the highest-uncertainty work (live 587 send, MIME threading, attachment encoding) grounded in tested transport. Every phase still delivers observable user value in dependency order.

**Hard constraints honored across phases:**

- All IMAP traffic flows through one SessionManager-owned session — `start_sync` is routed under manager leases in Phase 10, before any destructive op ships; never a second IMAP connection for moves.
- BODY.PEEK-only reads hold on all fetch paths (M1 invariant survives the read-write era); deletes/moves are UID-only addressed with UIDVALIDITY epoch gating.
- `flag_outbox` contract from Phase 6 is untouched — delete/move queue lives in a separate `imap_outbox` table; send durability lives in `send_queue`. Forward-only migrations, each with a preserve-rows test.
- SMTP is fail-closed STARTTLS (`Tls::Required`, never opportunistic/dangerous); creds reuse the existing keyring path, no new secret storage.
- Error mapping to plain language with no secret leakage; at least one live gate per phase against `mail.utfpr.edu.br` / `smtp.utfpr.edu.br:587`.

## Milestones

- ✅ **v1.0 Read-only Viewer** - Phases 1-5 (shipped 2026-10-03, archived under `.planning/milestones/archived-20261004-phases/`)
- ✅ **v1.1 Triage & Folders** - Phases 6-9 (shipped 2026-10-05, archived under `.planning/milestones/v1.1-ROADMAP.md`, audit `.planning/v1.1-MILESTONE-AUDIT.md`)
- 🚧 **v1.2 Compose & Organize** - Phases 10-14 (in progress)

## Phases

**Phase Numbering:**

- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order. v1.2 numbering continues from v1.1 (ended at Phase 9).

- [x] **Phase 10: Delete + Move** - Trash-semantics delete, permanent expunge, and move between folders with offline queue
- [ ] **Phase 11: Folder CRUD** - Create, rename, and delete folders via IMAP with sidebar tree
- [ ] **Phase 12: Drafts** - Local-first draft editing synced to the server via APPEND
- [ ] **Phase 13: Send Pipeline** - Durable SMTP send queue with retry and Sent filing
- [ ] **Phase 14: Compose UI** - Compose, reply/forward, and attachments over the proven pipelines

## Phase Details

### Phase 10: Delete + Move

**Goal**: User can delete messages (Trash by default, expunge when explicit) and move them between folders, trusting it survives offline and never nukes other clients' mail
**Mode:** mvp
**Depends on**: Phase 9 (v1.1 complete)
**Requirements**: DEL-01, DEL-02, MOVE-01
**Success Criteria** (what must be TRUE):

  1. User can delete a message and see it land in Trash with an undo window (~5–10 s reverse-MOVE), instead of vanishing irreversibly
  2. User can permanently expunge (Shift+Delete / Empty Trash) only behind an explicit confirmation dialog, scoped so other clients' `\Deleted` flags in the folder survive
  3. User can move messages between folders (folder picker excludes `\Noselect`, INBOX, and Sent) with `\Seen` preserved and no re-FETCH of cached bodies
  4. User can delete/move while offline and see the op replay to the server on reconnect with a pending indicator until acknowledged — locally-deleted mail is never resurrected by the same-pass sweep (pre-sweep `imap_outbox` replay)

**Backend hardening ships inside:** `start_sync` routed under SessionManager leases (precondition for all destructive ops); `SyncSession` delete/move verbs + lease-scoped manager methods (MOVE with COPY+STORE+EXPUNGE fallback, UIDPLUS-gated UID EXPUNGE) with MockSession arms; `imap_outbox` durable queue table with UIDVALIDITY epoch-drop; optimistic local delete + `pending_delete` hidden state; attachment-dir + FTS cleanup on confirmed expunge.
**UI hint**: yes

### Phase 11: Folder CRUD

**Goal**: User can organize their mailbox tree — create, rename, and delete folders — and see the real server tree reflected in the sidebar
**Mode:** mvp
**Depends on**: Phase 10 (verb + manager surface proven)
**Requirements**: FOLD-04, FOLD-05, FOLD-06
**Success Criteria** (what must be TRUE):

  1. User can create a new folder via IMAP CREATE (hierarchy delimiter respected) and see it appear in the sidebar after LIST refresh
  2. User can rename a folder and see message selection caches invalidated, UIDs preserved, and the sidebar re-LISTed — no stale selection pointing at the old name
  3. User can delete a folder via IMAP DELETE with INBOX protected and non-empty folders guarded, and see local cache cascade cleanly
  4. Trash auto-detection works (special-use/role mapping, known names) with one-time CREATE fallback behind confirmation

**Backend hardening ships inside:** `create/rename/delete_mailbox` manager methods reusing the Phase 10 verb pattern; `mailboxes` bookkeeping (roles, delimiter, `\Noselect`/special-use attributes); raw modified-UTF-7 wire names; NAMESPACE stays banned with parser tripwire tests.
**UI hint**: yes

### Phase 12: Drafts

**Goal**: User can save and edit drafts locally and trust exactly one server copy exists per compose session
**Mode:** mvp
**Depends on**: Phase 11 (Drafts folder role resolved by folder CRUD)
**Requirements**: DRAFT-01, DRAFT-02
**Success Criteria** (what must be TRUE):

  1. User can save a draft explicitly (plus 30 s dirty-only autosave) and resume editing it later from the Drafts folder, local-first with no network wait
  2. User sees exactly one server copy per compose session — every save APPENDs new and expunges the old (`draft_uid` tracked), never a new copy per tick
  3. User can edit drafts offline and see them sync to the server on reconnect (offline drafts queue like the send queue)

**Backend hardening ships inside:** `drafts` table as local-first editor backing (`dirty`, `server_uid`, attachments JSON); save = APPEND-new + delete-old reusing the Phase 10 machinery; Drafts role hard-coded (no user-mappable folder paths).
**UI hint**: yes

### Phase 13: Send Pipeline

**Goal**: User's outgoing mail is never lost and never double-sent — queued durably offline, retried with backoff, filed to Sent exactly once
**Mode:** mvp
**Depends on**: Phase 12 (drafts feed the pipeline; DRAFT-03 send-transaction lands here)
**Requirements**: SEND-04, SEND-06, DRAFT-03
**Success Criteria** (what must be TRUE):

  1. User can queue mail while offline and see it flush on reconnect with backoff retries — no duplicate sends (Message-ID-at-enqueue, DATA-timeout → `uncertain` reconcile-not-resend, double-click single delivery)
  2. User sees sent mail APPENDEd to the Sent folder on success (verbatim bytes with `\Seen`); APPEND failure surfaces `sent-unfiled` with APPEND retry, never a re-SMTP-send; server auto-save probed with same-Message-ID dedupe
  3. User sending a draft sees it deleted in the same send transaction (no orphan drafts left in Drafts or on the server)
  4. A live send against `smtp.utfpr.edu.br:587` (STARTTLS, keyring creds) is verified end-to-end

**Backend hardening ships inside:** `lettre 0.11` sync transport over `spawn_blocking` with per-account pool caching; `send_queue` table (MIME rendered once to `<app_data>/outbox/<id>.eml`, envelope columns, state machine queued|sending|sent|failed + `uncertain`, attempts/backoff); `SendGate` separate from `SyncGate`; crash-recovery reset of `sending`→`queued` at launch; 25 MB send cap; BCC envelope-only.
**UI hint**: yes (outbox badge + failed-retry UX surface minimal here, full compose UX in Phase 14)

### Phase 14: Compose UI

**Goal**: User can compose, reply, forward, and attach files — the read-only era is over
**Mode:** mvp
**Depends on**: Phase 13 (send pipeline proven; this phase is frontend over proven transport)
**Requirements**: SEND-01, SEND-02, SEND-03, SEND-05
**Success Criteria** (what must be TRUE):

  1. User can compose a new message with To/Cc/Bcc and send it via SMTP (STARTTLS, keyring credentials) — offline compose queues, online flush delivers, arrives with a Sent copy, no duplicate
  2. User can reply with quote + correct threading headers (`In-Reply-To` = parent Message-ID, `References` extended, single `Re:` prefix, Reply-To honored, Reply-All minus own identity)
  3. User can forward a message with its attachments re-attached (single `Fwd:` prefix)
  4. User can attach files via picker and drag & drop, including inline images, within the send size guard — staged attachments with remove-before-send
  5. Milestone verification: compose offline → online flush → arrives + Sent copy is green end-to-end, and the v1.1 deferred live-validation items that touch the send path still hold

**Backend hardening ships inside:** MIME render via lettre `Message` builder (plain+HTML, `Attachment::new`/`new_inline`); threading round-trip test (emitted MIME parsed by `mail-parser` threads under the original); staged-attachment size guard. No new tables.
**UI hint**: yes

## Progress

**Execution Order:**
Phases execute in numeric order: 10 → 11 → 12 → 13 → 14

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Scaffold + Connection | v1.0 | 3/3 | Complete | 2026-10-02 |
| 2. Sync Engine + Local Store | v1.0 | 3/3 | Complete | 2026-10-03 |
| 3. Mailbox UI Shell + Search | v1.0 | 3/3 | Complete | 2026-10-03 |
| 4. Reader + Attachments | v1.0 | 3/3 | Complete | 2026-10-03 |
| 5. Keyring + Packaging | v1.0 | 3/3 | Complete | 2026-10-03 |
| 6. Flag Sync + Outbox | v1.1 | 3/3 | Complete | 2026-10-04 |
| 7. Folders + Per-Folder Sync | v1.1 | 3/3 | Complete | 2026-10-05 |
| 8. Poll + Manual Refresh | v1.1 | 1/1 | Complete | 2026-10-05 |
| 9. UID Backfill | v1.1 | 1/1 | Complete | 2026-10-05 |
| 10. Delete + Move | v1.2 | 4/4 | Complete | 2026-10-06 |
| 11. Folder CRUD | v1.2 | 0/4 | Not started | - |
| 12. Drafts | v1.2 | 0/TBD | Not started | - |
| 13. Send Pipeline | v1.2 | 0/TBD | Not started | - |
| 14. Compose UI | v1.2 | 0/TBD | Not started | - |
