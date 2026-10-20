# Phase 42: Threading Backend Lane - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-10-20
**Phase:** 42-Threading Backend Lane
**Areas discussed:** Backfill de referências; Estratégia de thread-score

---

## Áreas discutidas (seleção inicial)

| Área | Selecionada |
|------|-------------|
| Backfill de referências | ✓ |
| Estratégia de thread-score | ✓ |
| Fixtures de mano do servidor (captura ao vivo) | ✗ (deferido à desseção do agente → D-05) |

---

## Backfill de referências

| Option | Description | Selected |
|--------|-------------|----------|
| Automático no primeiro sync por pasta | 1 lane extra HEADER.FIELDS por pasta na 1ª sincronização pós-upgrade, marcado como feito; syncs seguintes só novas mensagens | ✓ |
| Automático em background + status visível | Progress bar em SyncStatus com opção pular por pasta | |
| Manual por pasta (botão) | Botão "Ativar conversas nesta pasta" na sidebar; zero custo-surpresa, maior fricção | |
| Sem backfill (só novas) | Mail antigo nunca vira conversa até nova resposta | |

**User's choice:** Automático no primeiro sync por pasta (Recomendado)
**Notes:** Executor deve preservar disciplina de sync-gate (single-flight, batches, resumável, idempotente); folder grande não bloqueia caminho interativo.

---

## Estratégia de thread-score

| Option | Description | Selected |
|--------|-------------|----------|
| mail-threading crate (0.1.3) | JWZ + RFC 5256 conformance corpus; deps = chrono only; MSRV ok; in-house JWZ como fallback documentado se crate falhar | ✓ |
| JWZ in-house (~250 LOC) | Zero nova dep; maior esforço de fase e risco de bug algoritmo clássico | |
| Tree simplified (sem JWZ) | Encadeamento direto parent→child sem dummies; quebra no mail Zimbra real | |

**User's choice:** mail-threading crate (Recomendado)
**Notes:** `subject_merge: false` (grouping por referencias, não assunto — estratégia de assunto permanece rejeitada).

---

## the agent's Discretion

- **Fixture corpus (D-05):** área não selecionada para discussão → decisão por convenção do projeto: fixtures sintéticos offline (folded/brackets/malformed/dupes/empty/cap) + captura ao vivo de bytes REAIS deferida com motivo (padrão LIVE-01/02/03; script no padrão `scripts/live-*` fornecido para anexar bytes reais depois).
- Parsing/payload/SQL/ordem/erros — território research/planner (anotado em CONTEXT.md Agent's Discretion).

## Deferred Ideas

- Nenhuma — discussão ficou no escopo da fase.

---

## Planning-time gates — recorded verdicts (2026-10-20)

| Gate | Raw verdict | Action taken | Rationale |
|------|-------------|--------------|-----------|
| `check ui-plan-gate 42` | `{frontend:true, hasUiSpec:false, block:true, matchedToken:"ui"}` | **Proceeded with `--skip-ui` equivalent** (no UI-SPEC generated for this phase) | False positive: the token matched the goal sentence "…zero-risk to the UI". Phase 42 is backend-only per its own ROADMAP/CONTEXT text ("no UI work", "no grouping UI, no reader work"), touches no file under `src/`, and its success criteria require the flat list render stay *byte-identical*. UI design contracts belong to phases 43–46. |
| `verify context-drift 42` | `{block:false, stale_artifacts:[]}` | clean | — |
| `verify codebase-drift` | `{skipped:true, reason:"no-structure-md"}` | skipped by tool | No `.planning/codebase/STRUCTURE.md` in this repo. |
| `check decision-coverage-plan` | `{passed:true, covered:5/5}` | pass | D-01…D-05 all referenced by plans. |
| `check gap-analysis.plan-post` (advisory) | `{block:false, passed:true}` but table reports `34 of 34 … Not covered` incl. THR-01…04 | **Known false negative — ignored** | Same class as the milestone `verify-summary` false negative. The canonical coverage gate already confirmed all four phase REQ-IDs present in plan frontmatter (`42-01:[THR-01,THR-04] 42-02:[THR-01] 42-03:[THR-01,THR-04] 42-04:[THR-02] 42-05:[THR-01,THR-02,THR-04] 42-06:[THR-03] 42-07:[THR-01..04]`); the post-planning heuristic scans plan *prose* and does not read `requirements:` frontmatter, so it reports every milestone-wide REQ-ID (including the 30 owned by phases 43–46) as uncovered. |
| `check verify-command-paths` / `verify-failure-directions` | `{status:"ok"}` ×2 (both iterations) | pass | Note: these probe **paths and `<fails_when>` presence only, not flag validity** — the invalid `-x` cargo token was caught independently by the plan-checker and by direct execution in `src-tauri/`. |
