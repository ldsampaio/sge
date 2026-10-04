---
phase: 04
slug: reader-attachments
status: approved
mode: mvp
created: 2026-10-03
---

# Phase 4 — Reader + Attachments

## Goal

User reads full messages safely and saves attachments to disk.

## Context

Phase 3 delivered a three-pane mailbox UI reading from the local SQLite store
(headers-only sync — Phase 2 stores message envelopes, not bodies). Phase 4 adds
the message reader: fetching full RFC822 content on demand, rendering sanitized
HTML, and handling attachments.

### Key codebase facts

- `src-tauri/src/sync/bodies.rs` — already implements `sanitize_html()` (ammonia),
  `fetch_body_message()` (BODY.PEEk[] → parse → sanitize → cache), but is **not
  declared as a module** in `sync/mod.rs` (dead code). Must be wired up.
- `SyncSession::fetch_body(uid: u32)` in `imap/mod.rs:313` — issues `UID FETCH <uid>
  BODY.PEEK[]` (peek-only, M1 read-only invariant preserved).
- `connect_sync()` in `imap/session.rs:755` — creates an authenticated `BoxedSession`
  from `AccountConfig` (creds + server config from keyring).
- `bodies.rs` TODO at line 99: "Attachment persistence is deferred" — must implement.
- Schema: `message_bodies` (body_text, body_html, body_complete), `attachment_parts`
  (part_number, filename, mime_type, size_bytes, local_path).
- `mail-parser 0.11` with `full_encoding` — decodes base64/Quoted-Printable automatically.
- `ammonia 4` — HTML sanitizer, already in Cargo.toml.
- Need `tauri-plugin-dialog` for the file-save picker (success criterion #2).

### Architecture

- `fetch_message(uid)` — on-demand IMAP fetch via BODY.PEEk[], parse, sanitize,
  cache in SQLite, return MessageView (headers + html + text + attachments).
- `save_attachment(uid, part_number, path)` — re-fetch body, extract part, write to disk.
- No body pre-fetching: reader fetches on demand, caches result.

### Decisions from Phase 3

- OFFSET pagination for message lists — not needed for single-message fetch
- ReadingPane is placeholder — Phase 4 implements the real reader
- `save_server_config` added to LoginForm — fetch_message can now reconnect

### Security invariants (from PLAN.md)

- D-flags: T-02-01 (read-only), T-02-02 (200-UID batches), T-02-03 (peek-only),
  T-02-04 (DoS cap)
- D-attachments: attachment filenames reduced to basename, confined under
  app-data/attachments/<uidv>/<uid>/
- D-render: ammonia sanitization + iframe sandbox + CSP
