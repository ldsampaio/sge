# Phase 22: Draft & Seen Round-Trip — Live Gate (Live-02, SANDBOXED) - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-10-13
**Phase:** 22-Draft & Seen Round-Trip — Live Gate (Live-02, SANDBOXED)
**Areas discussed:** Sandbox mechanics, One-copy draft proof, Offline + resurrection proof, Credential handling in scripts
**Mode:** Text-mode (interactive `question` unavailable — dismissed by runtime; numbered lists, user replied with selections)

---

## Sandbox mechanics

| Option | Description | Selected |
|--------|-------------|----------|
| 1a-1 | `SGE-Test-<timestamp>` at account root, deleted post-run | ✓ |
| 1a-2 | Nested under persistent `SGE-Tests` parent | |
| 1b-1 | Name-prefix + exists-and-empty guard only | |
| 1b-2 | Above + LIST snapshot, abort on unexpected mid-run change | ✓ |
| 1c-1 | Leave sandbox + manual-cleanup steps only | |
| 1c-2 | Retry N with backoff, then manual-cleanup steps | ✓ |
| 1d-1 | Test drafts into sandbox folder, test-only role override | ✓ |
| 1d-2 | Real Drafts folder with `SGE-TEST-` subjects + expunge | |

**User's choice:** `1a=1, 1b=2, 1c=2, 1d=1`
**Notes:** Preceded by a milestone-level correction: user stated live-server testing IS possible with their credentials, reversing the earlier "no live server" adjustment; milestone artifacts re-adjusted to sandboxed-live before this discussion. User's hard constraints: no tampering with existing mail/folders, test mail only to self.

---

## One-copy draft proof

| Option | Description | Selected |
|--------|-------------|----------|
| 2a-1 | SEARCH returns 1 UID AND equals tracked `draft_uid` | ✓ |
| 2a-2 | Count-only | |
| 2b-1 | Live kill-session between APPEND and expunge, assert reconcile to 1 | ✓ |
| 2b-2 | Failure mode offline-only; live covers happy path | |
| 2c-1 | Edit-offline-then-reconnect in scope (latest wins) | ✓ |
| 2c-2 | Defer to doubles | |
| 2d-1 | FETCH surviving copy, compare subject/body | ✓ |
| 2d-2 | UID/count only | |

**User's choice:** `2a=1, 2b=1, 2c=1, 2d=1` (strictest on every axis)

---

## Offline + resurrection proof

| Option | Description | Selected |
|--------|-------------|----------|
| 3a-1 | App-level offline toggle (deterministic) | ✓ |
| 3a-2 | Real network cut | |
| 3b-1 | Verb order in logs + end-state (pre-sweep replay before sweep) | ✓ |
| 3b-2 | End-state only | |
| 3c-1 | Double-sync resurrection test (second full sync, UID stays gone) | ✓ |
| 3c-2 | Above + third sync after poll tick | |
| 3d-1 | Seen round-trip both directions across reconnect | ✓ |
| 3d-2 | Seen-direction only | |

**User's choice:** "all answers are 1"

---

## Credential handling in scripts

| Option | Description | Selected |
|--------|-------------|----------|
| 4a-1 | Source env file, pass as env vars, never CLI args | ✓ |
| 4a-2 | Binary reads file directly | |
| 4b-1 | Env seeds keyring once; auth reads keyring (exercises real path) | ✓ |
| 4b-2 | Env password used directly | |
| 4c-1 | Post-run automated secret scan, fail on hit | ✓ |
| 4c-2 | Manual log review | |
| 4d-1 | `SGE-TEST-` prefix + per-run Message-ID + no-unprefixed-send audit | ✓ |
| 4d-2 | Message-ID only | |

**User's choice:** "all answers are 1"

---

## the agent's Discretion

- Cleanup retry count N and backoff schedule (D-03).
- App-level offline toggle implementation (flag vs injected transport failure).
- SEARCH/FETCH verb dialect details for assertions.

## Deferred Ideas

None — discussion stayed within phase scope.
