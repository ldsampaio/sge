# Verification: Phase 17 (Classify Engine, No Moves)

- [x] Sync a folder → sane pt-BR labels appear shortly after (live: 5/8 agreement, misses safe-routed; stub tests prove drain)
- [x] Manual single classify works without moving anything (command + structural grep proof)
- [x] Excluded folders (Sent/Drafts seeded) skipped by automatic pass; manual still works (seed + is_excluded tests)
- [x] Low-confidence/edge mail lands in `A Classificar` review (gate + veto tests; live prova 0.58 + boleto-2 0.45 routed there)
- [x] Sidecar down → pending, sync unaffected (Unreachable → back-to-pending; hook never awaits)
- [x] Password-reset mail → no secret in DB/logs/UI (redact fixtures; labels pointer-only)

**Verdict: PASSED.**
