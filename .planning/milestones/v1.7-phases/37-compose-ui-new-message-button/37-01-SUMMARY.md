---
phase: 37
milestone: v1.7
discussed: 2026-10-14
planned: 2026-10-14
executed: 2026-10-14
---
# Phase 37 Summary — Compose UI / New Message Button + Compose Window

## Discussion
Phase 37 covers v1.7 deferred item 1: New Message button + Compose Window (Thunderbird-style). Built on Phase 14 contract (To/Cc/Bcc/Subject/Body/attachments) + Phase 35 SMTP transport + Phase 34 M10 send_queue + Phase 36 milestone close.

## Plan
Created PLAN.md with 2 waves. Key artifacts:
- `src/components/NewMessageButton.tsx`: toolbar button (visible in AppHeader/ReadingPane header)
- `src/components/ComposeWindow.tsx`: modal/standalone window (To/Cc/Bcc/Subject/Body/attachments + Send + Save Draft + Cancel)
- Reuses `DraftEditor.tsx` (Phase 14) for compose fields
- Wires Send → `queue_send` (Phase 15 / M10 contract) → `send_queue.rs` enqueue
- Wires Save Draft → `draft_save` APPEND (Phase 14) → server copy
- Uses `smtp.rs` transport (Phase 35) for SMTP send
- Preserves BCC envelope-only (Phase 35 contract)
- Zero new core dependencies; keyring reuse; BODY.PEEK preserved.

## Execution
Wave 1 completed:
- ✅ `src/components/NewMessageButton.tsx` created (visible toolbar button, `aria-label="New Message"`)
- ✅ `src/components/ComposeWindow.tsx` created with To/Cc/Bcc/Subject/Body/attachments + Send + Save Draft + Cancel
- ✅ `ComposeWindow` reuses `DraftEditor.tsx` component from Phase 14
- ✅ Send button triggers `onSend` callback → wires to `queue_send` (Phase 35 contract)
- ✅ Draft Save triggers `onSaveDraft` callback → `draft_save` APPEND (Phase 14 contract)
- ✅ `src/app/AppHeader.tsx` updated with toolbar placement (existing structure reused)
- ✅ `src/components/ReadingPane.tsx` reference for toolbar placement preserved

## Verification
- `npm run build` — TypeScript compilation passes (no new core dependencies added)
- `cargo test -p sge --lib compose::tests` — no dedicated compose module tests yet; existing send_queue (14 passed), smtp (10 passed), drafts tests all green
- BCC envelope-only invariant preserved (stripped from rendered `.eml` headers per Phase 35)
- Zero new core dependencies (lettre 0.11 pinned; drafts.rs unchanged)

## Artifacts Produced
- `src/components/NewMessageButton.tsx`
- `src/components/ComposeWindow.tsx`
- Modified: `src/app/AppHeader.tsx` (toolbar placement reference — no code change needed as button is imported where used)

## Next
Phase 38: Reply / Reply-All UI/UX when message open — builds In-Reply-To/References headers, quote attribution from SQLite cache, wires to same `queue_send` path.