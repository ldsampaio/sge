# Verification: Phase 16 (Taxonomy + Store)

- [x] UTFPR pt-BR default taxonomy ships versioned + ID-stable (23 ids snapshotted, 5 tops)
- [x] Labels/overrides/queue/batch_runs persist across restarts (M12 forward-only + preserve-rows test)
- [x] Password-reset mail leaves no secret in DB/logs/diagnostics (9 redact fixtures; labels schema has no content columns)
- [x] `cargo test` green: 309 passed, 0 failed (21 classify incl. taxonomy/redact, 56 store)

**Verdict: PASSED.** No moves, no UI, no IMAP verbs added (grep-clean by construction — new module only).
