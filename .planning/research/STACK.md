# STACK — SMTP Send + IMAP Folder/Message CRUD (v1.2 Compose & Organize)

> Research date: 2026-10-06. Verified against crates.io + docs.rs.
> Scope: what to ADD/CHANGE for (1) SMTP send via `smtp.utfpr.edu.br:587/STARTTLS`
> (compose + reply/forward with attachments), (2) IMAP folder CRUD
> (CREATE/RENAME/DELETE), (3) message delete (STORE \Deleted + EXPUNGE) and move
> (COPY + expunge), (4) drafts (APPEND). Existing stack is NOT re-researched.

## TL;DR

| Need | Decision | Crate / API |
|------|----------|-------------|
| SMTP send + STARTTLS | **ADD `lettre 0.11`** (latest 0.11.23, 2026-08-03) with default features | `SmtpTransport::starttls_relay` + `Credentials` |
| MIME building (attachments, reply/forward, drafts) | **Use lettre's built-in `Message` builder** — no extra crate | `Message::builder`, `MultiPart`, `SinglePart`, `Attachment` |
| Folder CRUD, delete/move, APPEND drafts | **NO new crate** — `async-imap 0.11` already has it all | `create/delete/rename`, `uid_store`, `expunge/uid_expunge`, `uid_copy`, `append` |
| TLS backend for SMTP | **native-tls via lettre defaults** (system CA store) | matches existing IMAP TLS story |

`Cargo.toml` delta is **one line**: `lettre = "0.11"`.

---

## 1. ADD: `lettre = "0.11"` (verified current: 0.11.23, 2026-08-03)

**WHY lettre and not alternatives:**
- It is the maintained standard Rust mailer (~19.7M downloads, 459 dependents). The
  question's alternative, `async-smtp`, is an older community crate effectively
  superseded by lettre — adding it would mean betting the send path on a
  stagnant dependency for zero gain.
- Single crate covers **transport + TLS + AUTH + MIME builder**. One dependency,
  one TLS story, one runtime story.
- AUTH mechanisms after STARTTLS: PLAIN, LOGIN, XOAUTH2 (RFC 4954). University
  submission servers expect user+password (PLAIN/LOGIN) — covered with no extra code.
- Has a `cargo run --example autoconfigure SMTP_HOST` probe — useful to fingerprint
  `smtp.utfpr.edu.br` capabilities (STARTTLS offer, AUTH mechs) before writing send code.

**Feature flags — use defaults, add nothing:**
```toml
lettre = "0.11"
```
- Default features = `["smtp-transport", "pool", "native-tls", "hostname", "builder"]`
  — that is exactly the needed set: transport + connection pool + TLS + builder.
- **WHY native-tls (not rustls):** the project already dials IMAP via
  `native-tls 0.2` / `async-native-tls 0.5` against the **system CA store** because
  university certs must validate without custom roots (`imap/session.rs`). Using
  lettre's default `native-tls` keeps ONE TLS backend and ONE trust-store story.
  Adding `rustls` would duplicate TLS stacks and force a `webpki-roots` /
  `rustls-native-certs` decision for no benefit.
- Do NOT enable `dkim` (signing is the *receiving* server's job; a submission
  client relaying via the university smarthost doesn't DKIM-sign), `file-transport`,
  `sendmail-transport`, or `tracing`.

### STARTTLS specifics for `smtp.utfpr.edu.br:587`

- Constructor: `SmtpTransport::starttls_relay("smtp.utfpr.edu.br")`.
  - WHY this and not `relay()`: `relay()` = implicit TLS on port 465 (SMTPS).
    This server is **587/STARTTLS** (explicit upgrade), which is exactly what
    `starttls_relay()` builds: port **587** + `Tls::Required`.
  - WHY `Tls::Required` matters: it **fails closed** if the server doesn't upgrade —
    no credentials or mail bytes ever go out on a downgraded plaintext connection.
    Never use `builder_dangerous` (no TLS) or `Tls::Opportunistic` (MITM-able);
    docs explicitly mark both unsuitable for production.
- Auth: `Credentials::new(username, password)` — same UTFPR user+password as IMAP
  (standard for university submission). Source both from the existing credential
  path (in-memory `active_account` → keyring fallback), never new storage.
- Cert policy: reuse the existing discipline — system-store verification, and the
  `allow_untrusted` flag must stay **refuse-loudly** (`check_cert_policy` pattern).
  No `dangerously_accept_invalid_*` anywhere (keep the grep gate green).

### Sync vs async lettre transport — use SYNC + blocking thread

- lettre offers `SmtpTransport` (blocking) and `AsyncSmtpTransport<Tokio1Executor>`
  (needs `tokio1` executor features; async-std path exists only under rustls-TLS
  features — incompatible with the native-tls decision above).
- **Recommendation: sync `SmtpTransport` called off-runtime**, i.e. inside
  `tauri::async_runtime::spawn_blocking` or the existing dedicated blocking-thread
  pattern (`imap/mod.rs`: IMAP runs via `block_on` on a dedicated thread, *never on
  Tauri tokio runtime threads*).
- WHY: send is a seconds-long, low-frequency operation; blocking one pooled thread
  per send is the established project pattern, avoids pulling tokio-executor feature
  surface into the SMTP path, and keeps the "never block Tauri runtime" invariant
  intact. Connection `pool` (default feature) still gives connection reuse across
  sends if the transport handle is cached per account like `SessionManager`.

## 2. MIME BUILDING: lettre's built-in `Message` builder — do NOT add `mail-builder`

The question names `mail-builder` — **verified: do not add it.**

- `mail-builder` (stalwartlabs, latest **1.0.0**, 2026-09-12) is a builder-only crate
  whose send half is the sibling `mail-send` and whose parse half is the already-used
  `mail-parser`. It is a good crate — but it would be a **second MIME builder**
  duplicating what lettre already ships.
- lettre's `builder` feature (on by default) covers the full v1.2 surface:
  - plain + HTML (`MultiPart::alternative_plain_html` / `alternative`),
  - attachments (`Attachment::new(filename).body(bytes, mime)`),
  - inline images (`Attachment::new_inline(cid)` inside `MultiPart::related`),
  - reply/forward headers (`In-Reply-To`, `References`) via `.header(...)`.
- Draft save = `Message::formatted()` bytes → IMAP `APPEND` with `\Draft` flag
  (§4). Sent-folder copy = same bytes APPENDed with `\Seen` after SMTP accept
  (standard "save sent" behavior; server may also auto-save — check `Sent` after
  first live send and skip double-save if the server does it).
- Reply/forward quoting reuses bodies already in SQLite (`BODY.PEEK[]` cache) —
  no new fetch path needed; parse with existing `mail-parser 0.11`.

## 3. IMAP FOLDER/MESSAGE CRUD: no new crate — extend `SyncSession` + `SessionManager`

Verified against `async-imap` latest docs: `Session` already exposes **every verb
needed**. The work is trait + manager surface, not dependencies.

| v1.2 operation | async-imap 0.11 API (already in tree) | Notes |
|----------------|----------------------------------------|-------|
| Folder create | `session.create(name)` | wire name raw (UTF-7), display decoded via existing `mutf7` |
| Folder rename | `session.rename(from, to)` | server moves subscriptions/children per RFC 3501; re-LIST after |
| Folder delete | `session.delete(name)` | fails on non-empty on some servers → UX: offer move-out first |
| Delete message | `uid_store(uid, "+FLAGS.SILENT (\\Deleted)")` + `expunge()` | prefer `uid_expunge(uid_set)` where server advertises UIDPLUS — selective, no collateral |
| Move message | `uid_copy(uid, dest)` + delete-source + expunge | COPY first, verify OK, then flag source; never flag-first |
| Save/edit draft | `session.append(folder, flags, internaldate, content)` | flags `Some("(\\Draft)")`; edit = APPEND new + expunge old |

**Integration points (concrete):**
1. **`imap/mod.rs` `SyncSession` trait** — add `create_mailbox / rename_mailbox /
   delete_mailbox / set_deleted / expunge / uid_copy / append` following the exact
   `set_seen` template: UID-only addressing (`u32` formatted by caller, never parsed
   from input), drain the returned response stream to completion (the dropped-stream
   aborts-write lesson from `set_seen`), `.SILENT` flag args via a canonical-arg
   helper like `seen_store_arg`.
2. **`imap/manager.rs` `SessionManager`** — add `create_folder_in / delete_message_in /
   move_message_in / append_draft_in` methods reusing the proven pattern: `lease_for`
   (single-flight mutex + SELECT) with **one transparent reconnect + retry** on
   failure. Folder ops go through a lease too (serializes against concurrent sync).
3. **`MockSession`** (`sync/worker.rs` tests) — extend with the new verbs so the
   existing mock-based gate style carries over (cf. `set_seen_store_arg_spelling`).
4. **Store (`store/`)** — Phase 7 already has per-folder sync + `STATUS UNSEEN`.
   Delete/move need tombstone + UIDVALIDITY-guarded reconciliation (same pattern as
   Phase 9 backfill: range-diff, no silent gaps). Drafts need a local drafts table
   row ↔ server UID binding (APPEND returns UIDPLUS UIDVALIDITY/UID on capable
   servers; fall back to re-search by `Message-ID` header).
5. **NAMESPACE tripwire stays** — folder CRUD uses raw names from `LIST` (already
   modified-UTF-7 on the wire, decoded only for display). Do NOT issue NAMESPACE
   (`namespace_response_is_unparseable` test documents the parser poison).

**What stays untouched:** `async-imap 0.11`, `async-native-tls 0.5`, `async-std`,
`mail-parser 0.11 + full_encoding`, `rusqlite 0.37 + rusqlite_migration 2.x`,
`keyring 3 + sync-secret-service`, `ammonia 4` (sanitize quoted HTML on reply/
forward the same as the reader), `zeroize` (SMTP password handling).

## 4. Drafts detail (APPEND)

- `append()` signature in 0.11: `append(mailbox, flags: Option<&str>,
  internaldate: Option<&str>, content)`. Save: `flags = Some("(\\Draft)")`.
- Content = `Message::formatted()` bytes (the exact bytes SMTP would send) — so a
  draft promoted to "send" needs no rebuild; send `formatted()` verbatim via
  `send_raw` or rebuild the same `Message`.
- Edit = APPEND replacement + expunge original (IMAP has no in-place edit).
  Keep the local row's stable ID across the swap so the UI doesn't flicker.
- Internaldate: pass `None` (server assigns) unless preserving original date on
  a move/copy-ish path.

## 5. Explicitly NOT adding

| Rejected | WHY |
|----------|-----|
| `async-smtp` | Older crate superseded by lettre; zero capability gain, stale maintenance |
| `mail-builder` / `mail-send` | Second MIME builder + second SMTP stack duplicating lettre's built-ins; `mail-parser` half already in use stays |
| `rustls` / `webpki-roots` | Would fork the TLS story; native-tls + system store already validates university certs |
| lettre `dkim` feature | Submission clients don't DKIM-sign; server-side concern |
| `imap 2.x` sync crate | Documented fallback only if 0.11 bridging hurts; CRUD needs no new transport |
| OAuth/XOAUTH2 flow crates | User+password auth only (M1 constraint carries over); lettre already speaks the mech if ever needed |
| New credential storage | SMTP reuses IMAP user+password via existing keyring path |

## 6. New risks / open probes

1. **Live SMTP unverified** — same posture as IMAP STARTTLS in M1: `smtp.utfpr.edu.br:587`
   needs a live handshake probe (lettre `autoconfigure` example) to confirm STARTTLS
   offer + AUTH mechs + Sent auto-save behavior. Unit-test the builder + a replay
   SMTP stub; mark live send deferred like prior phases did.
2. **SMTP host configurability** — don't hardcode `smtp.utfpr.edu.br`. Derive default
   from IMAP host? No — separate SMTP host/port/security fields with the UTFPR
   value as default (mirrors `SecurityMode` configurability on IMAP).
3. **`uid_expunge` support** — gated on UIDPLUS capability; probe CAPABILITY after
   login and fall back to full `expunge()` (document the collateral-expunge caveat:
   full EXPUNGE removes *all* \Deleted in the selected folder, including other
   clients' — prefer UIDPLUS path when advertised).
