---
phase: 05
slug: keyring-packaging
status: approved
mode: mvp
created: 2026-10-03
---

# Phase 5 — Context

## Goal
User launches straight into mail and can install the app on a clean Linux machine.

## Constraints
- Credentials persist in OS keyring (never plaintext) — keyring 3 + sync-secret-service
- Auto-connect on next launch (load creds from keyring, connect_account, start_sync)
- Linux-only bundle: .deb + .AppImage
- Keyring-less machines get guided setup, never silent plaintext fallback or login loop

## Key Findings
- `load_credentials()` already exists as Tauri command
- `save_server_config()` + `save_credentials()` already exist
- LoginForm already loads creds on mount (line 70-95) and handles keyring-unavailable
- But auto-connect is NOT implemented — user still presses Connect
- `load_server_config()` is a KeyringStore method but NOT a Tauri command — need to expose it
- `tauri.conf.json` already has `bundle.targets: ["deb", "appimage"]` + icon paths
- `bundle.active = true` — packaging is already configured

## Architecture
- New: `load_server_config` Tauri command (expose existing KeyringStore method)
- App.tsx: useEffect on mount → load_credentials + load_server_config → auto-connect
- Icons: verify they exist at src-tauri/icons/
- Bundle: verify `cargo tauri build` works

## Success Criteria
1. Credentials persist in keyring; app auto-connects on launch
2. Installable .deb or .AppImage on clean Ubuntu
3. Keyring-less machine → guided setup, no silent plaintext
