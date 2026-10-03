# Plan 04-01 Summary — Backend Commands

## Completed

- Declared `pub mod bodies;` in `sync/mod.rs`, compiling the previously dead-code bodies module.
- Extended `bodies.rs` with:
  - `extract_attachments(msg) -> Vec<AttachmentInfo>` — detects parts with
    `Content-Disposition: attachment` or filename, extracting name/size/type/part_number.
  - `extract_attachment_bytes(msg, part_number) -> Option<Vec<u8>>` — returns decoded
    attachment content for a given part index.
  - `MessageDetail` struct + `fetch_message_detail` async fn (cache-aware body +
    attachment extraction via `BODY.PEEK[]`).
- Added `AttachmentInfo` struct to `queries.rs` (serializable, used by both Rust
  commands and the bodies module).
- Added `get_message_by_uid()` and `list_attachments()` query functions.
- Added `fetch_message(uid)` Tauri command — loads creds from keyring, connects
  to IMAP, `BODY.PEEK[]` fetch, mail-parser parse, ammonia sanitize, SQLite caching.
- Added `save_attachment(uid, part_number, file_path)` Tauri command — fetches
  body, extracts bytes, writes to user-chosen path (basename-only defense).
- Added `pub mod bodies;` — compiles the bodies module.
- Registered both commands in `lib.rs` `invoke_handler`.
- Added `tauri-plugin-dialog` + `tauri-plugin-fs` Rust deps; registered in `run()`.
- Tests: 63 passed (4 new bodies tests), 0 failed, 1 ignored.

## Verification
- `cargo check` — compiles (edition 2021, ammonia + mail-parser + rusqlite)
- `cargo test --lib` — 63 passed
