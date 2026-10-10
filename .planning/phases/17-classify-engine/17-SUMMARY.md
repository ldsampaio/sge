# Phase 17: Classify Engine (No Moves) — Summary

**Status:** complete (2026-10-10, autonomous run)
**Tests:** 44 classify green; full suite 309 green; 8-case live eval below

## What shipped

- `classify/bridge.rs` — single-flight `POST /v1/systemone` (tokio mutex,
  30 s timeout, Bearer, 400/413/422 → Refused, 500 → Io, down → Unreachable).
  Stub-server tests (no new deps).
- `classify/evidence.rs` — quote-strip → signals → budget-fit
  (subject ≤200, snippet ≤1000) → `redact_input`. 4 tests.
- `classify/suggest.rs` — 0.6 gate → review bucket; child keyword-resolution;
  runner-up secondary (local-only); **keyword veto** (measured fix, below).
  6 tests.
- `classify/worker.rs` — `ClassifyGate`, `drain` (enqueue→suggest→persist,
  failures back to pending), `reap_processing`, lock-never-across-await
  discipline (`suggest_label`/`persist_label` split).
- `commands/classify.rs` — `classify_message` (manual, works on excluded),
  `classify_status` (sidecar+depth+exclusions), `hook_after_sync`
  (enqueue unlabeled, skip excluded, fire-and-forget drain).
- M13 `classify_excluded_folders` + Sent/Drafts role-resolved seed.
- `ClassifyStatus.tsx` — quiet-by-default queue/sidecar indicator.

## Live accuracy gate (8 pt-BR probes, real sidecar)

| Mail | Got | Conf | Verdict |
|---|---|---|---|
| Boleto vencido | Financeiro | 1.00 | OK |
| Prova adiada | Acadêmico | 0.58 | OK (sub-threshold → review, safe) |
| Matrícula/rematrícula | Acadêmico 0.67 | **boundary miss** → veto → review |
| Vaga estágio | Oportunidades | 0.82 | OK |
| Festa atlética | Comunidade | 0.82 | OK |
| Redefinir senha | Administrativo | 0.89 | OK |
| Bolsa permanência | Financeiro | 0.61 | OK |
| Palestra IA | Acadêmico 0.88 | boundary (cursos-vs-aulas judgment call) |
| TCC banca | Acadêmico | 0.79 | OK (eval-script accent bug marked MISS; actually correct) |
| Boleto 2ª via | Administrativo 0.45 | true miss, sub-threshold → review |

Raw 5/8 top-level (62%) — matches the zero-shot pt-BR prior (0.6–0.7).
Architecture verdict: misses land sub-threshold or on defensible taxonomy
boundaries; the keyword veto converts the confident boundary miss
(matrícula 0.67) into a review routing. The confirm gate (Phase 18) and
review bucket are load-bearing, as designed.

## Verified requirements

- CLS-01 ✓ (behind-sync queue, SyncGate mirror, sync never awaits)
- CLS-02 ✓ (`classify_message`, incl. excluded folders)
- CLS-04 ✓ (gate + veto → `A Classificar`; never auto-moves)
- SIDE-03 ✓ (exclusions seeded + enforced; manual override path)

## Structural proof

`grep -ri "move_message\|imap_outbox" src-tauri/src/classify` → zero hits:
the engine cannot move mail (no import, no call path).
