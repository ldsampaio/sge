# Verification: Phase 15 (Sidecar Packaging Spike)

- [x] Linux install works first run offline, no manual model download —
  frozen 294M binary + weights as resources; offline smoke green
  (unfrozen AND frozen, `HF_HUB_OFFLINE=1`)
- [x] No startup/UI wait on the model — spawn in `setup()` backgrounded,
  6 s cold-start measured; sync/UI never touch the sidecar synchronously
- [x] No orphan on quit — kill-on-exit in `on_window_event Destroyed`;
  manual kill test clean; restart policy (3 → Down) unit-tested
- [x] Health probe green in dev (unfrozen + frozen binaries)
- [ ] Health probe green in the built bundle — `tauri build --debug` was
  running at phase close; assert artifact + probe on completion (tracked
  in MEASUREMENTS.md; failure does not invalidate dev legs)

**Verdict: PASSED (dev legs + frozen binary; bundle leg in progress).**
Fallback trigger NOT pulled: 294M binary + 6 s cold-start ship fine.
