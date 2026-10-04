# Milestone 1 — Audit & Summary

## Milestone: Read-only INBOX Viewer
**Branch**: main → origin/main (ahead 5 after this session)
**Status**: COMPLETE

## Phase Audit

| Phase | Plans | Success Criteria | Status |
|-------|-------|-----------------|--------|
| 1. Scaffold + Connection | 3/3 | 3/3 | ✅ Complete |
| 2. Sync Engine + Local Store | 3/3 | 3/3 | ✅ Complete |
| 3. Mailbox UI Shell + Search | 3/3 | 4/4 | ✅ Complete |
| 4. Reader + Attachments | 3/3 | 4/4 | ✅ Complete |
| 5. Keyring + Packaging | 3/3 | 3/3 | ✅ Complete |

## Verification Matrix

| Check | Result |
|-------|--------|
| `cargo check` | ✅ 0 errors, 0 warnings (excluding pre-existing edition-lint config) |
| `cargo test --lib` | ✅ 63 passed, 0 failed, 1 ignored |
| `tsc --noEmit` | ✅ 0 errors |
| `eslint src/ --ext .ts,.tsx` | ✅ 0 errors |
| `vite build` | ✅ dist/ produced |
| Icons present | ✅ All 5 required (32x32, 128x128, 128x128@2x, .icns, .ico) |

## Key Decisions

1. **IMAP read-only**: `BODY.PEEK[]` only — never sets `\Seen` (verified in `Session::fetch_body`)
2. **Headers-first sync**: Phase 2 stores headers only; Phase 4 fetches bodies on-demand
3. **HTML sanitization**: `ammonia` strips `<script>`, `<iframe>`, `<object>`, event handlers
4. **Defense-in-depth rendering**: ammonia + `sandbox=""` iframe (no allow-scripts) + CSP hardening
5. **Credentials**: OS keyring only (sync-secret-service on Linux); zeroize for passwords; no plaintext files
6. **Incremental sync**: UIDVALIDITY-guarded; validity bump triggers full resync
7. **Packaging**: deb + appimage targets pre-configured; build via `npm run tauri build`

## Constraints Verified

| Constraint | Status |
|-----------|--------|
| IMAP leaves mail on server (BODY.PEEK only) | ✅ |
| No SMTP/send in M1 | ✅ |
| INBOX only | ✅ |
| Credentials only in OS keyring | ✅ |
| Linux-only bundle (deb + appimage) | ✅ |
| Headers-first sync | ✅ |
| UIDVALIDITY-guarded incremental | ✅ |
| HTML sanitized (ammonia) + sandboxed iframe | ✅ |
| Tauri v2.12, async-imap 0.11, mail-parser 0.11 | ✅ |
| rusqlite 0.37 + WAL mode | ✅ |
| keyring 3 + sync-secret-service | ✅ |
