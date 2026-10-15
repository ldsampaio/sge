# SGE v1.7 Roadmap — Compose UX + Reply/Reply-All + New Message

Milestone: v1.7 Compose UX + Reply/Reply-All + New Message. Continues numbering from Phase 36 (v1.6 ended at 36; 33 deferred permanently).

Phase range: 37–38 (2 phases, exact plan split confirmed at plan-phase).

### Phase 37: Compose UI / New Message Button + Compose Window (Thunderbird-style)
- Toolbar / global "New Message" button → opens Compose Window (modal or standalone window).
- Compose Window: To/Cc/Bcc/Subject/Body/attachments UI wired to Phase 14 contract → `queue_send` enqueue.
- Reuses existing `send_queue` contract (M10 finalized, Phase 34) and `smtp.rs` transport (Phase 35).
- OutboxBadge updates (`pending_count` from `send_status_snapshot`).
- SendStatus shows `sending / failed / retry` during send.
- Zero new core dependencies (lettre 0.11 pinned; no new crates).
- BODY.PEEK-only invariant preserved; no new IMAP fetch paths.
- End-to-end verification: compose → queue → SMTP send → Sent APPEND confirmed.
- ✅ **COMPLETED**: NewMessageButton.tsx + ComposeWindow.tsx implemented; AppHeader updated; DraftEditor reused.

### Phase 38: Reply / Reply-All UI/UX When Message Open
- In-Reply-To / References header build (proper threading per RFC 5322).
- Quote attribution with sender/date context (from message cache).
- Reply vs Reply-All: To/Cc population logic (original sender → To; all recipients except self → Cc).
- Wires to existing `queue_send` enqueue path (Phase 14 contract, Phase 37 UI).
- Preserves `imap_outbox` replay contract (separate from `send_queue`; Phase 10).
- OutboxBadge + SendStatus reflect pending/retry state for replies.
- Zero new core dependencies.
- BODY.PEEK-only invariant preserved.
- ✅ **COMPLETED**: Reply/Reply-All buttons in DraftEditor.tsx; ReplyContext type added; threading headers displayed; quote from cache.

Constraints carried from v1.6 audit / ROADMAP:
- BODY.PEEK-only on all read paths (no new fetch path changes).
- UID-only addressing; UIDVALIDITY-first gating; SessionManager single-session invariant.
- Zero new core dependencies (lettre 0.11 pinned; `native-tls` only if needed — SMTP already uses STARTTLS, no new TLS crate).
- Phase 33 deferred permanently — not revisited in v1.7.
- No SMTP server config changes; host/port fixed by server facts (`smtp.utfpr.edu.br:587`).
- Keyring creds integration reuses existing path; no new secret storage.
- Sandbox pre-flight + post-run secret grep preserved.

Trace: builds on v1.6 Phase 34-36 (send_queue M10 hardened, lettre 0.11 transport live, compose integration verified) and v1.3 Phase 13 contract.