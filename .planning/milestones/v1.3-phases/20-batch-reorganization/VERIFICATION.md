# Verification: Phase 20 (Batch Reorganization)

- [x] Whole-account run, no per-email confirm, live progress (events + UI bar)
- [x] Full `Auto/` tree pre-created up front; chunked 25–50 with per-chunk UIDVALIDITY re-check
- [x] Persisted per-category report + `A Classificar` remainder
- [x] Journaled per message; resume skips journaled (tested); undo restores originals
- [x] Pinned corrections never enter batch; excluded folders skipped
- [ ] Live large-folder gate (chunk tuning vs throttling server) — DEFERRED (creds)
- [ ] Frozen/binary + bundle measurements — PENDING (freeze running)

**Verdict: PASSED backend+UI (measurement legs pending Phase 15 close).**
