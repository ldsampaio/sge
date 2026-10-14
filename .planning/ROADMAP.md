# SGE v1.6 Roadmap — SMTP Send Pipeline

Milestone: v1.6 SMTP Send. Continues numbering from Phase 32 (v1.5 ended at 32; 33 deferred permanently).

Phase range: 34–36 (3 phases, 6–7 plans — aligned with v1.4 Phase 13 scale; exact plan split confirmed at plan-phase).

### Phase 34: Send Queue Hardening + Crash Recovery (M10 finalize)
- Finalize `send_queue` schema (M10 already defined); preserve-rows migration M11 (if needed) from v1.4 state.
- State-machine hardening: queued→sending→sent/failed/uncertain; attempts/backoff; 25 MB cap enforcement.
- Crash-recovery at `start_sync`: reset `sending`→`queued`; never lose queued items.
- `uncertain` reconcile via Sent SEARCH by Message-ID (not re-SMTP).
- Keyring creds integration (reuses existing path; no new secret storage).
- Sandbox pre-flight + post-run secret grep.

### Phase 35: Lettre Transport + Live SMTP Gate (`smtp.utfpr.edu.br:587`)
- Lettre 0.11 sync over `spawn_blocking`; per-account pool; `SendGate` independent of `SyncGate`.
- STARTTLS `Tls::Required`; endpoint `smtp.utfpr.edu.br:587`.
- MIME single-render to outbox; envelope only for BCC.
- APPEND to Sent on success (verbatim bytes, `\Seen`); APPEND failure → `sent-unfiled` retry.
- Live gate: end-to-end send verified; exit-2 SKIP with no creds; never create creds.
- Transcript scrubber (redaction of addresses / content from logs if needed — note: only envelope/addresses in transport logs; body never logged).

### Phase 36: Compose Integration + Outbox UX
- Compose UI (Phase 14 contract) → `queue_send`; To/Cc/Bcc/Subject/Body/attachments wired.
- OutboxBadge (pending send count); SendStatus (sending / failed / retry); failed-retry action.
- `imap_outbox` replay contract preserved (separate from `send_queue`; no overlap).
- End-to-end verification: compose → queue → send → Sent-folder APPEND confirmed locally.
- Audit / milestone close (v1.6): 6-check on artifacts + state + git.

Constraints carried from v1.5 audit / ROADMAP:
- BODY.PEEK-only on all read paths (no new fetch path changes).
- UID-only addressing; UIDVALIDITY-first gating; SessionManager single-session invariant.
- Zero new core dependencies (lettre 0.11 pinned; `native-tls` only if needed — SMTP already uses STARTTLS, no new TLS crate).
- Phase 33 deferred permanently — not revisited in v1.6.
- No SMTP server config changes; host/port fixed by server facts (`smtp.utfpr.edu.br:587`).

Trace: builds on v1.3 Phase 13 contract and v1.4 audit (`v1.4-MILESTONE-AUDIT.md`: PASSED 11/11, send pipeline verified at code level; live gate deferred-with-reason).
