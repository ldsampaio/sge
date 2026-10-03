# AGENTS.md — SGE

> GSD workflow enforcement + project context for agentic coding sessions (opencode runtime).

## Project

**SGE — Linux IMAP Desktop Client** (Rust + Tauri v2 + React + SQLite).
Core value: connect to an IMAP server on Linux and read mail locally in a fast Gmail-like UI.
Milestone 1: read-only INBOX viewer. Server facts: IMAP `mail.utfpr.edu.br:993/SSL`; SMTP `smtp.utfpr.edu.br:587/STARTTLS` reserved for a later send milestone.

See `.planning/PROJECT.md` for full context (requirements, constraints, decisions).

## Workflow (GSD)

- Phases run in numeric order per `.planning/ROADMAP.md` (5 phases, all `Mode: mvp`).
- Before planning a phase: `/gsd-discuss-phase N` (gather context), then `/gsd-plan-phase N`.
- Before executing: plans must exist and pass plan-check; execute with `/gsd-execute-phase`.
- After each phase: verify success criteria in ROADMAP.md, update STATE.md, evolve PROJECT.md (requirements validated/invalidated, decisions logged).
- Research (`workflow.research: true`), plan-check (`true`), and verifier (`true`) are enabled — do not skip them.
- Parallel execution is enabled for independent plans.

## Key Constraints

- IMAP must leave mail on server (BODY.PEEK only, never set \Seen in M1). No SMTP/send in M1.
- INBOX only; credentials only in OS keyring (never plaintext); Linux-only bundle.
- Headers-first sync, UIDVALIDITY-guarded incremental; sanitize HTML (ammonia) + sandboxed iframe before render.
- Stack pins: Tauri v2.12, async-imap 0.11, mail-parser 0.11 + full_encoding, rusqlite 0.37 bundled + rusqlite_migration 2.x, keyring 3 + sync-secret-service.

## Pointers

- Roadmap: `.planning/ROADMAP.md` | Requirements: `.planning/REQUIREMENTS.md`
- State: `.planning/STATE.md` | Research: `.planning/research/SUMMARY.md`
- Config: `.planning/config.json`
