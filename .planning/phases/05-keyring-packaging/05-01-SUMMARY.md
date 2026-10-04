# Plan 05-01 Summary — Auto-connect on Launch

## Completed

- Added `load_server_config()` Tauri command in `lib.rs` — exposes the existing
  `KeyringStore::load_server_config` method. Returns `Ok(None)` when keyring is
  unavailable (StoreUnavailable) — never a silent plaintext fallback (WR-02).
- Registered `load_server_config` in `invoke_handler`.
- Updated `App.tsx`:
  - Auto-connect `useEffect` on mount: `load_credentials` → `load_server_config` →
    `connect_account` → `start_sync` → `setConnected(true)`.
  - `autoConnecting` state shows "Connecting to your mail…" instead of login form.
  - If credentials missing, keyring unavailable, or auto-connect fails → login form
    shown (user can retry manually).
  - Added `SavedCredentials`, `ServerConfig`, `ConnectSummary`, `SecurityMode` types.

## Verification
- `cargo check` ✓
- `cargo test --lib` — 63 passed ✓
- `tsc --noEmit` ✓
- `eslint` ✓
