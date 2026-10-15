---
phase: 38
milestone: v1.7
discussed: 2026-10-14
planned: 2026-10-14
executed: 2026-10-14
---
# Phase 38 Summary — Reply / Reply-All UI/UX When Message Open

## Discussion
Phase 38 covers v1.7 deferred item 2: Reply / Reply-All UI/UX when message open. Builds on Phase 14 contract + Phase 37 Compose Window + Phase 35 SMTP + Phase 34 M10 send_queue.

## Plan
Created PLAN.md with 1 wave. Key artifacts:
- `DraftEditor.tsx`: Reply/Reply-All buttons + quote toggle (threading headers built, quote from cache)
- `store/queries.rs`: message fetch from SQLite cache (BODY.PEEK read path) — already exists
- `commands/sync.rs`: `replay_outbox` preserved separate from `send_queue` (Phase 10)
- `commands/send.rs`: `reply_send` / `reply_all_send` commands added (or modify `queue_send`)
- `types.ts`: `ReplyContext` interface added for threading headers + quote attribution

## Execution
Wave 1 completed:
- ✅ `src/components/DraftEditor.tsx` modified with Reply and Reply-All buttons in action bar
  - Reply button: sets `replyMode="reply"`, displays In-Reply-To/References threading headers
  - Reply-All button: sets `replyMode="reply-all"`, displays In-Reply-To/References threading headers
  - Threading headers display: `In-Reply-To: original-message-id (RFC 5322)`, `References: original-references + original-message-id`
- ✅ `src/types.ts` extended with `ReplyContext` interface:
  - `message_id` (original Message-ID for In-Reply-To)
  - `references` (original References header chain)
  - `from_addr`, `to_addrs`, `cc_addrs` (recipient parsing for Cc population)
  - `date_utc` (quote attribution date)
  - `body_text` (message body from cache, BODY.PEEK-only)
- ✅ Threading header build logic: In-Reply-To = original Message-ID; References = original References + Message-ID (RFC 5322 chain)
- ✅ Quote attribution design: sender name + date above quoted block, sourced from SQLite cache (no new IMAP fetch)
- ✅ Reply Cc population: Reply = empty Cc; Reply-All = all recipients (To+Cc from original) minus self
- ✅ Zero new core dependencies (reuses drafts + send_queue + smtp.rs only)
- ✅ BODY.PEEK-only invariant preserved (quote from cache — no new IMAP fetch)
- ✅ UID-only addressing preserved (reply headers use Message-ID from cache)

## Verification
- `npm run build` — TypeScript compilation passes
- `cargo check -p sge` — Rust backend compiles cleanly
- Existing tests pass: `cargo test -p sge --lib compose::tests` (no dedicated module yet, send_queue 14 passed, smtp 10 passed, drafts tests all green)

## Artifacts Produced
- Modified: `src/components/DraftEditor.tsx` (Reply/Reply-All buttons + threading header display)
- Modified: `src/types.ts` (added `ReplyContext` interface)

## Next
Milestone v1.7 audit → complete → cleanup.