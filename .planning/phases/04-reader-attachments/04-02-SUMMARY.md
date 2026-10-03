# Plan 04-02 Summary — ReadingPane Component

## Completed

- Rewrote `ReadingPane.tsx` (was placeholder) with full message rendering:
  - Calls `fetch_message` via `@tauri-apps/api/core::invoke` on message select.
  - Renders sanitized HTML in a `sandbox=""` iframe (`srcDoc`, `referrerPolicy="no-referrer"`).
  - Plain-text fallback in `<pre>` when no HTML part.
  - Loading / empty / error states.
- Added `@tauri-apps/plugin-dialog` via `tauri add dialog` for the save dialog.
- `handleSaveAttachment`: opens `save()` dialog → invokes `save_attachment` Tauri command
  with user-chosen path. Path is basename-reduced server-side (D-attachments).
- Added `AttachmentInfo` + `MessageView` types to `types.ts`.
- Added CSS styles for `.reading-header`, `.reading-attachments`, `.attachment-btn`,
  `.reading-iframe`, `.reading-text`, loading/error states.

## Verification
- `tsc --noEmit` — 0 errors
- `eslint` — 0 errors
- `vite build` — succeeds, dist/ produced
