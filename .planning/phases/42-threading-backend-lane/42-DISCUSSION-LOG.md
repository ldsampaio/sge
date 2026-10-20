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
