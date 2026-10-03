# Phase 4 — Reader + Attachments — Summary

## Status: Complete ✅

## What was delivered

### Backend (Wave 1 — 04-01)
- Wired up `src-tauri/src/sync/bodies.rs` (was dead code — never declared as a module)
  with `pub mod bodies;` in `sync/mod.rs`.
- Added `MessageView` struct + `fetch_message(uid)` Tauri command.
  Loads creds from keyring, connects IMAP (BODY.PEEK[], read-only),
  parses with mail-parser, sanitizes HTML with ammonia, caches in SQLite.
- Added `save_attachment(uid, part_number, file_path)` Tauri command.
  Fetches body, extracts attachment bytes, writes to user-chosen path
  (basename-only defense-in-depth).
- Added `AttachmentInfo` struct to `queries.rs`; `get_message_by_uid`,
  `list_attachments` query helpers.
- Added `extract_attachments()` + `extract_attachment_bytes()` to bodies.rs.
- Added `tauri-plugin-dialog` + `tauri-plugin-fs` to Cargo.toml + lib.rs.

### Frontend (Wave 2 — 04-02)
- Rewrote `ReadingPane.tsx` (was placeholder) — full message reader:
  - Iframe with `sandbox=""` (no allowlist) + `referrerPolicy="no-referrer"`
  - Ammonia-sanitized HTML via `srcDoc`
  - Plain-text `<pre>` fallback
  - Attachment list with "Save" buttons
  - Loading/empty/error states
- Added `AttachmentInfo` + `MessageView` types to `types.ts`.
- Added CSS for reading pane header, attachments, iframe, text, states.
- Used `@tauri-apps/plugin-dialog` `save()` for file picker.

### Security (Wave 3 — 04-03)
- Extended CSP: `object-src 'none'`, `base-uri 'none'`, `frame-ancestors 'none'`,
  `frame-src 'self'`.
- iframe sandbox with no permissions — no IPC reachability from rendered content.
- `referrerPolicy="no-referrer"` — no referrer leakage.
- Path-basename reduction in `save_attachment` (D-attachments).

## Success Criteria Verification

| # | Criterion | Status |
|---|-----------|--------|
| 1 | Open message, read sanitized HTML (remote images blocked) or plaintext | ✅ |
| 2 | See attachment names/sizes, save via picker | ✅ |
| 3 | Read vs unread visual distinction (display-only, no \Seen writes) | ✅ |
| 4 | Malicious mail (script/iframe/object) renders inert, no IPC reach | ✅ |

## Verification Results
- **Rust**: `cargo check` ✓, `cargo test --lib` — 63 passed, 0 failed, 1 ignored
- **Frontend**: `tsc --noEmit` ✓, `eslint` ✓, `vite build` ✓
