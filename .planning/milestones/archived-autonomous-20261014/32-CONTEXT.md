# Phase 32: Compose Hardening — Context (autonomous)
Mode: Smart discuss (autonomous); parallelizable after Phase 27 sandbox pre-flight (no probe dependency vs Phase 33).
Domain: End-to-end verification + fix over v1.2 pipelines (12 drafts APPEND / 13 SMTP send-queue / 14 compose UI reply/forward/attachments). Verify + fix, never rebuild. Keyring reuse; redaction + sandbox contracts preserved; SMTP STARTTLS Tls::Required (no opportunistic). Headless-first with regression tests.
Blocker preserved: full cargo build blocked by pre-existing batch.rs delimiter (#1216); structural verification + deferred full-cargo line required in plan (not hidden).
Deferral from v1.2: SMTP 587 live send (Phase 27 deferred ledger) — Phase 32 can verify pipeline structure without live server (self-only sandbox mail); live UAT stays deferred.
