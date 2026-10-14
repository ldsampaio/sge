# Roadmap: SGE — Linux IMAP Desktop Client

## Overview

v1.0 shipped a read-only INBOX viewer (Phases 1–5, archived). v1.1 Triage & Folders promoted it into a triage-capable multi-folder client (Phases 6–9, shipped 2026-10-05). v1.2 Compose & Organize ends the read-only era: the user creates, organizes, and sends mail — delete/move with Trash semantics and offline queue, IMAP folder CRUD, local-first drafts synced to the server, a durable SMTP send pipeline, and a compose UI with reply/forward and attachments. Each phase is a user-visible capability — backend hardening (manager-lease unification, SyncSession verbs, new outbox tables) ships inside the phase that needs it, never as a standalone infra phase.

v1.3 Auto-Classify adds one new lane — on-device automatic email classification (Laya, fully offline) that organizes mail into an `Auto/` folder tree. Build order is risk-forced: the sidecar packaging spike goes first (the sole true unknown — bundle size, cold-start, lifecycle), then the taxonomy + store foundation, then suggestions without moves (trust gate: labels must prove themselves before anything is allowed to move), then confirm-gated MOVE + trust UX, then the taxonomy editor over the proven engine, and batch last (a loop over the proven confirm path with dialog off, progress on). Privacy is structural, not polish: the redaction sanitizer is designed up front in the store phase, before any justification is displayed or logged.

**Hard constraints honored across phases:**

- All IMAP traffic flows through one SessionManager-owned session — `start_sync` is routed under manager leases in Phase 10, before any destructive op ships; never a second IMAP connection for moves.
- BODY.PEEK-only reads hold on all fetch paths (M1 invariant survives the read-write era); deletes/moves are UID-only addressed with UIDVALIDITY epoch gating.
- `flag_outbox` contract from Phase 6 is untouched — delete/move queue lives in a separate `imap_outbox` table; send durability lives in `send_queue`. Forward-only migrations, each with a preserve-rows test.
- SMTP is fail-closed STARTTLS (`Tls::Required`, never opportunistic/dangerous); creds reuse the existing keyring path, no new secret storage.
- Error mapping to plain language with no secret leakage; at least one live gate per phase against `mail.utfpr.edu.br` / `smtp.utfpr.edu.br:587`.
- **v1.3 additions:** no mail content ever leaves the machine (loopback-only sidecar, no cloud fallback); single-email moves are always user-confirmed (backend-enforced at IPC level, not UI-only); worst case is `A Classificar`, never delete; `Auto` root name reserved for the classifier tree; justifications/logs never contain senhas/códigos/dados sigilosos.
- **v1.5 additions:** BODY.PEEK-only reads hold on every new fetch path (IDLE/CONDSTORE add no full-fetch); all addressing stays UID-only with UIDVALIDITY-first gating (stored modseq discarded on epoch change); single SessionManager session invariant holds — IDLE runs on a manager-owned watcher connection (`imap/idle.rs`, SELECT+IDLE+DONE+LOGOUT read-only verbs only, wakeup-only through SyncGate), never a side session; forward-only migrations with preserve-rows tests; `imap_outbox`/`send_queue` contracts untouched; redaction sanitizer reused by every new surface; sandbox contract for all live legs (exit-2 SKIP without creds, never create a creds file, self-only mail, pre-flight guard, post-run cleanup); zero new deps except probe-gated `io-managesieve` (`native-tls` only, added only if the port-4190 probe succeeds); no async-imap bump (0.11.3 stays); QRESYNC deliberately deferred (UID tombstoning covers the gap).

## Milestones

- ✅ **v1.0 Read-only Viewer** - Phases 1-5 (shipped 2026-10-03, archived under `.planning/milestones/archived-20261004-phases/`)
- ✅ **v1.1 Triage & Folders** - Phases 6-9 (shipped 2026-10-05, archived under `.planning/milestones/v1.1-ROADMAP.md`, audit `.planning/v1.1-MILESTONE-AUDIT.md`)
- ✅ **v1.2 Compose & Organize** - Phases 10-14 (shipped 2026-10-08)
- ✅ **v1.3 Auto-Classify** - Phases 15-20 (shipped 2026-10-10, audit `.planning/v1.3-MILESTONE-AUDIT.md`: PASSED 17/17, 328 tests green, deferred live gates documented)
- ✅ **v1.4 SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules** - Phases 21-26 (shipped 2026-10-13, archived under `.planning/milestones/v1.4-ROADMAP.md`, audit `.planning/v1.4-MILESTONE-AUDIT.md`: PASSED 11/11, 13 plans, live runs deferred-with-reason)
- 🚧 **v1.5 Live Validation + Compose Hardening + Deferred Features** - Phases 27-33 (in progress; Phase 33 conditional on the port-4190/ManageSieve probe)

## Phases

**Phase Numbering:**

- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order. v1.2 numbering continues from v1.1 (ended at Phase 9). v1.3 numbering continues from v1.2 (ended at Phase 14). v1.4 numbering continues from v1.3 (ended at Phase 20). v1.5 numbering continues from v1.4 (ended at Phase 26).

- [x] **Phase 10: Delete + Move** - Trash-semantics delete, permanent expunge, and move between folders with offline queue
- [x] **Phase 11: Folder CRUD** - Create, rename, and delete folders via IMAP with sidebar tree (completed 2026-10-06)
- [x] **Phase 12: Drafts** - Local-first draft editing synced to the server via APPEND (completed 2026-10-08)
- [x] **Phase 13: Send Pipeline** - Durable SMTP send queue with retry and Sent filing (completed 2026-10-08)
- [x] **Phase 14: Compose UI** - Compose, reply/forward, and attachments over the proven pipelines (completed 2026-10-08)
- [x] **Phase 15: Sidecar Packaging Spike** - Laya sidecar frozen, bundled, and supervised with measured cold-start
- [x] **Phase 16: Taxonomy + Store** - Versioned UTFPR taxonomy and M12 classification tables with redaction sanitizer
- [x] **Phase 17: Classify Engine (No Moves)** - Behind-sync suggestions with confidence gate, exclusions, and fallback bucket
- [x] **Phase 18: Confirm + Trust UX** - Confirm-gated MOVE, override, badges, justifications, and threshold setting
- [x] **Phase 19: Taxonomy Editor + Import** - Options UI for editing categories and importing/exporting taxonomy JSON
- [x] **Phase 20: Batch Reorganization** - Whole-account classify with progress, report, resume, and undo-batch
- [x] **Phase 27: Live-Validation Gate + Capability Probes** - Real-creds live-21/22/26 runs, draft-send gap closure, Node 20+ bundle, human UAT, protocol probes (completed 2026-10-13)
- [x] **Phase 28: Override-Event Log + CONDSTORE Fast Path** - Headless sync core: telemetry log + MODSEQ-delta resync (completed 2026-10-13)
- [x] **Phase 29: Rule Import/Export + IDLE Watcher** - Versioned rule JSON portability + INBOX push over a manager-owned watcher (completed 2026-10-14)
- [x] **Phase 30: Batch Trust Pair** - Dry-run preview + retry-only-failures over a per-item run record (completed 2026-10-14)
- [ ] **Phase 31: Sieve Export + History + Learning v1 + Auto-Confirm** - Feedback loop ships with its governor as one unit
- [ ] **Phase 32: Compose Hardening** - Send/drafts/reply/reply-all verified end-to-end over v1.2 pipelines (parallelizable after Phase 27)
- [ ] **Phase 33 (conditional): ManageSieve Upload** - Server-side rule upload only if the port-4190 probe succeeds; otherwise export-only stands

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

### Phase 15: Sidecar Packaging Spike

**Goal**: The Laya classifier runs as a bundled offline sidecar the app can spawn, health-check, and kill — packaging risk retired before any classification UX exists
**Mode:** mvp
**Depends on**: Phase 14 (v1.2 complete; greenfield lane, no code dependency)
**Requirements**: SIDE-01, SIDE-02
**Success Criteria** (what must be TRUE):

  1. User installs the Linux bundle (.deb/AppImage) and the classifier works on first run with no network and no manual model download — sidecar binary present with the correct target-triple suffix, weights shipped as Tauri resources, multilingual checkpoint only (`LAYA_MAX_LOADED=1`)
  2. User never waits on the model at startup or in the UI — cold-start happens backgrounded (measured in seconds and documented), sync and UI never block on it
  3. User quits the app and no orphan sidecar process survives — spawn in `setup()`, kill-on-exit, health probe green in dev AND in the built bundle, restart recovers from a crashed sidecar

**Backend hardening ships inside:** `laya[serve]==0.4.2` frozen via PyInstaller into one `sge-laya-<target-triple>` binary (CPU-only torch); Tauri `externalBin` + `resources` + `shell:allow-execute` wiring; loopback-only env (`LAYA_HOST=127.0.0.1`, preload, per-boot API key); `tauri-plugin-shell 2` supervision; checkpoint Hub revision pinned (hash + sha256 in the build script). No classification logic yet — hello-world `/health` only.
**Plans**: TBD

### Phase 16: Taxonomy + Store

**Goal**: The app owns a versioned category system and a classification store — every later phase reads from this foundation
**Mode:** mvp
**Depends on**: Phase 15 (sidecar binary contract exists; store is headless and independent of serving)
**Requirements**: TAX-01, CLS-06
**Success Criteria** (what must be TRUE):

  1. User's install ships the UTFPR pt-BR default taxonomy (hierarchical, versioned, ID-stable — 5 top-level + children + fallback + rules) visible as the category source the classifier will use
  2. User's labels, overrides, queue entries, and batch runs persist across restarts in the local store — forward-only M12 migration (`taxonomy`, `labels`, `label_overrides`, `classify_queue`, `batch_runs`) with a preserve-rows test, pointer-only labels (IDs, never email content)
  3. User with a password-reset email in the mailbox can trust no secret ever lands in the database, logs, or diagnostics — shared backend redaction sanitizer (input redaction + output filter) designed once here and reused by every later phase; labels store pointers, never justification text

**Backend hardening ships inside:** `classify/taxonomy.rs` validation (no cycles, no duplicate IDs, `Auto` root reserved); M12 migration following the M2–M11 template; stable-UUID category IDs + taxonomy versioning so future edits never orphan labels (P7 mitigation from day one); log hygiene (IDs/scores only, never bodies).
**Plans**: TBD

### Phase 17: Classify Engine (No Moves)

**Goal**: Mail gets sensibly labeled automatically after sync — the user sees the classifier work before anything is ever allowed to move
**Mode:** mvp
**Depends on**: Phase 16 (taxonomy + store + sanitizer exist)
**Requirements**: CLS-01, CLS-02, CLS-04, SIDE-03
**Success Criteria** (what must be TRUE):

  1. User syncs a folder and sees sane pt-BR category labels appear on new mail shortly after — sync itself never waits on the model (behind-sync queue under `ClassifyGate`, fire-and-forget post-sync hook); if the sidecar is down, mail stays `pending` and sync is unaffected
  2. User can classify or reclassify a single email manually and see its label update without anything moving folders
  3. User can exclude folders from auto-classify (per-folder opt-out, e.g. Sent/Drafts never auto-filed) and sees excluded folders stay unlabeled by the automatic pass while manual classify still works on them
  4. User sees low-confidence and edge mail land in the `A Classificar` review bucket instead of being misfiled — confidence gate with a sensible shipped default, runner-up recorded as the local-only secondary label

**Backend hardening ships inside:** `classify/` module (`bridge.rs` `/v1/systemone` single-flight client + health probe + ephemeral port + per-boot key; `worker.rs` queue drain; `suggest.rs` confidence gate → `A Classificar`, runner-up → secondary, child keyword-resolved); evidence-selection pipeline (quote-strip → signal-extract → budget-fit; subject ≤200, snippet ≤1000); redaction-before-inference + output filter from Phase 16; `classify_message` + `classify_status` commands with `ClassificationReady` events. Zero new IMAP verbs; the classifier never touches IMAP directly.
**UI hint**: yes (classify status + label display surface; full badges/confirm UI lands in Phase 18)
**Plans**: TBD

### Phase 18: Confirm + Trust UX

**Goal**: The user stays in control of every move — confirm before filing, one-click correct, visible badges, and redacted justifications
**Mode:** mvp
**Depends on**: Phase 17 (suggestions exist and are trustworthy; moves reuse the proven Phase 10/11 verbs)
**Requirements**: CLS-03, CLS-05, TRUST-01, TRUST-02, TRUST-03
**Success Criteria** (what must be TRUE):

  1. User confirms a suggestion and sees the mail MOVE into the auto-created `Auto/<Top>/<Child>` tree (root always `Auto`) — suggest → confirm → MOVE; offline confirms replay on reconnect; a direct-IPC confirm bypass is refused backend-side
  2. User can override a classification in one click and sees the mail re-move to the corrected folder with the override logged (and pinned across future syncs)
  3. User sees category badges in the message list — primary category per email, local-only secondary badged distinctly — plus queue-depth badges and `classifying` list states; secondary labels produce zero IMAP traffic
  4. User sees the redacted justification for a suggestion at confirm time (transient UI string, never persisted) and can tune the confidence threshold that routes mail to `A Classificar`; `A Classificar` mail never moves on its own

**Backend hardening ships inside:** `confirm_suggestion` / `override_label` commands; `ensure_auto_tree` + existing Phase 10 `move_message_in` + `imap_outbox` (zero new verbs); `A Classificar` review list; override-log writes; `Auto`-root collision handling (user already owns `Auto` → confirm-then-nest, decided at plan time).
**UI hint**: yes
**Plans**: TBD

### Phase 19: Taxonomy Editor + Import

**Goal**: The user can shape the category system — edit categories and import/export taxonomies without orphaning labels
**Mode:** mvp
**Depends on**: Phase 18 (engine + move path exist; edits are folder-rename + label-remapping problems that need the confirm path underneath)
**Requirements**: TAX-02, TAX-03
**Success Criteria** (what must be TRUE):

  1. User can add, rename, and delete categories and edit keywords/rules in the options UI — ID-stable edits with migration (rename → folder RENAME + relabel, merge → MOVE + remap, delete → orphan-prompt), no orphaned labels, overrides survive
  2. User can import a taxonomy JSON file and sees it validated before anything changes (no cycles, no duplicate IDs, `Auto` root reserved, charset/depth/delimiter sanitization) — invalid files are rejected with a plain-language reason; valid imports bump the version and stale-flag old labels
  3. User can export the current taxonomy to JSON for portability across machines

**Backend hardening ships inside:** `import_taxonomy` command (validate → version++ → stale-flag); rename/merge/delete migration reusing Phase 11 CREATE/RENAME guards + Phase 18 move path; malicious-taxonomy sanitization.
**UI hint**: yes
**Plans**: TBD

### Phase 20: Batch Reorganization

**Goal**: The user can reorganize the whole account in one explicit run — with progress, a persisted report, resume, and undo
**Mode:** mvp
**Depends on**: Phase 18 (confirmed moves proven — batch is a loop over that path with dialog off, progress on)
**Requirements**: BATCH-01, BATCH-02
**Success Criteria** (what must be TRUE):

  1. User can run whole-account batch classification with no per-email confirmation and watch live progress — full `Auto/` tree pre-created up front, chunked 25–50 with per-chunk UIDVALIDITY re-check, auto-confirm at or above threshold
  2. User gets a persisted per-category report after the run and can review what moved where, including the `A Classificar` remainder
  3. User survives a mid-run failure intact — runs are journaled per message and resumable without dupes or skips; a UIDVALIDITY shift aborts to resync; undo-batch restores originals so a half-filed account never strands mail

**Backend hardening ships inside:** `batch_classify` command + `batch_runs` table (bulk enqueue, per-message move journal enabling resume + undo-batch); interplay with `imap_outbox` verified; live large-folder gate against the real throttling server (chunk sizing tuned at plan time).
**UI hint**: yes
**Plans**: TBD

## Phase Details (v1.4 — shipped 2026-10-13, full details archived)

> v1.4 phases collapsed to one-line entries at milestone close. Full phase details: `.planning/milestones/v1.4-ROADMAP.md`. Audit: `.planning/v1.4-MILESTONE-AUDIT.md` (PASSED 11/11).

- [x] **Phase 21: SMTP Send — Live Gate** — live_gate_21 STEPs 1-12 + REPORT + SCAN-DB (19 bin + 42 safety green, exit-2 SKIP; live run pending human via scripts/live-21.sh)
- [x] **Phase 22: Draft & Seen Round-Trip — Live Gate** — live_gate_22 STEPs 1-13 (19/19 safety green, exit-2 SKIP; live run pending human via scripts/live-22.sh)
- [x] **Phase 23: Search + Filters** — M17 FTS rebuild + M18 saved filters + search UI (345 lib green, tsc clean; vite bundle blocked environmentally — Node 18 vs required 20+)
- [x] **Phase 24: Auto-Archive** — archive_run on verbatim batch machinery + ArchivePanel (472 green, tsc clean; scheduler structurally absent by design)
- [x] **Phase 25: Smart Folder Rules** — folder_rules M19 + matcher + conflicts + RulesPanel (392 lib green incl. 37 rule tests, tsc clean)
- [x] **Phase 26: Live Validation Bundle** — live_gate_26 STEPs 1-15 + BUNDLE-REPORT (bin 15/15, safety 48/48 green; live pending human via scripts/live-26.sh)

## Phase Details (v1.5 — active)

> Build order per research `research/SUMMARY.md` (binding): probes first (Phase 27 gates every protocol decision), telemetry-before-consumers + cheap-pass-before-push (Phase 28 before 29), model-before-generator (Phase 29 before 31), feedback loop ships with its governor (Phase 31 is one unit), Sieve upload strictly conditional (Phase 33), compose off the critical path (Phase 32 parallelizable after Phase 27).

### Phase 27: Live-Validation Gate + Capability Probes

**Goal**: Every open live-validation thread is closed against the real servers and the app is human-verified end-to-end
**Mode**: mvp
**Depends on**: Phase 26 (v1.4 complete; sandbox contract + live-21/22/26.sh harnesses exist)
**Requirements**: LIVE-04, LIVE-05, UAT-01, UAT-02
**Success Criteria** (what must be TRUE):

  1. Operator runs `live-21/22/26.sh` with own git-ignored creds under the sandbox contract and each leg is green or newly deferred with reason — transcripts secret-grep-clean, post-run cleanup verified
  2. Draft-send gap is closed — sends leave no orphan drafts — or explicitly deferred with reason (the FAIL BY DESIGN verdict is resolved either way)
  3. `npm run build` succeeds on a Node 20+ host (rolldown-vite `node:util` styleText requirement) producing the Linux bundle
  4. Human completes the click-through UAT (scoped search/filter restore, archive rule save → Arquivar agora → progress → report → undo, rules live end-to-end tracer) with results recorded
  5. Capability probes are recorded and gate downstream work — real-server CAPABILITY atoms (IDLE/CONDSTORE/QRESYNC), per-account capability profile, port-4190/ManageSieve probe transcript deciding Phase 33

**Plans**: 2/2 (27-01 probe tracer + transcripts + capability-profile; 27-02 credentialed deltas + UAT/BUNDLE scaffolds)

### Phase 28: Override-Event Log + CONDSTORE Fast Path

**Goal**: Reconnects resync fast over CONDSTORE deltas while every correction is banked for future learning
**Mode**: mvp
**Depends on**: Phase 27 (capability profile confirms CONDSTORE advertisement before wiring)
**Requirements**: SYNC-01
**Success Criteria** (what must be TRUE):

  1. User reconnects and sees flags/folders converge via MODSEQ deltas (CHANGEDSINCE) without a full UID range-diff — the fast path is measurably cheaper than baseline
  2. A UIDVALIDITY epoch change discards stored modseq with mailbox state (never trusted across epochs) and falls back to full resync — covered by an epoch-clears-modseq test plus transcript tripwire tests
  3. Servers without CONDSTORE converge exactly as before via the UID range-diff fallback — no hard failure, silent degradation only
  4. Every confirm/override/dismiss/batch decision appends to the `override_events` log (the dataset future learning consumes) — verified headless via `cargo test`, no UI in this phase

**Plans**: 2/2 (28-01 override_events tracer; 28-02 CONDSTORE fast path)

### Phase 29: Rule Import/Export + IDLE Watcher

**Goal**: Users carry their rules across machines as validated JSON and receive new INBOX mail instantly via push
**Mode**: mvp
**Depends on**: Phase 28 (cheap CONDSTORE pass exists so push wakeups run the fast path)
**Requirements**: IMP-01, IDLE-01
**Success Criteria** (what must be TRUE):

  1. User can export smart-folder rules to versioned JSON and import them on another machine — invalid files (empty patterns, reserved names) rejected with plain-language reasons, round-trip import→export is a fixpoint
  2. User receives new INBOX mail via IDLE push (honest Push status in the UI) with advisory wakeups funneled through SyncGate — poll stays as fallback and multi-folder sweeper, never removed
  3. Killing the connection mid-IDLE reconnects (DONE-on-all-exits, 29-min re-IDLE, manager-consistent backoff) with no duplicate sync and no manager-session stall — the watcher never parks on the manager session

**Plans**: 3 plans

- [x] 29-01-PLAN.md — rules_io module + versioned JSON import/export + validation + round-trip fixpoint test (IMP-01 tracer)
- [x] 29-02-PLAN.md — IdleWatcher module + dedicated connection + DONE handling + advisory wakeup via SyncWake→SyncGate + capability gating + fake-server transcript tests (IDLE-01)
- [x] 29-03-PLAN.md — RulesPanel import/export UI + validation preview + Sidebar Push/Polling/Offline status + capability gates (IMP-01 + IDLE-01 UI)

**UI hint**: yes

### Phase 30: Batch Trust Pair

**Goal**: Whole-account batch operations are trustworthy — preview before running, retry only failures after
**Mode**: mvp
**Depends on**: Phase 29 (all triggers funneled through SyncGate with row-level outbox claims)
**Requirements**: BATCH-03, BATCH-04
**Success Criteria** (what must be TRUE):

  1. User can dry-run a batch/archive operation and review the per-item plan (shared matcher, `execute=false`, writes off) with a [Run for real] path
  2. User can retry only the failed items of a batch run with writes re-on for failures — already-executed rows execute exactly once (burst-EXISTS + double-click-retry stress: no double-replay)
  3. Stale previews re-resolve UIDs at confirm time — the confirmed run never files the wrong mail

**Plans**: 2 plans

- [x] 30-01-PLAN.md — shared dry-run preview core (preview_batch/preview_archive, writes off) + stale-preview re-resolution in confirm path (BATCH-03 backend)
- [x] 30-02-PLAN.md — retry-only-failures on same run_id with stress tests + BatchPanel/ArchivePanel preview + retry UI (BATCH-04 + surfaces)

**UI hint**: yes

### Phase 31: Sieve Export + History + Learning v1 + Auto-Confirm

**Goal**: The classifier compounds local value — server-side export, visible history, adaptation from corrections, earned autonomy — with governors attached
**Mode**: mvp
**Depends on**: Phase 29 (versioned rule envelope settles the predicate model the generator consumes) and Phase 28 (override-event dataset exists)
**Requirements**: SIEVE-01, HIST-01, LEARN-01, CONF-01
**Success Criteria** (what must be TRUE):

  1. User can export smart-folder rules + taxonomy filing as RFC 5228 Sieve text (preview, copy, `.sieve` download; golden-file tested, `fileinto`-only subset) — non-expressible predicates appear as comments with a warning count
  2. User can browse override history with per-category stats (the visible control surface and the dataset learning consumes)
  3. App adapts keyword weights + confidence thresholds from explicit override signals only — silence is never approval (training-neutrality tested); adaptation is interpretable and reversible, no fine-tuning runtime
  4. User can enable per-category auto-confirm on earned precision (opt-in per category, default off) with visible precision display — an override auto-disengages it (override-flips-to-manual tested), journaled MOVEs with digest + one-click undo
  5. Local auto-file and Sieve export coexist without flapping — "managed by Sieve" mutual-exclusion badges ride with this phase

**Plans**: 3 plans

- [ ] 31-01-PLAN.md — sieve.rs pure generator + golden fixtures + export_sieve_text + RulesPanel Export-Sieve UI (SIEVE-01)
- [ ] 31-02-PLAN.md — history_list/history_stats + M22 tables + learn.rs learner + HistoryPanel UI (HIST-01 + LEARN-01)
- [ ] 31-03-PLAN.md — precision gate + override-flips-to-manual + journaled auto-confirm digest/undo + Sieve-managed coexistence (CONF-01)
**UI hint**: yes

### Phase 32: Compose Hardening

**Goal**: Compose paths verified end-to-end over the proven v1.2 pipelines — send, drafts, reply, reply-all just work
**Mode**: mvp
**Depends on**: Phase 27 (sandbox pre-flight proven; parallelizable with Phases 28–31 — disjoint files, verify + fix only, never a rebuild)
**Requirements**: COMP-01, COMP-02, COMP-03, COMP-04
**Success Criteria** (what must be TRUE):

  1. User can send a new email end-to-end — queue → flush → arrival + Sent copy, no duplicate (Message-ID dedupe, uncertain-reconcile-not-resend)
  2. User can save and resume drafts with exactly one server copy per session (APPEND-new + expunge-old invariant holds)
  3. User can reply with correct threading headers (`In-Reply-To`, extended `References`, single `Re:`) and quoted body
  4. User can reply-all honoring Reply-To and excluding own identity — expanded-recipient allowlist enforced on the resolved list

**Plans**: TBD
**UI hint**: yes

### Phase 33 (conditional): ManageSieve Upload

**Goal**: Server-side rules activate only where the server speaks ManageSieve — safely or not at all
**Mode**: mvp
**Depends on**: Phase 27 (port-4190/SIEVE probe must succeed — no probe, no phase) and Phase 31 (export text exists to upload)
**Requirements**: SIEVE-02
**Success Criteria** (what must be TRUE):

  1. If the probe succeeded: user uploads Sieve text via ManageSieve (`io-managesieve`, `native-tls` only, `spawn_blocking`-hosted — the sole new dep, added only now) behind backup-then-diff with an explicit append/replace choice — never blind overwrite, never auto-sync; `fileinto` targets validated against the folder tree
  2. If the probe failed: this phase does not execute — export-only stands as the shipped scope with documented deferral, and no uploader code exists in the tree

**Plans**: TBD
**UI hint**: yes

## Progress

**Execution Order:** ... → 20 → 21 → 22 → 23 → 24 → 25 → 26 → 27 → 28 → 29 → 30 → 31 → 33, with 32 parallelizable anytime after 27

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
| 11. Folder CRUD | v1.2 | 3/3 | Complete | 2026-10-06 |
| 12. Drafts | v1.2 | 3/3 | Complete | 2026-10-08 |
| 13. Send Pipeline | v1.2 | 3/3 | Complete | 2026-10-08 |
| 14. Compose UI | v1.2 | 2/5 | Complete | 2026-10-08 |
| 15. Sidecar Packaging Spike | v1.3 | 3/3 | Complete | 2026-10-10 |
| 16. Taxonomy + Store | v1.3 | 3/3 | Complete | 2026-10-10 |
| 17. Classify Engine (No Moves) | v1.3 | 2/2 | Complete | 2026-10-10 |
| 18. Confirm + Trust UX | v1.3 | 2/2 | Complete | 2026-10-10 |
| 19. Taxonomy Editor + Import | v1.3 | 2/2 | Complete | 2026-10-10 |
| 20. Batch Reorganization | v1.3 | 2/2 | Complete | 2026-10-10 |
| 21. SMTP Send — Live Gate | v1.4 | 2/2 | Complete (offline green; live run pending human via scripts/live-21.sh) | 2026-10-13 |
| 22. Draft & Seen Round-Trip — Live Gate | v1.4 | 2/2 | Complete (offline 19/19 green; live run pending human via scripts/live-22.sh) | 2026-10-13 |
| 23. Search + Filters | v1.4 | 3/3 | Complete (345 lib tests green, tsc clean; vite bundle blocked environmentally — Node 18 vs required 20+) | 2026-10-13 |
| 24. Auto-Archive | v1.4 | 2/2 | Complete (472 tests green, tsc clean; scheduler structurally absent) | 2026-10-13 |
| 25. Smart Folder Rules | v1.4 | 2/2 | Complete (392 lib green incl. 37 rule tests, tsc clean) | 2026-10-13 |
| 26. Live Validation Bundle | v1.4 | 2/2 | Complete (offline: bin 15/15, safety 48/48 green; live pending human via scripts/live-26.sh) | 2026-10-13 |
| 27. Live-Validation Gate + Capability Probes | v1.5 | 2/2 | Complete (verified 9/9) | 2026-10-13 |
| 28. Override-Event Log + CONDSTORE Fast Path | v1.5 | 2/2 | Complete (verified) | 2026-10-13 |
| 29. Rule Import/Export + IDLE Watcher | v1.5 | 3/3 | Complete (425 lib green, verified 11/11, review 11/11 fixed) | 2026-10-14 |
| 30. Batch Trust Pair | v1.5 | 2/2 | Complete (434 lib green, verified 7/7) | 2026-10-14 |
| 31. Sieve Export + History + Learning v1 + Auto-Confirm | v1.5 | 0/? | Not started | - |
| 32. Compose Hardening | v1.5 | 0/? | Not started (parallelizable after Phase 27) | - |
| 33. ManageSieve Upload (conditional) | v1.5 | 0/? | Not started (only if Phase 27 port-4190 probe succeeds) | - |
