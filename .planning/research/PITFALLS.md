# PITFALLS — Adding SMTP Send + Folder/Message CRUD to SGE (v1.2)

> Research for the **v1.2 Compose & Organize** milestone. SGE already has a working
> read/triage client (headers-first sync, per-folder sync, `flag_outbox`, single-flight
> `SessionManager`, SQLite + FTS5, 993/SSL live-verified). This doc lists **common
> mistakes when ADDING send + CRUD to that existing system** — not how to build a
> client from scratch. Each pitfall: what goes wrong → warning signs → prevention →
> which v1.2 phase should own it.

**Suggested v1.2 phase split** (referenced as P1–P4 below):

| Phase | Scope | Why this order |
|-------|-------|----------------|
| **P1 — SMTP transport** | Auth + STARTTLS + send-one-mail path, no queue, no threading | Unblocks everything; smallest live-testable slice against `smtp.utfpr.edu.br:587` |
| **P2 — Compose safety** | Offline send queue, exactly-once, reply/forward threading headers, attachments | Hardest correctness surface; builds on P1 transport |
| **P3 — Folder CRUD** | CREATE / RENAME / DELETE with delimiter, `\Noselect`, special-use | Independent of send; touches folder-discovery + sidebar tree |
| **P4 — Delete / Move** | `\Deleted` + EXPUNGE/UID EXPUNGE, MOVE (= COPY + STORE), UIDVALIDITY-gated local deletes | Most destructive; must come after sync/backfill semantics are solid (they are — Phases 6–9) |

Rule of thumb: **P1 before P2, P3 and P4 in either order, never P4 before a live UIDVALIDITY test.**

---

## 1. SMTP SEND — Auth, STARTTLS, Queue, Duplicates, Threading

### 1.1 Reusing the IMAP credential path blindly for SMTP

SMTP auth on port 587 is a *different* handshake (EHLO → STARTTLS → EHLO → AUTH)
against a *different* host (`smtp.utfpr.edu.br`), and servers commonly advertise a
different auth mechanism set (PLAIN vs LOGIN) or require the full address as username
where IMAP accepts the short login. Copy-pasting the IMAP `connect_sync` config
produces "auth works for reading, fails for sending" bugs that look like password
problems.

- **Warning signs:** login succeeds, send fails with `535` / `530 Authentication required`;
  works on one provider, fails on UTFPR; LOGIN-vs-PLAIN confusion in logs.
- **Prevention:**
  - Separate `SmtpConfig { host, port, username, password, security }` struct — never
    reuse `AccountConfig` verbatim. Reuse only the *keyring lookup*, not the connection
    parameters.
  - Prefer a dedicated SMTP crate (`lettre` is the Rust standard) over hand-rolling
    SMTP on a raw TLS stream — it already handles EHLO capability parsing + mechanism
    fallback.
  - Log the server's EHLO capability list + chosen mechanism (never the secret) at send
    time; this turns "auth failed" into a 30-second diagnosis.
  - Live-test P1 against the real UTFPR 587 endpoint early; STARTTLS is currently
    unit-tested only (known residual risk from M1).
- **Phase:** **P1** (blocks all send work).

### 1.2 STARTTLS downgrade / cert-validation gaps

Two classic failures: (a) silently falling back to plaintext when STARTTLS fails
(credential leak on port 587), (b) accepting any certificate to "make it work in dev"
and shipping that. Both are invisible until a network attacker or a cert rotation
exposes them.

- **Warning signs:** a `TLS optional / opportunistic` flag anywhere; `danger_accept_invalid_certs`
  or equivalent in non-test code; no test that asserts plaintext refusal.
- **Prevention:**
  - Enforce **required** STARTTLS on 587 (fail closed, surface a clear error).
    Only 465/SMTPS uses implicit TLS — don't conflate the two code paths.
  - Keep the existing configurable-security pattern from IMAP (SSL-TLS vs STARTTLS
    per-account) but default SMTP to `STARTTLS-required`.
  - Pin a negative test: connection to a non-TLS fake server must fail, not downgrade.
  - Reuse the IMAP redaction discipline (`Debug` redacts secrets, no password in logs —
    see `manager.rs` password discipline) for the SMTP path.
- **Phase:** **P1**.

### 1.3 Duplicate sends on retry (the most user-visible send bug)

SMTP has no idempotency key. If the client times out *after* the server accepted the
message (or after DATA but before the `250 OK` is read), a naive retry sends the mail
twice. Users forgive a slow send; they do not forgive double-sending a job application.

- **Warning signs:** retry loop around the whole `send()` call; no `Message-ID` generated
  client-side; timeout values copied from IMAP FETCH (too short for large attachments
  on DATA); "user clicked Send twice" reports.
- **Prevention:**
  - Generate `Message-ID` client-side (UUID + domain) and **persist the queued message
    with that ID before first attempt**; dedupe the outbox on `Message-ID`.
  - Retry only on transport errors *before* DATA acceptance is ambiguous; after a
    timeout during/after DATA, mark `state=uncertain` and **reconcile instead of
    blind-resending**: check Sent (via IMAP APPEND-confirm or server Sent copy) for
    the `Message-ID` before re-queueing.
  - Disable the Send button + single-flight the send command (same pattern as
    `SessionManager` single-flight) so double-click ≠ double-send.
  - Generous DATA-phase timeout (attachments upload slowly on 587); short EHLO/AUTH
    timeouts are fine, DATA timeout must scale with size.
- **Phase:** **P2** (queue design), with the single-flight guard in **P1**.

### 1.4 Offline send queue that diverges from the `flag_outbox` pattern

v1.1 already solved durable offline replay for flags (`flag_outbox`: UNIQUE collapse,
attempts/last_error, UIDVALIDITY-gated drops). The mistake is building the send queue
as a second, inconsistent mechanism (in-memory Vec, different retry/drop rules),
so offline-send behaves differently from offline-flag and the two can deadlock or
double-apply after reconnect.

- **Warning signs:** new queue table without `attempts`/`last_error`/`created_at`;
  no drop rule defined; queue survives app restart but flags don't (or vice versa);
  sync worker and send worker both opening write transactions independently.
- **Prevention:**
  - Model `send_queue` explicitly on the `flag_outbox` schema shape
    (`id, message_id UNIQUE, state queued|sending|sent|failed|uncertain, attempts,
    last_error, created_at`) and reuse the same replay-engine conventions.
  - One writer at a time: route send-queue flush through the same single-writer
    discipline as the store (`Arc<Mutex<Store>>` — never hold across `.await`).
  - Cap attempts with backoff, then park as `failed` with a user-visible retry —
    never infinite-loop a 587 outage (same lesson as `fetch_tombstones` strikes
    in Phase 9).
  - Migration as M7 (forward-only, preserving rows — follow the M2→M6 pattern, with a
    test like `m2_upgrades_v1_database_forward_preserving_rows`).
- **Phase:** **P2**.

### 1.5 Broken reply threading (`In-Reply-To` / `References` / `Subject` Re:)

Replies that start a new thread (missing/wrong headers) or nest 10-deep quote pyramids
are the #1 "send works but looks broken" complaint, and Gmail-style threading makes it
worse because the UI groups by these headers.

- **Warning signs:** composing replies by concatenating strings instead of building
  headers from the original message record; `References` truncated to one ID;
  `Subject` gaining multiple `Re:` prefixes; forwarded attachments duplicated inline.
- **Prevention:**
  - Build replies from the **local DB record** (which has the parse-verified headers
    from `mail-parser`): `In-Reply-To = original Message-ID`; `References =
    original.References + original.Message-ID` (cap length, keep first + last N);
    `Subject = Re: <orig>` only if not already prefixed (case-insensitive, single `Re:`).
  - For forwards: new `Message-ID`, no `In-Reply-To`, `Subject = Fwd:`, attachments
    re-attached from the attachment store path (`attachments/<uid_validity>/<uid>/`),
    not re-downloaded.
  - Round-trip test: reply → parse the emitted MIME with `mail-parser` → assert headers
    thread under the original. Add a UI test that a reply appears in the same thread.
- **Phase:** **P2**.

### 1.6 Sent-mail split brain (SMTP send vs IMAP Sent folder)

Sending via SMTP does not put a copy in Sent — the client must APPEND it (or rely on
the server's auto-save, which UTFPR may or may not do). Doing both creates duplicates;
doing neither creates "I sent it but it's gone" panic. This is an *integration*
pitfall: P1 transport works, but the read-side Sent folder (Phase 7) disagrees.

- **Warning signs:** Sent folder empty after sending; or exactly two copies per send;
  per-folder `uid_next` for Sent jumping unexpectedly; STATUS UNSEEN badge wrong on Sent.
- **Prevention:**
  - Decide one strategy and feature-detect: probe whether the server auto-saves
    (send a test mail, SEARCH Sent for the `Message-ID`); if yes, skip APPEND; if no,
    APPEND with the same `Message-ID` so a later sync dedupes rather than duplicates.
  - APPEND **after** SMTP `250 OK`, with the sent bytes verbatim (preserves threading
    headers + attachments); on APPEND failure, keep the queued record as
    `sent-unfiled` and retry APPEND separately — never re-SMTP-send to fix a filing
    failure (see 1.3).
  - Update the local Sent mailbox optimistically but mark it `pending-append` so the
    next per-folder sync reconciles instead of duplicating.
- **Phase:** **P2** (needs P1 transport + Phase 7 Sent sync).

### 1.7 Attachment encoding regressions on the send path

The read path uses `mail-parser 0.11 + full_encoding` for *decoding*. The send path
needs correct *encoding* (MIME multipart/mixed, base64, RFC 2231 filenames with
accents — the codebase already handles accented folder names, expect accented
filenames too). Wrong `Content-Transfer-Encoding` or a missing boundary corrupts
attachments only for the recipient, so the sender never sees the bug.

- **Warning signs:** attachments hand-built with string templates; non-ASCII filenames
  untested; no size guard before DATA (587 servers often cap at 10–25 MB).
- **Prevention:**
  - Use a MIME builder crate (e.g. `lettre::message` / `mail-builder`), not format!.
  - Pre-send size check with a clear error ("attachment exceeds ~X MB"); stream large
    files, don't buffer whole attachments in memory (read side already caps body cache
    at 256 KiB — apply the same memory discipline).
  - Test matrix: ASCII + accented filename, small + multi-MB, reply-with-attachments
    vs forward-with-attachments; verify by parsing the emitted bytes.
- **Phase:** **P2**.

---

## 2. IMAP FOLDER CRUD — CREATE / RENAME / DELETE

### 2.1 Hardcoding the hierarchy delimiter

The codebase already stores `delimiter` per mailbox (M6) — the pitfall is ignoring it
on the write path: `CREATE "Archive/2024"` breaks on servers using `.` (Courier,
UTFPR may use `.`), and splitting display names on `/` corrupts the sidebar tree for
those servers.

- **Warning signs:** literal `"/"` or `"."` in CREATE/RENAME arguments; client-side
  `name.split('/')` for nesting; tests only covering one delimiter.
- **Prevention:**
  - Always read the delimiter from LIST response / local `mailboxes.delimiter` for the
    *parent*; never assume. Empty delimiter = flat, no nesting UI.
  - Encode folder names: MUTF-7 encode segments (the codebase has `imap/mutf7.rs` —
    reuse it for CREATE/RENAME args, not just display), join with the real delimiter.
  - Parametrize folder-CRUD tests over at least `/` and `.` delimiters.
- **Phase:** **P3** (first folder-write work).

### 2.2 Creating / renaming under `\Noselect` parents (or deleting one)

`\Noselect` mailboxes are hierarchy placeholders — you can't SELECT or APPEND them,
but you *can* create children under them. Clients fail two ways: refusing to create
anything under a `\Noselect` node (missing feature), or trying to SELECT/APPEND/move
mail *into* it (server `NO`s, confusing error). Deleting a `\Noselect` parent with
children has server-dependent semantics (some refuse while children exist).

- **Warning signs:** folder tree treats every LIST entry as selectable; no `\Noselect`
  flag stored locally; CREATE failures surfaced as raw `NO` without explanation.
- **Prevention:**
  - Store `attributes` (`\Noselect`, `\HasChildren`, `\HasNoChildren`) alongside the
    delimiter at discovery time (extend the M6 column family — new migration).
  - UI: `\Noselect` nodes render as expand-only (greyed, no message list, no "move
    here" target); CREATE-child allowed; DELETE on a node with children requires
    explicit confirmation + pre-check via LIST.
  - Handle both LIST dialects: classic `\Noselect` and LIST-EXTENDED `CHILDREN`
    (`\HasChildren`) — UTFPR capability matrix is still unverified (see STATE.md
    blockers), so probe at runtime and degrade gracefully.
- **Phase:** **P3**.

### 2.3 Special-use folders (Sent/Drafts/Trash/Junk) treated as ordinary folders

RFC 6154 `\Sent \Drafts \Trash \Junk` folders have client-visible roles: deleting the
folder mapped as Trash orphans the delete flow; renaming `Sent` breaks the Sent-append
logic (1.6); creating a second `Trash` confuses automated filing. Servers may also
refuse to delete special-use mailboxes outright.

- **Warning signs:** folder DELETE enabled uniformly with no role check; no
  SPECIAL-USE detection at LIST time; Sent/Drafts mapping hardcoded to English names.
- **Prevention:**
  - Detect via `LIST ... RETURN (SPECIAL-USE)` when advertised, fall back to
    well-known-name heuristics *recorded as heuristic* (re-check each discovery).
  - Protect mapped roles in UI: DELETE/RENAME on a special-use folder requires
    explicit role-remap (or refusal with explanation); never silently orphan.
  - The v1.1 Sent/Drafts browsing work (Phase 7) must gain a `role` column — plan the
    migration in P3, not as an afterthought in P4.
- **Phase:** **P3**.

### 2.4 RENAME races and stale-selection writes

RENAME is effectively atomic server-side, but the client's *other* state isn't: an
in-flight sync lease SELECTed on the old name, a pending `flag_outbox` row keyed by
old `mailbox_id`/name, or a poll tick can write flags, APPEND, or sync against a
mailbox that no longer exists under that name. async-imap's single session + SGE's
single-flight lease serializes commands, but a RENAME issued from webmail (or a second
client) bypasses the lease entirely.

- **Warning signs:** RENAME implemented as a bare command with no local invalidation;
  `selected_mailbox` cache not cleared after RENAME; outbox rows surviving a rename;
  sync worker 404ing (`NO Mailbox does not exist`) in a loop after rename.
- **Prevention:**
  - After local RENAME: drop the `SessionManager` selection cache (same as `reconnect()`),
    update the local `mailboxes` row atomically with any `flag_outbox` rows
    (single SQLite transaction), re-run LIST + STATUS to confirm.
  - Handle remote renames: treat persistent `NO` on a previously-good mailbox as
    "re-discover folders" signal (LIST diff → match by UIDVALIDITY, not name), not
    as a fatal sync error. Never auto-CREATE a replacement — that resurrects deleted
    folders.
  - Serialize RENAME against sync: take the same single-flight discipline (no sync pass
    mid-rename); hold the Store mutex only for the DB transaction, never across `.await`.
- **Phase:** **P3**, with the remote-rename detector owned jointly with **P4** sync.

### 2.5 Case-sensitivity and INBOX special-casing

`INBOX` is case-insensitive and special (CREATE must not recreate it; some servers
treat `INBOX.Children` differently). Custom folders are typically case-preserving.
Bugs: offering "Delete INBOX", creating `inbox` as a separate folder, or RENAMEing
`INBOX` (servers refuse, but the local tree may already have optimistically renamed it).

- **Warning signs:** INBOX appears in the DELETE/RENAME menu; optimistic rename applied
  before server OK; duplicate `INBOX` + `inbox` rows in local DB.
- **Prevention:** hard-guard INBOX in UI + command layer (no RENAME/DELETE, ever);
  compare case-insensitively for INBOX only; roll back optimistic folder-tree changes
  on server `NO`.
- **Phase:** **P3**.

---

## 3. EXPUNGE SEMANTICS — `\Deleted`, EXPUNGE vs UID EXPUNGE, UIDVALIDITY

### 3.1 Treating `\Deleted` + EXPUNGE as one step (deleting other clients' mail)

Plain `EXPUNGE` permanently removes **all** messages with `\Deleted` in the selected
mailbox — including ones flagged by another client or webmail session. The naive
delete flow (STORE `\Deleted` → EXPUNGE) can nuke mail the user never touched.

- **Warning signs:** `expunge()` with no UID set; delete flow tested only single-client;
  no UID EXPUNGE capability check.
- **Prevention:**
  - Prefer `UID EXPUNGE <uidset>` (RFC 4315) when advertised: removes only the UIDs
    this client flagged. Capability-probe at connect; fall back to COPY-to-Trash +
    STORE + EXPUNGE only with explicit user confirmation that other `\Deleted` mail
    may also vanish (or better: move-to-Trash as the default delete, expunge Trash
    only on "empty trash").
  - Default delete UX = **move to Trash** (P4 MOVE), with expunge reserved for
    explicit empty-trash / permanent-delete actions.
- **Phase:** **P4**.

### 3.2 Expunging without a UIDVALIDITY gate (deleting the wrong mail)

UIDs are valid only within a UIDVALIDITY epoch. If UIDVALIDITY changed (mailbox
recreated) between sync and delete, the stored UID may now point at a *different*
message. STORE + EXPUNGE without re-validating UIDVALIDITY deletes a stranger's mail
and the local DB happily reports success.

- **Warning signs:** delete command taking only `(mailbox, uid)` with no validity
  check; SELECT summary's `uid_validity` ignored; Phase 9 backfill/tombstone logic
  bypassed on the write path.
- **Prevention:**
  - Gate every destructive op: SELECT → compare `uid_validity` against local
    `mailboxes.uid_validity` → on mismatch, **abort the op, resync the mailbox, and
    tell the user** (same drop-and-resync rule as the `flag_outbox` replay engine:
    whole-mailbox queue drops on UIDVALIDITY bump — extend that rule to delete/move).
  - Carry `uid_validity` in the delete/move outbox rows (like `flag_outbox` already
    does for flags) and validate at flush time, not just enqueue time.
  - Unit-test the mismatch path: validity bump between enqueue and flush must drop,
    never execute.
- **Phase:** **P4** (builds directly on the Phase 6/9 validity infrastructure).

### 3.3 `\Deleted` visibility confusion (user deletes, mail "comes back")

Until EXPUNGE, `\Deleted` messages still exist: a re-sync that doesn't filter them
re-displays "deleted" mail, and the FTS index keeps returning it in search. Users read
this as data loss / broken delete. Conversely, hiding `\Deleted` locally before server
confirmation makes a failed STORE look like a successful delete.

- **Warning signs:** sync upsert unconditionally reviving locally-hidden rows; FTS
  results including pending-delete messages; no `pending_delete` / tombstone state
  in the local model.
- **Prevention:**
  - Optimistic UI: mark `pending_delete` locally, hide from list/search immediately,
    but keep the row until server confirms; on STORE/EXPUNGE failure, un-hide with an
    error (same optimistic-with-rollback pattern as Phase 6 Seen flags).
  - Sync filter: exclude locally-pending-delete UIDs from list/search results even if
    the server still returns them; reconcile on next sync (server EXPUNGE → drop row +
    FTS entry; server still has it + op failed → un-hide).
  - Purge FTS entries on confirmed expunge (the `msg_ad` trigger path) — test that
    expunged subjects no longer match search.
- **Phase:** **P4**.

### 3.4 Attachment orphans and FTS ghosts after expunge

Bodies live in SQLite, attachment bytes on disk
(`attachments/<uid_validity>/<uid>/`). A delete that removes the DB row but not the
disk directory leaks storage silently; a delete that removes the row but leaves FTS
entries returns phantom search hits.

- **Warning signs:** no cleanup step for attachment dirs on expunge; FTS delete path
  untested; `VACUUM`/WAL growth unmonitored.
- **Prevention:** confirmed-expunge = one transaction deleting message + body +
  attachment-part rows, then filesystem removal of the UID dir (scoped by
  uid_validity, so a validity-recycled UID can never delete a new message's files),
  then FTS verification query. Test with a message that has attachments.
- **Phase:** **P4**.

---

## 4. MOVE — COPY + STORE + EXPUNGE "Atomicity" (There Is None by Default)

### 4.1 Assuming MOVE is atomic (message loss and duplication)

Without the MOVE extension (RFC 6851), "move" is three separate commands —
`UID COPY → UID STORE +FLAGS \Deleted → UID EXPUNGE` — with a crash/reconnect window
between each. Crash after COPY = duplicate (in both folders); crash after STORE but
before EXPUNGE = ghost in source; retry of a half-completed move compounds both.

- **Warning signs:** move implemented as fire-and-forget command trio; no persisted
  move intent; retry re-issues COPY unconditionally.
- **Prevention:**
  - Prefer server `MOVE` (UID MOVE) when advertised — single command, no window.
    Probe capabilities; use MOVE path vs emulated path explicitly and log which.
  - Emulated path must be a **state machine persisted in SQLite**
    (`move_outbox`: `copied → flagged → expunged`, with source/target
    mailbox+uid+validity), so a restart resumes instead of restarting. Idempotent
    steps: before COPY, UID SEARCH target for a message with the same `Message-ID`
    (COPY succeeded pre-crash → skip to flag step).
  - Order matters: COPY first, verify present at destination, *then* flag+expunge
    source. Never flag source before confirming the copy.
- **Phase:** **P4**.

### 4.2 MOVE breaking threading, flags, and local identity

COPY preserves the message (new UID in target) but the local DB row is keyed by
(source mailbox, UID). Naive moves lose `\Seen`, lose the body/attachment linkage,
break reply threading (draft replies reference the old UID), and confuse the
backfiller (source UID vanishes → tombstone strikes? target UID appears → re-fetch?).

- **Warning signs:** move deletes the local row and re-fetches full body from server;
  flags reset to unread after move; drafts/replies pointing at moved messages break;
  Phase 9 sweeper treating move-target arrivals as gaps.
- **Prevention:**
  - Preserve on move: carry over flags (`\Seen` at minimum), reuse the cached body
    bytes (don't re-FETCH what you already have — bodies are mailbox-independent),
    retarget attachment dir references or move the directory to the new
    `(validity, new_uid)` path only after confirming the new UID.
  - Keep a `moved_from (old_mailbox, old_uid)` trace until the next successful sync
    of both folders converges; resolve pending draft references by `Message-ID`,
    not by UID.
  - Backfill awareness: source-folder disappearance after a confirmed move is
    expected, not a gap — exempt in-flight move UIDs from tombstone strikes.
- **Phase:** **P4**.

### 4.3 MOVE into `\Noselect` / special-use / wrong-delimiter targets

The move destination picker reuses the folder tree — inheriting every P3 bug (2.1,
2.2): moves into `\Noselect` fail opaquely, moves into Trash vs permanent-delete
semantics confuse, cross-delimiter names corrupt.

- **Warning signs:** destination list = raw LIST output with no filtering; no
  Trash-vs-expunge policy; move-to-Sent allowed.
- **Prevention:** destination picker excludes `\Noselect`, INBOX-except, and Sent
  (unless explicitly refiling); default delete = MOVE to Trash role folder (from P3
  special-use mapping); validate target delimiter before issuing COPY/MOVE.
- **Phase:** **P4** (depends on P3 folder metadata).

### 4.4 Single-session lease vs concurrent move + sync + send

SGE's `SessionManager` serializes everything through one session/lease — moves,
flag writes, sync passes, and folder ops all contend. A long MOVE (large mailbox,
slow server) can starve the 5-min poll or block the UI-perceived send; worse, adding a
*second* IMAP connection for moves (to "fix" the blocking) breaks the single-flight
invariant and interleaves SELECT/EXPUNGE across connections (the exact bug the manager
was built to prevent).

- **Warning signs:** proposals for a second IMAP connection; move command holding the
  lease across large data transfer; poll timer firing mid-move and queueing a sync
  that SELECTs another folder halfway through the COPY→EXPUNGE sequence.
- **Prevention:**
  - Keep the single-session invariant. Route MOVE through `lease_for()` like
    `set_seen_in` (reconnect + retry once, re-SELECT target mailbox).
  - Chunk multi-message moves (bounded UID sets per command) so the poll/single-flight
    queue stays responsive; surface move progress in UI rather than blocking.
  - Never hold the Store mutex across `.await` (existing invariant) — stage move
    state transitions as short transactions between network steps.
- **Phase:** **P4** (integration hardening after basic move works).

---

## 5. Cross-Cutting Integration Pitfalls (New Features vs Existing System)

| # | Pitfall | Warning sign | Prevention | Phase |
|---|---------|--------------|------------|-------|
| 5.1 | **Sync resurrection**: next poll re-fetches moved/expunged mail as "new" | `UIDNEXT`/exists jumps after move; deleted mail reappears | Update local sync state (`uid_next`, tombstones) in the same transaction as the confirmed move/expunge; exempt in-flight op UIDs from backfill | P4 |
| 5.2 | **Poll-vs-send race**: 5-min poll SELECTs mid-send-append | Sent APPEND lands but sync misses it / duplicates it | Single-flight all IMAP through the manager; dedupe Sent arrivals by `Message-ID` | P2 + P4 |
| 5.3 | **Schema migration pile-up**: M7+ breaking forward-only chain | New tables added without M-test; downgrade attempted | One migration per feature table (M7 send_queue, M8 move_outbox, M9 folder attrs/roles), each with a preserve-rows test following the M2–M6 pattern | P2, P3, P4 each |
| 5.4 | **Keyring scope creep**: SMTP password stored as second secret, logout leaves one | Two keyring entries, only one cleared on logout/account-switch | Store one secret per account, reuse for both protocols; account-switch replaces the cached `SessionManager` *and* SMTP transport together | P1 |
| 5.5 | **Error-message leakage**: raw SMTP/IMAP `NO` surfaced to users | "NO [ALREADYEXISTS]" dialogs; passwords in error strings | Map server errors to plain-language messages (follow the `__TAURI_INTERNALS__` guard precedent); assert no secret material in any error path | Each phase |
| 5.6 | **Live-vs-unit drift**: send/folder/delete verified only against mocks | All-green suite, first real UTFPR run fails | Each phase needs at least one live gate against `mail.utfpr.edu.br` (587 SMTP, folder CRUD on a scratch prefix like `SGE-TEST-*`, move/expunge round-trip); keep the Phase 6–9 pattern of documenting deferred live items explicitly | P1–P4 |
| 5.7 | **Drafts as second-class moves**: save/edit draft re-implements APPEND+DELETE badly | Draft edits creating duplicate drafts; offline draft edits lost | Treat draft-save as APPEND-new + (UIDVALIDITY-gated) delete-old — i.e. reuse the P4 move machinery, not a bespoke path; drafts queue offline like send_queue | P2 (design), P4 (reuse) |

---

## 6. Suggested Minimum Test/Live Gates per Phase

- **P1:** EHLO capability log redacted; STARTTLS-required negative test; one live mail
  delivered to self via UTFPR 587; double-click Send test (single delivery).
- **P2:** `Message-ID` dedupe test; timeout-during-DATA → `uncertain` → reconcile test
  (no auto-duplicate); reply/forward header round-trip through `mail-parser`;
  offline-compose → restart → flush test; Sent APPEND-or-autosave probe documented.
- **P3:** delimiter-parametrized CREATE/RENAME/DELETE tests (`/`, `.`); `\Noselect`
  render + create-child-only test; special-use protection test; INBOX guard test;
  live scratch-folder CRUD + rename-while-sync race test; post-rename LIST re-discovery.
- **P4:** UID EXPUNGE-vs-EXPUNGE capability test; UIDVALIDITY-bump-aborts-delete test;
  `\Deleted`-hidden-but-recoverable test; FTS + attachment-dir cleanup test;
  crash-between-COPY-and-EXPUNGE resume test (emulated path); multi-client `\Deleted`
  test (other client's flag survives my delete).

---

*Sources: RFC 3501 (IMAP4rev1) § CREATE/RENAME/DELETE/EXPUNGE/COPY/UID EXPUNGE notes,
RFC 4315 (UIDPLUS/UID EXPUNGE), RFC 6154 (SPECIAL-USE), RFC 6851 (MOVE), RFC 5322
§ In-Reply-To/References threading, RFC 3207 (STARTTLS); SGE codebase facts:
`SessionManager` single-flight (`imap/manager.rs`), `flag_outbox` replay rules +
`M2_FLAG_OUTBOX_SQL`, per-folder sync + delimiter M6 + STATUS/SYNC timestamp split
M5 (`store/mod.rs`), attachment dir layout, tombstone sweeper (Phase 9). UTFPR server
capabilities (CONDSTORE, SPECIAL-USE/LIST-EXTENDED, UIDPLUS/MOVE advertisement,
587 auth mechanisms) remain unverified — probe live in P1/P3 before committing to
code paths that assume them.*
