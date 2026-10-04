# Phase 5 — UI Design Contract: Keyring + Packaging

## Overview
No new UI components needed. Changes are to `App.tsx` (auto-connect state)
and the LoginForm already handles keyring-less fallback.

## Auto-connect flow
1. App loads → `load_credentials()` from keyring
2. If creds exist → `load_server_config()` → `connect_account()` → `start_sync()`
3. If successful → transition to MailboxView
4. If keyring unavailable → show LoginForm with hint (already implemented)
5. If auto-connect fails → show LoginForm (user retries manually)

## States in App.tsx
- **Auto-connecting**: spinner + "Connecting…" message, no login form
- **Login form**: shown when no credentials or auto-connect failed
- **Mailbox**: shown when connected

## Packaging
- No UI changes needed
- `tauri.conf.json` bundle config already set (deb + appimage)
- Verify icons exist, verify build
