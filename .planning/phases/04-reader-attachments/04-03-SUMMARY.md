# Plan 04-03 Summary — Security Hardening

## Completed

- **CSP**: Extended `tauri.conf.json` CSP with:
  - `frame-src 'self'` — restricts iframe sources
  - `object-src 'none'` — blocks plugins (Flash/Java/etc)
  - `base-uri 'none'` — prevents `<base>` tag injection
  - `frame-ancestors 'none'` — prevents frame embedding
- **iframe sandbox**: `sandbox=""` with NO allowlist (no `allow-scripts`,
  `allow-same-origin`, `allow-forms`, `allow-top-navigation`). Content via
  `srcDoc` is in a unique origin — cannot access parent `window` or Tauri IPC.
- **referrerPolicy**: `no-referrer` — prevents referrer leakage to remote
  resources in the iframe.
- **ammonia sanitization**: server-side `bodies::sanitize_html()` with default
  allow-lists strips `<script>`, `<iframe>`, `<object>`, event handlers, and
  `srcdoc`/`data:`/`javascript:` URIs from HTML before it reaches the iframe.
- **path traversal defense**: `save_attachment` reduces `file_path` to basename
  only (D-attachments — prevents writing outside the chosen directory).
- **BODY.PEEK**: IMAP fetch uses `BODY.PEEK[]` (never sets `\Seen`) — confirmed
  in `SyncSession::fetch_body` implementation.

## Verification
- Rust tests pass (63); ammonia test `sanitize_strips_script_and_iframe` passes
- TypeScript + ESLint + build all clean
