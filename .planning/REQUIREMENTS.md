# SGE v1.6 Requirements — SMTP Send Pipeline

Milestone: v1.6 SMTP Send (brownfield; continues v1.5 numbering from Phase 32). Phase 33 (ManageSieve) remains deferred permanently (port-4190 probe failed; no ManageSieve on server `mail.utfpr.edu.br`); not in this milestone.

Constraints (carried from .planning/PROJECT.md / ROADMAP.md / v1.5 audit):
- SMTP `smtp.utfpr.edu.br:587/STARTTLS`; IMAP `mail.utfpr.edu.br:993/SSL`. Body.PEEK-only on read; never set \Seen in M1/append paths.
- Creds only in OS keyring (keyring 3 + sync-secret-service); never plaintext, never committed.
- Fail-closed STARTTLS (`Tls::Required`); no opportunistic TLS.
- Sandbox for live legs: exit-2 SKIP if no creds; never create a creds file; self-only mail; pre-flight guard; post-run secret grep.
- Forward-only migrations with preserve-rows tests; zero new core dependencies (lettre 0.11 already pinned; `send_queue` table M10 already defined).

Requirements (scoped; 5/5 — aligns with Phase 13 send-pipeline scope in ROADMAP.md line 105–118):
1. Durable `send_queue` state machine (queued | sending | sent | failed | uncertain) with attempts/backoff, persisted to SQLite M10.
2. MIME rendered once to `<app_data>/outbox/<id>.eml`; envelope columns (to/cc/bcc/from/subject/body); 25 MB cap; BCC envelope-only.
3. Lettre 0.11 sync transport via `spawn_blocking`; per-account pool caching; `SendGate` separate from `SyncGate`.
4. Crash recovery: at launch reset `sending`→`queued`; `uncertain` reconcile via Sent SEARCH by Message-ID, never re-SMTP-send.
5. Sent-folder APPEND (`
Seen` set), not re-send; APPEND failure → `sent-unfiled` with retry, never re-SMTP.
6. Live gate: end-to-end send against `smtp.utfpr.edu.br:587` (keyring creds) verified; deferred-with-reason if no creds file available.
7. Error mapping to plain language; zero secret leakage in logs / transcripts; transcript scrubber (pre-flight + post-run).
8. Compose UI (Phase 14 contract) integrates with `queue_send`; outbox badge + failed-retry surface.

Deferred / out of scope:
- Phase 33 ManageSieve / sieve upload (server capability absent; deferred permanently with reason).
- QRESYNC (deliberately deferred per v1.5; UID tombstoning covers gap).
- SMTP relay / multiple-account send; only single-account `smtp.utfpr.edu.br`.
- Attachment encryption / PGP (not in pipeline scope).

Trace: v1.4 Phase 13 (Send Pipeline) delivered M10 `send_queue` table, lettre 0.11, SendGate — v1.6 validates live gate + hardening on that pipeline + compose integration.
