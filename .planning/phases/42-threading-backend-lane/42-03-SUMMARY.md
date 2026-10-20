---
phase: 42-threading-backend-lane
plan: 03
status: complete
subsystem: imap-wire-lane
tags: [references-lane, peek-audit, envelope-copy, same-commit-invariant]
requires: [THR-01, SC-2, D-02, parse_references (42-02)]
provides: [FETCH_REFS_ATTRS lane, fetch_refs_batch, attach_refs_row, MessageHeader.references, peek_audit path 5]
affects: [42-04 thread compute, 42-05 backfill driver, 42-06 payload]
tech-stack:
  added: []
  patterns: [attribute-constant + paren tripwire + peek_audit enumeration (FETCH_ATTRS analog)]
key-files:
  created: []
  modified: [src-tauri/src/imap/headers.rs, src-tauri/src/imap/manager.rs, src-tauri/src/sync/worker.rs, src-tauri/src/bin/sync_demo.rs, src-tauri/tests/live_gate_21_safety.rs, src-tauri/tests/live_gate_22_safety.rs, src-tauri/tests/live_gate_26_safety.rs]
key-decisions:
  - "FETCH_REFS_ATTRS stays its own PEEK-only constant; fetch_attrs_are_parenthesized asserts the two constants stay disjoint"
  - "shape miss (Fetch::section None) skips the UID so callers' requested-vs-received diff shows the gap instead of silence"
  - "references field additive only; MessageHeader literal builders updated mechanically (references: None)"
metrics:
  tasks: 2
  commits: 2
  duration: ~3 attempted subagent spawns + inline completion
actuals:
  tokens: n/a (inline)
  tasks: 2
  commits: 2
plan_head_before: 2754834
---

# Phase 42 Plan 03: Wire-Lane References FETCH Summary

Shipped THR-01's wire lane: `in_reply_to` copied off the already-fetched ENVELOPE (zero extra bytes), the separate `FETCH_REFS_ATTRS` lane feeding 42-02's parser, and `peek_audit` extended 4 → 5 paths in the SAME commit (SC-2's proof bar).

## Completed Tasks

- Task 1: `in_reply_to` copy-through on both ENVELOPE parse paths + additive `MessageHeader.in_reply_to` + shape-equality test. Commit `fb64467`.
- Task 2: `FETCH_REFS_ATTRS` constant, `fetch_refs_batch`, `attach_refs_row`, additive `MessageHeader.references`, `fetch_attrs_are_parenthesized` guard extension, `peek_audit` path 5 — all in one commit. Commit `119d26f`.

## Verification

- `cd src-tauri && cargo fmt --check` — clean.
- `cargo test -p sge peek_audit` — 1 passed (asserts exactly 5 audited paths).
- `cargo test -p sge fetch_attrs_are_parenthesized` — 1 passed (both constants parenthesized, disjoint, FETCH_ATTRS negative tripwire intact).
- `cargo test -p sge fetch_refs_batch` — 2 passed (`fetch_refs_batch_attaches_parsed_chain`: folded bytes parsed through `references.rs` only, shaped ids verbatim; `fetch_refs_batch_empty_field_is_own_thread`: empty/`\r` → `None` sentinel, shape-miss never attaches).
- `cargo test -p sge in_reply_to` / `parse_fetch` — shape-equality test passed on both ENVELOPE parse paths.
- Full `cargo test -p sge`: **543 passed / 2 credential-gated ignored / 0 failed** + all integration suites (48/42/19/19/15/6).
- Acceptance grep: every wire-time `HEADER.FIELDS` site in `src-tauri/src` references the `.PEEK`-prefixed constant (bare `BODY[HEADER.FIELDS …]` appears only inside doc prose warning against it).

## Must-Have Truths — all delivered

| Truth | Where |
|---|---|
| `.PEEK` mandatory on the lane (never sets \Seen) | `FETCH_REFS_ATTRS` constant + `peek_audit` bare-body assertion |
| Lane is a separate parenthesized constant, disjoint from FETCH_ATTRS | `fetch_attrs_are_parenthesized` extension + `!FETCH_ATTRS.contains("REFERENCES")` |
| peek_audit enumerate path 5 in the same commit | `audited_paths` 5-tuple + `assert_eq!( .., 5)` |
| Parse only through `imap::references::parse_references` (no `split(' ')`, no `decode_header_words`) | `attach_refs_row` doc + test |
| `None` = own-thread sentinel; shape miss visible via requested-vs-received diff | `fetch_refs_batch` skip + `fetch_refs_batch_empty_field_is_own_thread` |
| ENVELOPE wire bytes byte-identical (zero new bytes) | `FETCH_ATTRS` untouched; discipline test still green |

## Deviations from Plan

- Rule 3 (documented): adding `references: None` mechanically to 6 `MessageHeader` literal mock builders beyond the plan's `files_modified` list (sync_demo.rs, manager.rs, worker.rs, live_gate_21/22/26_safety.rs) — required to keep compiling with the additive field; one-line additions only, mirroring 42-01's arity ripple precedent.
- Execution-mode deviation: 3 subagent spawn attempts ended abnormally (output-token truncation ×2, interruption ×1) with partial artifacts on disk; completed inline per the execute-phase recovery path ("kill and switch to inline execution"), reconciling the in-flight Task 2 work against the plan contract before committing.

## Deferred / Limitations

- Live-server leg of the lane (real `mail.utfpr.edu.br` Zimbra referencesbyte capture) — deferred-with-reason: no IMAP credentials in this environment. Backstop is plan 42-07 `scripts/live-42.sh` (must exit 2 offline). Offline synthetic fixtures only; nothing here claims a live pass.
