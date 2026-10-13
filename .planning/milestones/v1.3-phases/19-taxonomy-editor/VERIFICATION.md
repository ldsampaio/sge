# Verification: Phase 19 (Taxonomy Editor + Import)

- [x] Add/rename/delete + keywords/rules in options UI, ID-stable, no orphaned labels (orphan assert = 0; remap tests)
- [x] Overrides survive structural ops (override rows untouched; ids remapped alongside labels)
- [x] Import validated before anything changes (cycle/dup/Auto/charset/depth/delimiter fixtures rejected with reasons)
- [x] Export portable (round-trip: default export re-imports clean)
- [x] Rename → folder RENAME + relabel-free; merge → MOVE + remap; delete → orphan-prompt + guards
- [ ] Live RENAME/DELETE vs real server — DEFERRED (standing live deferral)

**Verdict: PASSED (with the standing live deferral).**
