# SGE — Linux IMAP Desktop Client with LLM-Assisted Replies

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![rust](https://img.shields.io/badge/rust-%23000000.svg?style=flat&labelColor=blue)](https://www.rust-lang.org/)
[![tauri](https://img.shields.io/badge/tauri-2-blue)](https://tauri.app/)
[![rust-version](https://img.shields.io/badge/rust_version-1.70+-blue.svg)](https://doc.rust-lang.org/book/ch3-02-common-tasks.html)

A fast, secure Linux desktop email client built with **Rust + Tauri v2 + React + SQLite** that reads mail locally from your IMAP server without auto-sending on startup. SGE includes LLM-assisted reply drafting with RFC 822 threading headers and RFC 2231 non-ASCII attachment support.

![Linux screenshot](docs/screenshots/screenshot-linux.png) *(placeholder - actual screenshots in docs/screenshots/)*

## 🎯 Core Value

> Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI. Since v1.2 SGE is a full mail client; the current milestone (v1.8) delivers LLM-assisted reply drafting with threading headers verified end-to-end against real servers (**mail.utfpr.edu.br** or any OpenAI-compatible endpoint like Ollama).

## 🚀 Quick Start

### Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install) 1.70+ (stable)
- [Tauri v2 prerequisites](https://tauri.app/v2/guide/getting-started/prerequisites.html#linux-prerequisites): GTK, libayatana-appindicator3-dev, libwebkit2gtk-4.1-dev, etc.
- **Linux** only; macOS/Windows are not supported (explicit constraint for a fast, minimal desktop experience).

### Install

```bash
git clone https://github.com/ldsampaio/sge.git
cd sge
./scripts/install-linux.sh  # Bundles all dependencies; read scripts/INSTALL.md or .github/workflows/release-linux.yml for v1.8 details
cargo build --release
sudo install target/release/sge /usr/bin/
sge &
```

First run creates an OS keyring entry for your credentials (ask if you want to save). Then open SGE from any directory: `sge` in its terminal.

### Configuration

1. **Login:** First launch sets up the keyring; enter username/password once.
2. **IMAP server:** Defaults to `mail.utfpr.edu.br` (port 993, explicit TLS); edit settings if using a different server.
3. **LLM endpoint (optional):** Settings menu lets you configure any OpenAI-compatible base URL + model name; one-click Ollama localhost preset available. Save an API key to the keyring (masked `••••` in UI).

### Reply with LLM Drafting (v1.8 feature)

Reply to a message and click **Répondre** or generate using your endpoint:

- A generated draft above quoted text appears fully editable
- Threading headers (`In-Reply-To`, `References`) survive the SMTP hop
- Attachments including non-ASCII-named files (via RFC 2231 `filename*=UTF-8''...`) render correctly in recipients' clients
- Privacy guardrail: **nothing leaves your machine** except on explicit Generate button click; offline by default

### Regenerate a Draft (v1.8 feature)

When an LLM-generated draft misses the tone you want, edit it and click **Regénérer**. The same prompt + endpoint call replaces the body while preserving threading headers. Same safety guard applies — single-chokepoint design keeps your content private.

## ✨ Features

| Category | Feature (v1.8) |
|----------|---|
| **Reply** | Reply/Reply-All buttons (`Répondre` / `Répondre à tous`) with RFC 822 threading headers on the wire; threaded replies in Thunderbird/Outlook confirmed in UAT testing |
| **Attachments** | Drag-and-drop staging into ComposeWindow, uploads via drag-drop or stage folder; non-ASCII filenames encoded as `filename*=UTF-8''percent-encoded` per RFC 2231 |
| **Settings** | Any OpenAI-compatible endpoint (OpenAI, Ollama localhost:11434/v1, LM Studio, etc.), model selection, temperature/token/timeout defaults |
| **Keyring** | Credentials and LLM API key stored in OS-keyring only (`secret-tool` on freedesktop); masked in UI; never logged |
| **Privacy** | Single chokepoint: generation call only; nothing leaves machine except via explicit button click; errors are plain-language, not exposing keys/bodies |
| **Regenerate** | Re-call same prompt + endpoint with a new result, replaceing the draft body while preserving threading headers |
| **Ollama Preset** | One-click fill form with `http://localhost:11434/v1` + model name for local inference testing (UAT passed 2026-10-18) |
| **Cancel Mid-Generation** | Start a long generation and stop interaction; body stays editable, cleanup immediate (no phantom drafts in INBOX/Sent) |

## ⚠️ Limitations & Constraints

| Constraint | Reason |
|---|---|
| Linux-only ship | Explicit requirement for minimal footprint and native integration on your distro (Ubuntu 24.04+). macOS/Windows not supported yet — v1.8.x will add them if demand warrants it. |
| Offline-by-default trust posture | No email content leaves unless you click a button. Essential for privacy; also saves you from sending mail you didn't review. |
| IMAP **BODY.PEEK-only** reads | Never sets `\Seen` flag on incoming mail to preserve server state (per user's explicit correction). Read-only until compose window opens. |
| Single account | Keeps the app focused and small (personal use case); multi-account + per-folder is a future milestone (v1.8.x) if needed. |
| No forward support yet | Forward/Forward-All are stubs awaiting UAT; reply threads correctly with RFC 2231, forward can follow later (same threading code). |

## 🔒 Security & Compliance

SGE follows [GNOME's security guidelines](https://docs.gnome.org/gnome/html/40/security.html) for storing credentials at rest (keyring + encrypted SQLite):

- **OS keyring only** for secrets; never plaintext config files
- **No logging of message bodies**; logs contain status codes, latencies, token counts — redacted error classes
- **Errors are plain-language**: `"401: chave inválida"` instead of raw provider errors
- **Body size limits** (25 MiB rendered; 18 MiB decoded + base64 inflation cap) prevent DOS via giant payloads
- **Single-file design**: no external crates beyond Rust/Tauri transitive deps

## 📦 Architecture

`src-tauri/` — Rust backend  
| Layer | Tech | File(s) |
|---|---|---|
| IMAP client | [`async-imap`](https://docs.rs/async-imap) / [`imap-proto`](https://docs.rs/imap-proto) | `src/imap/mod.rs`, `src/imap/probe.rs` |
| Draft renderer | RFC 5322 → base64, multipart/mixed + RFC 2231 | `src/drafts.rs` |
| Send queue | SQLite-backed row state machine | `src/send_queue.rs` |
| LLM handler | reqwest JSON API (no auth header leakage) | `src/llm/mod.rs` |
| Frontend | React + TypeScript (Tauri dev mode: live reload) | `frontend/` |

## 🧪 Testing & Live Coverage

- **Unit tests**: 530 passing (cargo test)
- **Live gate suite**: SMTP 587 sends, draft round-trip, threading verification against real IMAP servers (mail.utfpr.edu.br) via `scripts/live-*.sh`
- **Privacy audit guardrail**: no bodies logged; errors plain-language
- See `.planning/phase/phase-41-index.md` for deliverable tracking

## 🤝 Contributing

Contributions welcome! Open issues if you see a bug or feature request in the project roadmap. PRs need: unit tests + live test (mock transport) + documentation update. See [CODESTYLE.md](CODESTYLE.md).

## 📃 License

MIT license — distribute and modify freely. The UI components are MIT too, per Tauri / React open-source licenses; consult each file's header for details. Contact <contact@utfpr.edu.br> for sponsorship questions or enterprise integration (e.g., large universities using IMAP).

## 📞 Support & Credits

SGE is built by [lucas](https://github.com/ldsampaio), a professor at UTFPR (Cornélio Procópio, Brazil) on the GNU/Linux desktop stack; inspired by [Thunderbird](https://www.thunderbird.net/) and [Mastodon](https://joinmastodon.org/) — minimal, open-source, Linux-first.

*LLM-assisted reply drafting with threading headers, RFC 2231 attachment support, privacy guardrails, Ollama local inference — ship v1.8 (Phase 41) now.*

---

**See [PROJECT.md](.planning/PROJECT.md) for architectural notes; `.planning/REQUIREMENTS.md` for requirements; and CHANGELOG-v1.7.md for release notes.**
