---
phase: 42-threading-backend-lane
plan: 02
status: complete
subsystem: imap-threading
tags: [references-parser, fixtures, tracer, offline-corpus]
requires: [THR-01, D-04, D-05]
provides: [parse_references single parse point, ThreadChain, fixture corpus]
affects: [42-03 fetch lane, 42-04 thread compute]
tech-stack:
  added: []
  patterns: [pure-leaf-module + cfg(test) fixtures (mutf7.rs analog)]
key-files:
  created: [src-tauri/src/imap/references.rs]
  modified: [src-tauri/src/imap/mod.rs]
key-decisions:
  - "fixture fn names carry references_ prefix so plan verify filters select the corpus"
  - "None = own-thread sentinel; store never writes synthetic empty chain"
  - "contrast probe pins decode_header_words mangling proof in-repo"
metrics:
  tasks: 2
  commits: 2
  duration: ~40m
actuals:
  tokens: 3022
  tasks: 2
  commits: 2
plan_head_before: 239608cc7f51149676f0919212f95135bedb3529
---

# Phase 42 Plan 02: References Header Parser Summary


Shipped `src-tauri/src/imap/references.rs`: a pure References-field byte parser (unfold → tokenize → normalize → dedupe → cap) with the full D-05 offline fixture corpus, proving msg-id tokens never pass through `decode_header_words`.

## Completed Tasks

- Task 1: `references.rs` — `parse_references(&str) -> Option<ThreadChain>` plus 7 named fixtures (`references_folded_two_line`, `references_framed_multi`, `references_bare_and_case`, `references_malformed_or_empty`, `references_duplicate_ids`, `references_chain_cap_overflow`, `references_encoded_shaped_ids`) and the `references_decoder_mangles_ids_contrast_probe`. Commit `df5aefd`.
- Task 2: `pub mod references;` registered alphabetically in `imap/mod.rs`. Commit `81dc680`.

## Verification

- `cd src-tauri && cargo fmt --check` — clean (fmt run before each commit).
- `cargo test -p sge references_` — **8 passed / 0 failed** (7 fixtures + contrast probe; 534 filtered out).
- `cargo test -p sge references_folded_two_line` — 1 passed; `cargo build -p sge` Finished (warnings pre-existing).
- Offline synthetic fixtures only (D-05): no live-server leg claimed; as-received Zimbra fold widths remain deferred to plan 42-07's `scripts/live-42.sh` capture (backstop).

## Must-Have Truths — all delivered

| Truth | Where |
|---|---|
| Unfold CRLF+WSP FIRST; ids carry no CR/LF | `references_folded_two_line` asserts clean ordered ids |
| Order-preserving first-occurrence dedupe | `references_duplicate_ids` → `["<a@x>", "<b@y>"]` |
| Empty/absent → `None` (NULL `references_json`, no synthetic chain) | `references_malformed_or_empty` (`.is_none()` asserts) |
| Cap: first 100 of 150 ids, `truncated` recorded, 32 KiB raw guard | `references_chain_cap_overflow` |
| Msg-ids VERBATIM; contrast probe proves `decode_header_words` mangles `<X=?utf-8?Q?=42?=@host>` → `<XB@host>` (shorter) | `references_encoded_shaped_ids` + probe |
| Bare/case-variant tokens framed `<...>`, case preserved | `references_bare_and_case` |

## Deviations from Plan

1. **[Rule 3] Fixture test fns carry a `references_` prefix** (`references_folded_two_line`, not `folded_two_line`): cargo test paths are `imap::references::tests::<fn>`, so the plan's verify filters (`references_`, `references_folded_two_line`) only match when the fn name itself starts with `references_`. RESEARCH contract names preserved as exact substrings.
2. **[Rule 3] Registration staged before Task 1's gate, committed as Task 2:** an unregistered `.rs` file isn't compiled, so `pub mod references;` had to exist for the 8-test filter to report >0. Atomic per-task commits preserved.
3. **[Rule 1] Toolchain fixes while authoring** (array-literal `[]` → `Vec::new()`, `assert_eq` bangless → bangless-with-`is_none()` / `assert_eq!` per toolchain, `fmt!` printf spec → `{:03}`) — compile-time, no behavior impact; final file compiles and passes.

## Threat Handling (T-42-02)

Tokenizer is strict (`<...>`/bare only, brackets can't be smuggled in bare tokens), dedupe is order-preserving, and the 100-id / 32 KiB caps bound graph-walk blowup with `truncated` recorded; verbatim pass-through is asserted, not prose (contrast probe).

## Artifacts

- `src-tauri/src/imap/references.rs` — 275 lines (new)
- `src-tauri/src/imap/mod.rs` — +1 line `pub mod references;`

## State for 42-03

`parse_references` is the single parse point at fetch time; store (42-01 M26) persists `references_json` verbatim from `ThreadChain.ids`; `None` → NULL sentinel.

## Self-Check: PASSED

- `src-tauri/src/imap/references.rs` — FOUND (275 lines, `pub mod references;` present in mod.rs)
- Commits FOUND in git log: `df5aefd` (parser), `81dc680` (registration)
- Measured plan commits from ledger `239608c..HEAD` = 2

