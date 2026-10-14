---
phase: 29-rule-import-export-idle-watcher
reviewed: 2026-10-14T05:30:00Z
depth: deep
files_reviewed: 13
files_reviewed_list:
  - src-tauri/src/classify/rules_io.rs
  - src-tauri/src/commands/classify.rs
  - src-tauri/src/store/mod.rs
  - src-tauri/src/store/queries.rs
  - src-tauri/src/imap/idle.rs
  - src-tauri/src/imap/mod.rs
  - src-tauri/src/sync/mod.rs
  - src-tauri/src/commands/sync.rs
  - src-tauri/src/lib.rs
  - src/components/RulesPanel.tsx
  - src/components/Sidebar.tsx
  - src/components/SyncStatus.tsx
  - src/hooks/useIdleStatus.ts
findings:
  critical: 1
  warning: 6
  info: 4
  total: 11
status: fixed
fixed_at: 2026-10-14T06:00:00Z
fixed_commits:
  - d50331b
  - 39837b9
  - 2407d99
  - d27cf6f
---

# Phase 29: Code Review Report

**Reviewed:** 2026-10-14T05:30:00Z
**Depth:** deep
**Files Reviewed:** 13 (commits 0594d24, bbeadd2, c1bd5bb, b04da56)
**Status:** fixed (all 11 findings addressed — see Fix Record below)

## Summary

Reviewed all Phase 29 changes (rule import/export + IDLE watcher + UI) at deep depth, including cross-module traces (commands → rules_io → queries, idle → SyncWake → SyncGate, frontend hook → backend commands). Tests pass (420 passed, 0 failed). No security vulnerabilities found on the checked axes: import takes a JSON **string** (no file-path handling, so no path traversal), all SQLite writes ride bound parameters, IDLE path issues no FETCH (BODY.PEEK holds trivially), UID-only discipline untouched, `idle.rs` holds no manager/lease reference (dedicated `connect_sync`, structurally tested), migration M21 is forward-only with preserved rows, and no bodies/secrets are logged (only usernames and rule/folder names).

One correctness issue is classified **Critical**: the commit path of `import_rules` is non-atomic (partial writes persist when the loop fails midway, and the duplicate snapshot goes stale within the file). Six warnings and four info items follow. No new dependencies, no SMTP/send changes — constraints hold.

## Critical Issues

### CR-01: Non-atomic import commit — partial writes persist on mid-loop failure, stale duplicate snapshot

**File:** `src-tauri/src/classify/rules_io.rs:237-360`
**Issue:** `import_rules(commit=true)` loops over envelope items calling `upsert_rule_import` per row with no transaction. If any row's DB write fails, the function returns `Err` but all previously upserted rows stay committed — the caller (RulesPanel) reports "Falha ao importar" while the store holds a half-applied file, with no way to know which subset landed. Related: the `known` (id,name) duplicate snapshot is taken once before the loop, so renames earlier in the same file are invisible to later items (a name freed by an earlier same-file update is still falsely reported as duplicate; conversely a name claimed by an earlier same-file insert is only caught via the separate `seen` map, which keys on trimmed-lowercased name while `known` comparison does its own lowercasing — consistent today but the two sources can diverge).
**Fix:**
```rust
// Wrap the commit loop in a transaction: dry-run unchanged,
// commit either fully applies or rolls back.
let tx_guard = ...; // rusqlite Transaction on guard.conn()
for item in &envelope.rules {
    // ...validate against known + already-applied-in-this-tx...
}
// tx.commit() only after the whole file validates/applies;
// on any error, drop without commit and return Err with zero writes.
```
At minimum, validate the entire file first (collect rows), then apply inside `conn.execute_batch("BEGIN IMMEDIATE")` / transaction, and refresh the duplicate check against rows already staged in this import.

## Warnings

### WR-01: 29-minute Timeout re-IDLE emits a spurious advisory wakeup

**File:** `src-tauri/src/imap/idle.rs:302-309`
**Issue:** `run_with` treats `CycleOutcome::ReIdle` (29-min server silence, explicitly "no wakeup" in `drive_cycle`) the same as `Woke`: it increments the wake counter and invokes `on_wake`, which emits `idle-wakeup` → the frontend runs a full sync pass. Every silent 29-minute period therefore triggers a pointless sync and inflates `wakeups_requested`, contradicting the documented "timeout re-IDLEs without wakeup" contract asserted in tests.
**Fix:**
```rust
CycleOutcome::Woke => {
    attempt = 0;
    self.wakes.fetch_add(1, Ordering::SeqCst);
    (self.on_wake)();
}
CycleOutcome::ReIdle => {
    attempt = 0;
    // No wakeup: server silence, connection stays healthy.
}
```

### WR-02: No size/count cap on rule import (frontend or backend) — unbounded file read

**File:** `src/components/RulesPanel.tsx:298-302`, `src-tauri/src/classify/rules_io.rs:237-251`
**Issue:** `onFilePicked` calls `file.text()` with no size check, and `import_rules` parses and loops over an unbounded `envelope.rules` vec while holding the store Mutex the entire time. A multi-MB/malformed-huge JSON file blocks the UI thread (read + double invoke round-trip) and starves every other store user (sync worker included) for the duration. Envelope version is checked, but rule count and byte length are not.
**Fix:**
```tsx
// RulesPanel.tsx — reject absurd files before touching the backend:
const MAX_IMPORT_BYTES = 1 << 20; // 1 MiB, rules are metadata-only
if (file.size > MAX_IMPORT_BYTES) { setMessage("Arquivo grande demais para importar."); return; }
```
```rust
// rules_io.rs — defense in depth:
const MAX_IMPORT_RULES: usize = 500;
if json.len() > 1_048_576 { return Err("rules file too large".into()); }
if envelope.rules.len() > MAX_IMPORT_RULES { return Err("too many rules (max 500)".into()); }
```

### WR-03: IDLE capability gate reads only the FIRST account's atom

**File:** `src-tauri/src/classify/rules_io.rs:393-403`
**Issue:** `read_idle_atom` does `.get("accounts")?.as_array()?.first()?` — in a multi-account profile only account[0]'s `IDLE` atom is consulted. The active account may differ, so the gate can refuse to spawn on an IDLE-capable connection (stuck in poll-only) or, worse, allow spawn based on another account's capabilities. M1 is single-account so impact is latent, but the function is the single gate for the whole feature.
**Fix:**
```rust
// Look up the atom for the active account id (or require ALL accounts
// to advertise IDLE); at minimum iterate instead of .first():
v.get("accounts")?.as_array()?.iter()
    .find(|a| a.get("id") == Some(&active_id))?
    .get("atoms")?.get("IDLE")?.as_bool()
```

### WR-04: Accept-blocked-on-any-error contradicts the "ignored rules" copy and blocks valid subsets

**File:** `src/components/RulesPanel.tsx:329-331`, `src/components/RulesPanel.tsx:702`
**Issue:** The preview modal labels errors "estas regras serão ignoradas" (these rules will be skipped — implying partial import of the valid subset, which is exactly what the backend commit path does: `continue` on error items). But `acceptImport` early-returns and the Accept button is disabled whenever `preview.errors.length > 0`, so a file mixing 1 bad + 20 good rules can never be committed without hand-editing the file. Either the copy or the gating is wrong.
**Fix:** Decide one contract: (a) if partial commit is intended, enable Accept when `imported + updated > 0` and surface per-item skip counts; (b) if all-or-nothing is intended, change the copy to "correct the file — nothing will be imported" and make the backend commit path reject atomically (see CR-01).

### WR-05: `LiveIdleDriver::begin_idle` leaks the half-open session when `idle.init()` fails

**File:** `src-tauri/src/imap/idle.rs:410-421`
**Issue:** `session.idle()` moves the session into `handle`; if `handle.init().await` then fails, the `?` propagates with `handle` (owning the session) dropped in place — no DONE, no LOGOUT. The server holds a half-open connection until its own timeout, and rapid retry loops (backoff floor is ~5s) can accumulate server-side sessions. The `end_idle`-on-`begin`-failure path in `drive_cycle` only helps `ScriptDriver`-style drivers; for the live driver `self.handle` was never stored, so `end_idle` is a no-op.
**Fix:**
```rust
let mut handle = session.idle();
if let Err(e) = handle.init().await {
    release_handle(handle).await; // best-effort DONE + LOGOUT
    return Err(format!("idle init: {e}"));
}
```

### WR-06: Store Mutex held across the entire import (validation + all upserts)

**File:** `src-tauri/src/classify/rules_io.rs:251-252`
**Issue:** `import_rules` holds `store.lock()` from the duplicate snapshot through every row's validation and write. Combined with WR-02 (unbounded file), a large import stalls sync, classify, and every other store consumer. Validation that doesn't need the DB (structure/regex/priority) should run lock-free; only the snapshot reads and the (transacted, per CR-01) write batch need the guard.
**Fix:** Parse + `validate_item_structure` + regex checks before locking; take the lock only for `get_all_rule_names` / folder-tree reads and the commit transaction.

## Info

### IN-01: Dead public helpers — `folder_known` and `import_export_available` have no non-test callers

**File:** `src-tauri/src/classify/rules_io.rs:382-386`, `src-tauri/src/classify/rules_io.rs:415-425`
**Issue:** `grep` shows zero non-test call sites: the import path inlines its own folder queries instead of using `folder_known`, and `import_export_available` (always-`true` stub) is never called by commands or UI. Dead surface that will rot (the inlined queries already duplicate `folder_known`'s logic).
**Fix:** Either route the warn-but-allow checks through `folder_known` and gate the UI buttons on `import_export_available`, or delete both.

### IN-02: Double `idle_status` fetch on every Sidebar mount

**File:** `src/hooks/useIdleStatus.ts:91-96`
**Issue:** The mount effect fires `invoke("idle_start").then(fetch)` and an immediate `fetch()` — two backend round-trips per mount for the same snapshot. Harmless but wasteful on every navigation/sidebar remount.
**Fix:** Fetch once: `void invoke("idle_start").catch(...).finally(() => { if (alive) void fetch(); });` and drop the standalone `void fetch()`.

### IN-03: Snapshot mutex `.lock().unwrap()` in async IDLE paths can panic the watcher task

**File:** `src-tauri/src/imap/idle.rs:213-231` (`drive_cycle`, `set_state`)
**Issue:** A poisoned snapshot mutex (any earlier panic while holding it) turns every subsequent `drive_cycle`/`status_of` read into a fresh panic, permanently killing the watcher task and wedging `idle_status`. Matches existing codebase style, but this is a long-lived background task where a poisoned lock is fatal and silent.
**Fix:** Use `snap.lock().unwrap_or_else(|e| e.into_inner())` for snapshot reads/writes.

### IN-04: `SGE_CAPABILITY_PROFILE` env override can force IDLE spawn from an attacker-chosen file

**File:** `src-tauri/src/classify/rules_io.rs:406-413`
**Issue:** `default_profile_path` trusts `SGE_CAPABILITY_PROFILE` unconditionally. Any process/user able to set the backend's environment can point the gate at a crafted profile with `IDLE:true`, bypassing the poll-only degradation. Local-only, low severity — but the gate is a security-relevant decision (it chooses whether to open a persistent connection).
**Fix:** In release/bundled builds ignore the env var (or only honor it when the file also passes a sanity check, e.g. is inside the app-data dir).

---

## Fix Record (2026-10-14T06:00:00Z)

All 11 findings fixed, verified (`cargo test -p sge --lib`: 425 passed / 0 failed / 1 ignored; `cargo test -p sge --bins`: ok; `npx tsc --noEmit`: clean), committed atomically:

- **CR-01** (`d50331b`): `import_rules` is now validate-all-then-apply: lock-free validation stages rows, commit applies them inside ONE rusqlite transaction (mid-apply DB failure → rollback, zero writes). Duplicate detection uses a single working map seeded from the snapshot and updated as rows stage, so same-file renames free their old name. Commit re-verifies against fresh in-tx state (concurrent-writer race aborts cleanly).
- **WR-01** (`39837b9`): `ReIdle` arm no longer bumps the wake counter nor calls `on_wake` — silent re-IDLE, no spurious sync. Regression test `reidle_cycles_never_fire_on_wake`.
- **WR-02** (`d50331b` backend + `2407d99` frontend): 1 MiB / 500-rule caps enforced before parsing/locking (backend) and before `file.text()` (frontend).
- **WR-03** (`d50331b`): `read_idle_atom` consults every account — spawn allowed when any account advertises `IDLE:true`; absent atoms abstain (fail-closed). Multi-account regression test added.
- **WR-04** (`2407d99`): Accept enabled when `imported + updated > 0` — partial-valid-subset import works, invalid entries skipped with reasons, matching the "serão ignoradas" copy.
- **WR-05** (`39837b9`): `begin_idle` releases the handle best-effort (DONE + LOGOUT) when `init()` fails — no half-open server session.
- **WR-06** (`d50331b`): store Mutex held only for the snapshot read and the commit transaction; all structural validation runs lock-free.
- **IN-01** (`d50331b`): `folder_known` deleted (its exact-match check was weaker than the inlined display-aware check); `import_export_available` retained as the documented UI-gate call site with rationale in its doc comment.
- **IN-02** (`d27cf6f`): mount effect fetches once via `idle_start().then(fetch)`; standalone duplicate `fetch()` removed.
- **IN-03** (`39837b9`): snapshot locks in `drive_cycle`/`set_state`/`IdleWatcherHandle::snapshot` use `unwrap_or_else(|e| e.into_inner())`.
- **IN-04** (`d50331b`): `SGE_CAPABILITY_PROFILE` honored in debug builds only; release/bundled builds ignore it.

No new deps, no SMTP/send changes, BODY.PEEK-only and read-only IDLE verbs untouched (structural test still green), forward-only migrations untouched, no bodies/secrets logged.

_Fixed: 2026-10-14T06:00:00Z_
_Fixer: the agent (gsd-code-fixer)_

---

_Reviewed: 2026-10-14T05:30:00Z_
_Reviewer: the agent (gsd-code-reviewer)_
_Depth: deep_
