# Project Research Summary

> v1.2 Compose & Organize — SMTP send + IMAP folder/message CRUD + drafts.
> Synthesized 2026-10-06 from STACK.md, FEATURES.md, ARCHITECTURE.md, PITFALLS.md.
> Baseline: v1.1 read/triage client shipped (headers-first sync, per-folder sync,
> flag_outbox, single-flight SessionManager, SQLite + FTS5, 993/SSL live-verified).
> Servers: IMAP `mail.utfpr.edu.br:993/SSL`, SMTP `smtp.utfpr.edu.br:587/STARTTLS`.

## Key Findings

### 1. SMTP: add `lettre 0.11`, nothing else
- **One-line Cargo delta:** `lettre = "0.11"` (verified current 0.11.23). Default
  features already supply exactly what's needed
  (`smtp-transport`, `pool`, `native-tls`, `hostname`, `builder`).
- Constructor: `SmtpTransport::starttls_relay("smtp.utfpr.edu.br")` — port 587 +
  `Tls::Required` (fail-closed; never `builder_dangerous` / `Tls::Opportunistic`).
- **Sync transport + `spawn_blocking`**, not async lettre: send is seconds-long and
  low-frequency; matches the existing "never block Tauri runtime / IMAP via
  block_on on dedicated thread" discipline and avoids pulling a tokio-executor
  feature surface into the SMTP path. Cache the transport per account for pool reuse.
- **No `mail-builder` / `mail-send` / `async-smtp` / `rustls` / `dkim` feature.**
  lettre's built-in `Message` builder covers plain+HTML, attachments
  (`Attachment::new` / `new_inline`), and reply/forward headers. A second MIME
  builder or TLS stack is pure duplication; `native-tls` + system CA store keeps
  one trust-store story with IMAP.
- SMTP creds reuse the IMAP user+password via the existing keyring path — no new
  storage, one secret per account. SMTP host/port/security are separately
  configurable (UTFPR value as default); username defaults to IMAP username but
  stays overridable (bare-user vs full-address variance). Live 587 probe
  (STARTTLS offer, AUTH mechs, Sent auto-save behavior) is still unverified —
  run lettre's `autoconfigure` example early.

### 2. IMAP: no new crate — extend `SyncSession` + `SessionManager`
- `async-imap 0.11` already exposes every v1.2 verb: `create` / `rename` /
  `delete`, `uid_store` (±`\Deleted`), `expunge` / `uid_expunge`, `uid_copy`,
  `append`. The work is trait + manager surface, not dependencies.
- Add ~7 verbs to `SyncSession` (`create_mailbox`, `rename_mailbox`,
  `delete_mailbox`, `store_deleted`, `expunge`, `uid_expunge`, `copy/move_message`,
  `append_message`) following the `set_seen` template: UID-only addressing,
  drain response streams to completion, canonical `.SILENT` args; extend
  `MockSession` in lockstep.
- `SessionManager` gains lease-scoped methods (`mark_deleted_in`,
  `expunge_in`/`uid_expunge_in`, `move_message_in`, `append_to`,
  `create/rename/delete_mailbox`) with the proven lease → attempt → reconnect +
  retry-once pattern. MOVE holds the lease across its internal double-SELECT so
  no interleaved SELECT can redirect a plain EXPUNGE.
- Capability-gate at connect: prefer `MOVE` (RFC 6851) with COPY+STORE+EXPUNGE
  fallback; prefer `UID EXPUNGE` (UIDPLUS) with plain-`EXPUNGE` fallback.
  Wire names stay raw modified-UTF-7 (`mutf7.rs` reuse, delimiter from LIST);
  NAMESPACE stays banned (parser poison); APPENDUID/MOVE extended responses need
  the same tripwire-test treatment as NAMESPACE.

### 3. Delete = move-to-Trash by default; raw expunge is an anti-feature
- Desktop convention (Thunderbird/Apple/Outlook): Del = COPY/MOVE → Trash, undo
  window (~5–10 s reverse-MOVE), permanent delete only via Shift+Delete /
  Empty Trash (`STORE \Deleted` + `UID EXPUNGE` scoped to Trash).
- Raw EXPUNGE-as-default is irreversible, diverges from every desktop client,
  and plain `EXPUNGE` nukes *all* `\Deleted` in the folder including other
  clients' flags. PROJECT.md "expunge" must be read as expunge-under-the-hood
  with Trash UX on top.
- Trash identity: auto-detect (`Trash`/`Deleted`/`Lixeira`/Gmail path) else CREATE
  once with confirm; needs the P3 special-use/role mapping. Move/copy picker
  excludes `\Noselect`, INBOX, and Sent.

### 4. Store: two new durable tables (M7), `flag_outbox` untouched
- **`send_queue`** (SMTP durability core): MIME rendered once at enqueue to
  `<app_data>/outbox/<id>.eml` (file, not blob) + envelope columns
  (to/cc/bcc JSON, Message-ID-at-enqueue, draft link) + `state`
  (queued|sending|sent|failed, plus `uncertain` after DATA-timeout for
  reconcile-not-resend) + `attempts`/`last_error`/`next_retry_at` backoff.
  Crash recovery: reset `sending`→`queued` at launch (at-least-once contract,
  documented in UI); 25 MB send cap; BCC envelope-only with a unit test.
- **`imap_outbox`** (delete/move queue): `(mailbox_id, uid, op, dest_mailbox,
  uid_validity, attempts, last_error)`, UNIQUE(mailbox_id,uid). Enqueueing a
  delete/move drops the same-key `flag_outbox` row (flag write to a soon-dead
  message is moot). UIDVALIDITY epoch-drop + absent-UID-drop mirror RFC 4549.
- **Do NOT add `op`/`dest` columns to `flag_outbox`** — it changes the Phase 6
  contract under existing tests. Same replay conventions, separate tables;
  `drafts` table is local-first editor backing (`dirty`, `server_uid`,
  attachments JSON) with save = APPEND-new + delete-old.
- Sent filing: APPEND sent bytes verbatim with `\Seen` after SMTP 250 OK; probe
  whether the server auto-saves and skip APPEND if so (same-Message-ID dedupe);
  APPEND failure → `sent-unfiled`, retry APPEND, never re-SMTP-send.

### 5. Precondition fix: unify `start_sync` under manager leases first
- `start_sync` currently opens a fresh `connect_sync` session per pass instead
  of `SessionManager`. With EXPUNGE/MOVE in play that's a correctness hazard
  (sweep SELECTs one folder while a lease EXPUNGEs another). Route sync through
  `manager_for().lease_for(mailbox)` before any destructive op ships.
- Related lease/sync rules: replay `imap_outbox` pre-sweep (before flag replay
  and header sweep) so locally-deleted mail isn't resurrected in the same pass;
  extend the convergence `pending` gate to `imap_outbox` depth (`send_queue`
  stays independent except Sent/Drafts post-send sync); keep the single-session
  invariant — never add a second IMAP connection for moves; chunk multi-message
  moves so poll stays responsive.

### 6. Reply/forward/drafts correctness details
- Threading headers are non-negotiable: `In-Reply-To` = parent Message-ID,
  `References` = parent References + parent Message-ID, single `Re:`/`Fwd:`
  prefix, Reply-To honored, Reply-All minus own identity (needs own-address
  config). Round-trip test: emitted MIME parsed by `mail-parser` threads under
  the original.
- Drafts: explicit Save + 30 s dirty-only autosave; one server copy per compose
  session (track `draft_uid`, replace = APPEND-new + expunge-old — never a new
  copy per tick); send = SMTP OK → delete draft UID → APPEND Sent (orphan
  drafts are the #1 Thunderbird complaint); offline drafts queue like
  send_queue; hard-code the Drafts role (no user-mappable folder paths).

## Implications for Roadmap

Suggested build order (dependency-aware, each step shippable + tested):

1. **Manager unification** — route `start_sync` through `SessionManager` leases.
   Precondition for every destructive op. Gate: Phase 9 suite green +
   single-flight contention test.
2. **IMAP verbs + manager methods + mocks** — trait verbs, BoxedSession impls,
   MockSession arms, reconnect-retry, folder-name quoting, capability helpers,
   parser tripwire. No store/commands yet — pure transport.
3. **Delete/move (`imap_outbox` + worker replay + commands)** — M7-C migration,
   pre-sweep replay order, optimistic local delete + `pending_delete` hidden
   state, attachment-dir + FTS cleanup on confirmed expunge, UIDVALIDITY-gated
   drops. Verify offline-delete → reconnect → server expunged; move lands in
   dest with `\Seen` preserved and no re-FETCH of cached bodies.
4. **Folder CRUD + roles** — CREATE/RENAME/DELETE commands, `mailboxes`
   bookkeeping (roles, delimiter, `\Noselect`/special-use attributes), LIST
   refresh, INBOX guards, sidebar tree. Verify rename preserves UIDs, delete
   cascades, RENAME invalidates selection cache + re-LISTs.
5. **Drafts local + APPEND** — M7-B, editor backed by `drafts`, save→APPEND→
   `server_uid`, edit = resend + delete-old reusing the P4 machinery.
6. **SMTP module + `send_queue` + flush worker** — lettre transport, M7-A,
   `queue_send`/`flush_send_queue`, `SendGate` (separate from `SyncGate`),
   backoff, crash-recovery reset, APPEND-to-Sent, MockSmtp suite (retry,
   permanent-fail, BCC, DATA-timeout→`uncertain`→reconcile). Live-verify
   against `smtp.utfpr.edu.br:587`.
7. **Compose/reply/forward UI + attachments** — MIME render, staged attachments
   with size guard, threading headers, outbox badge, failed-retry UX.
   End-to-end: compose offline → online flush → arrives + Sent copy, no duplicate.

Cross-cutting gates per phase: forward-only migrations each with a
preserve-rows test; error mapping to plain language with no secret leakage;
at least one live gate per phase (587 send, `SGE-TEST-*` folder CRUD,
move/expunge round-trip); multi-client `\Deleted` survival test; send
double-click single-delivery test.

Open probes to settle early (discuss-phase): Trash name + MOVE/UIDPLUS/
SPECIAL-USE advertisement on `mail.utfpr.edu.br` (live LIST/CAPABILITY);
SMTP username format + server Sent auto-save; Junk scope; QUOTA exposure for
large APPENDs.

## Sources

- `STACK.md` — lettre 0.11 decision (0.11.23, 2026-08-03), STARTTLS-relay +
  sync-transport analysis, MIME-builder rejection table, async-imap verb map,
  `SyncSession`/`SessionManager`/`MockSession`/store integration points,
  `uid_expunge` fallback caveat. Verified against crates.io + docs.rs.
- `FEATURES.md` — desktop-client conventions (Thunderbird/Apple Mail/Gmail):
  table-stakes vs differentiators vs anti-features for compose/send,
  folders, delete/move (trash-default decision), drafts; dependency-aware
  build order; open server-capability questions. Grounded in RFC 5322 §3.6,
  RFC 6851, RFC 4315, vendor docs, SGE PROJECT.md + repo layout.
- `ARCHITECTURE.md` — repo-read integration design (v1.1 shipped state,
  SCHEMA_VERSION 6): single-session leases, trait verbs, M7
  send_queue/drafts/imap_outbox DDL, send state machine + BCC/crash rules,
  drafts flow, before→after data flows, new-vs-modified file list,
  8-step build order, EXPUNGE/MOVE/APPENDUID/lettre-runtime risks.
- `PITFALLS.md` — failure catalog for adding send + CRUD to the existing
  system: SMTP auth/STARTTLS/dup-send/queue-divergence/threading/Sent-split-brain/
  attachment-encoding; delimiter/`\Noselect`/special-use/RENAME-race/INBOX;
  expunge blast radius + UIDVALIDITY gating + `\Deleted` visibility + orphans;
  non-atomic MOVE; cross-cutting (resurrection, poll-vs-send, migration
  pile-up, keyring scope, error leakage, live-vs-unit drift); per-phase
  P1–P4 test/live gates. Sources: RFC 3501/4315/6154/6851/5322/3207 + SGE
  manager/outbox/tombstone code facts.
