# Verification: Phase 18 (Confirm + Trust UX)

- [x] Confirm moves into auto-created `Auto/<Top>/<Child>` (plan+ensure+execute; offline replays via outbox)
- [x] Direct-IPC bypass refused backend-side (`plan_filing` requires a label row; no-label → `NoSuggestion`, stale → `StaleSuggestion`)
- [x] Override re-moves + logs + pins across syncs (drain skips `latest_override` rows)
- [x] Badges: primary per email, secondary distinct local-only; queue/`classifying` states
- [x] Redacted justification at confirm time, transient only (never persisted — schema has no column for it)
- [x] Threshold tunable; `A Classificar` never moves on its own (no filing path accepts it as destination)
- [ ] Live confirm→MOVE→undo vs real server — DEFERRED (no headless creds; see SUMMARY)

**Verdict: PASSED (with one documented live deferral).**
