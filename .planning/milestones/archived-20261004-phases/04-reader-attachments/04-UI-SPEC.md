# Phase 4 — UI Design Contract: Reader + Attachments

## Overview

The reader displays a single email message after the user selects it from the
Phase 3 message list. HTML is sanitized with ammonia and rendered in a sandboxed
iframe; attachments are listed with name/size and downloadable via a file-save
picker.

## Layout

```
┌─────────────────────────────────────────────────────────┐
│ ReadingPane                                             │
├─────────────────────────────────────────────────────────┤
│ From: alice@utfpr.edu.br  To: ...  Date: 2024-10-03    │  ← Header strip
├─────────────────────────────────────────────────────────┤
│ [Subject line]                                          │  ← Subject
├─────────────────────────────────────────────────────────┤
│ Attachments:                                            │
│  📎 homework.pdf (2.4 MB) [Save]                        │  ← Attachment list
├─────────────────────────────────────────────────────────┤
│ ┌─────────────────────────────────────────────────────┐ │
│ │ <sandiaboxed iframe>                                 │ │
│ │   sanitized HTML content                            │ │
│ │   (no scripts, no remote images, no iframes inside) │ │
│ └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

## States

| State | Trigger | UI |
|-------|---------|-----|
| Loading | `fetch_message` in flight | Skeleton bars (header lines shimmer) |
| Error | IMAP/TLS/auth failure | Error message + Retry button |
| Auth error | TLS/cert/refused | "Authentication or connection failed — please reconnect" |
| Empty | No HTML + no text | "📭 This message has no readable content" |
| Ready | HTML available | Sandboxed iframe with `srcDoc` |
| Ready (text only) | Only text/plain | `<pre>` with text content |
| Offline | No network + no cache | "Working offline — message not yet cached" |

## Security

- iframe sandbox: `sandbox="allow-same-origin"` (no `allow-scripts`, `allow-forms`,
  `allow-popups`) — scripts cannot run, forms cannot submit, popups blocked
- CSP meta-tag in the iframe document: blocks remote images, inline scripts
- ammonia strips: `<script>`, `on*` handlers, `javascript:` URLs, nested `<iframe>`
- No Tauri IPC from within the iframe (no `window.__TAURI__` access)

## Interaction

- Click attachment "Save" → opens native save dialog → writes file to chosen path
- Retry button on error → re-fetches message
- Message selection from Phase 3 MessageList passes `uid` to ReadingPane
