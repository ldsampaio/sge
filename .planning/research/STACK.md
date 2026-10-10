# Stack Research — v1.3 Auto-Classify (Laya sidecar)

**Domain:** on-device email classification from Rust + Tauri v2 Linux desktop app
**Researched:** 2026-10-10
**Confidence:** HIGH (Laya 0.4.2 pyproject + README + PyPI + HF model pages read live; Tauri v2 sidecar/resources docs verified; crates.io versions checked)

> Scope: ONLY what to ADD/CHANGE for Laya classification. Existing stack
> (Tauri v2.12, async-imap 0.11, lettre 0.11, rusqlite 0.37, keyring 3, …)
> is NOT re-researched. See prior STACK.md (v1.2) for the base.

## TL;DR

| Need | Decision |
|------|----------|
| Inference engine | **Bundled offline sidecar: `laya[serve]` 0.4.2 (Python) speaking `POST /v1/systemone` on `127.0.0.1`** |
| Checkpoint | **Multilingual only** (`convaiinnovations/laya-multilingual`, 322M params, `model.safetensors` 644 MB), pinned Hub revision, shipped as Tauri `resources` |
| Tauri ↔ sidecar bridge | **ADD `tauri-plugin-shell = "2"` (stable 2.4.0) + `reqwest = "0.12"`** — Rust backend spawns sidecar, calls it over localhost HTTP |
| Sidecar packaging | **PyInstaller single binary** (`src-tauri/binaries/sge-laya-<target-triple>`) with CPU-only torch; model files as `resources`, NOT inside the binary |
| Frontend | **No new npm deps** — classification UI goes through existing Tauri `invoke`, never touches the sidecar directly |

`Cargo.toml` delta is **two lines** (`tauri-plugin-shell`, `reqwest`).
`tauri.conf.json` delta is `externalBin` + `resources` + one capability entry.

---

## 1. Recommended stack additions

### Core additions

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `laya` (PyPI, extra `serve`) | **0.4.2** (current, verified on PyPI 2026-10-10; `requires-python >=3.10`) | Typed-decision inference (`choice`/`score`/`noul`, single forward pass) + `laya-serve` HTTP server | This IS the classifier the milestone chose. The `serve` extra is the only integration surface the Rust backend needs: one Jev-compatible endpoint, batch route included. Zero-shot fits the "no fine-tuning in v1.3" constraint. Apache-2.0. |
| `laya-multilingual` checkpoint | Hub repo `convaiinnovations/laya-multilingual`, **pin a revision hash** | pt-BR + 100-language classification (mmBERT-base 322M, ctx 1024, up to 8192 via `max_len`) | pt-BR mail (incl. short Latin-script subjects like "Quero cancelar") routes to `multilingual` by default since Laya 0.4.0 — English checkpoint can't be trusted for Portuguese. 322M is also ~2x faster than the 421M English checkpoint (121 ms vs 146 ms on MPS; ~192 ms on CPU per predict). Ship ONLY this checkpoint: all three = ~2.3 GB, multilingual alone = **~678 MB repo / 644 MB weights**. `LAYA_MAX_LOADED=1` keeps exactly one resident. |
| `tauri-plugin-shell` | **2.4.0** (stable, crates.io; requires `tauri ^2.12` — matches the pinned Tauri v2.12) | Spawn + supervise the sidecar binary from the Rust backend (`ShellExt::sidecar`) | This is THE Tauri-blessed sidecar mechanism (`externalBin` + `sidecar()` + `shell:allow-execute` capability). Spawning from Rust (not JS `Command`) keeps mail bytes on the backend path the sync pipeline already uses, and lets the existing `SessionManager`-style lease/guard patterns supervise the child (kill on app exit is built into the plugin). |
| `reqwest` | **0.12** (async, default features; plain `http://127.0.0.1` — no TLS feature decision needed) | Rust backend → sidecar HTTP client | Tauri runs on tokio; `reqwest` async fits without a new runtime. Only needs `json` (serde already in tree). Alternatives (`ureq` blocking, raw `tokio::net`) either fight the runtime or reimplement HTTP — no gain for a localhost JSON POST. |
| PyInstaller | current (≈6.x, pin in sidecar build script) | Freeze `laya[serve]` + CPU torch into one `sge-laya` binary for `externalBin` | Tauri `externalBin` needs a **single self-contained executable**, not a venv. PyInstaller one-dir/one-file is the standard freeze for FastAPI+torch sidecars. The alternative (shipping a venv + system python3 dependency) breaks the "no extra user installs" promise of a `.deb`/AppImage. |
| `torch` CPU-only wheel | `>=2.0` per laya deps; install via `--index-url https://download.pytorch.org/whl/cpu` at sidecar build time | Inference runtime inside the sidecar | Laya's verified numerics path (`verify/numerics_check.py`, SDPA vs eager) runs on torch. GPU builds are dead weight on a Linux email client (no CUDA target in this milestone, bundle targets are deb/AppImage for commodity machines). CPU-only torch ≈ 200 MB vs ≈ 800 MB+ CUDA — the single biggest sidecar-size lever. |

### Sidecar Python dependency closure (exact)

From the live `pyproject.toml` of laya 0.4.2 — freeze these in the sidecar build, nothing else:

```
laya[serve]==0.4.2
  torch>=2.0.0            # CPU-only wheel via pytorch cpu index
  transformers>=4.48.0
  safetensors>=0.4.0
  huggingface_hub>=0.20.0 # only used at BUILD time to bake the checkpoint; offline at runtime
  numpy>=1.20.0
  fastapi>=0.110.0        # serve extra
  uvicorn>=0.27.0         # serve extra
  python-multipart>=0.0.9 # serve extra
```

Do NOT install `fast`, `mcp`, `langchain`, `crewai`, `llamaindex`, `structured`, or `onnx` extras
(see §4 — each rejected with a reason).

### Supporting: Tauri config changes (no new deps, but part of the stack)

| Change | Where | Why |
|--------|-------|-----|
| `bundle.externalBin: ["binaries/sge-laya"]` + binary at `src-tauri/binaries/sge-laya-x86_64-unknown-linux-gnu` | `tauri.conf.json` | The sidecar contract: Tauri looks up `<name>-<target-triple>` at bundle time. Linux-only milestone ⇒ exactly one triple to build (add `-aarch64-unknown-linux-gnu` only if ARM ship is ever scoped). |
| `bundle.resources: { "<models dir>/": "laya-models/" }` (multilingual checkpoint files only) | `tauri.conf.json` | Model weights (~644 MB) must NOT be frozen inside the PyInstaller binary (would duplicate per build and defeat HF hash verification). `resources` lands them in `$RESOURCE/`; the backend passes the resolved path to the sidecar via env/argv. |
| `shell:allow-execute` (or `allow-spawn`) scope for `sge-laya` + fixed argv | `src-tauri/capabilities/default.json` | Sidecar spawn is denied without it. Pin argv (`--host 127.0.0.1 --port <fixed> --models <resource-dir>`) statically — no dynamic user-controlled args (mail text travels in the POST body, never argv). |
| `LAYA_HOST=127.0.0.1`, `LAYA_PORT=<fixed high port>`, `LAYA_MODELS=multilingual`, `LAYA_PRELOAD=1`, `LAYA_MAX_LOADED=1`, `LAYA_THREADS=<n-phys>`, `LAYA_API_KEY=<random per-boot>` | sidecar spawn env (set by Rust backend) | `127.0.0.1` (never the `0.0.0.0` default) keeps inference loopback-only — the offline/privacy constraint made concrete. Preload avoids first-classify latency spike; `MAX_LOADED=1` matches single-checkpoint residency; per-boot API key means even other local processes can't casually drive the classifier. |

### Suggested Rust integration shape (for planners, not prescriptive code)

- New module (e.g. `classify/`) owning: sidecar spawn (via `ShellExt::sidecar("sge-laya")`), `/health` readiness probe with backoff, `POST /v1/systemone` + `/v1/systemone/batch` (batch for the whole-account pass — capped at 64 states/server call, `LAYA_MAX_BATCH_TOKENS` splits internally), LRU of taxonomy→questions JSON.
- Reuse the established patterns: spawn off-runtime like IMAP's dedicated blocking thread; serialize through a lease like `SessionManager::lease_for` (torch CPU inference is effectively single-flight per process — concurrent sync + batch classify must queue, not pile on); never log state/question bodies beyond lengths (sensitive-data rule).
- Input budget: multilingual default ctx is 1024 tokens; pass `max_len=8192` only for long threads (costs ~1.7 s/4k tokens) — default path stays short (subject + snippet) so single-mail classify stays in the ~200 ms CPU band.

---

## 2. Installation

```bash
# --- Rust backend (the only Cargo delta) ---
cargo add tauri-plugin-shell@2 reqwest@0.12
# then register: tauri::Builder::default().plugin(tauri_plugin_shell::init())

# --- Sidecar build environment (build-time only, never shipped) ---
python3.12 -m venv .sidecar-build && .sidecar-build/bin/python -m pip install \
  --index-url https://download.pytorch.org/whl/cpu \
  "torch>=2.0.0" \
&& .sidecar-build/bin/python -m pip install "laya[serve]==0.4.2" pyinstaller
# bake checkpoint once, with revision pin:
#   HF_HUB_CACHE=./models .sidecar-build/bin/python -c \
#     "from laya import Router; Router(models={'multilingual': '<pinned snapshot path>'}, preload=True)"
# freeze:
#   .sidecar-build/bin/pyinstaller --onefile --name sge-laya sidecar_main.py
#   mv dist/sge-laya src-tauri/binaries/sge-laya-x86_64-unknown-linux-gnu
```

`sidecar_main.py` is a ~20-line shim calling `laya.serve` (or `create_app(router)` with a
local-models `Router`) under uvicorn bound to the argv-supplied host/port. No frontend change:
no `npm install` needed.

---

## 3. Alternatives considered (sidecar vs ONNX vs HTTP — the downstream question)

| Recommended | Alternative | When to use alternative instead |
|-------------|-------------|---------------------------------|
| **Python sidecar (`laya[serve]` + torch CPU, `externalBin`)** | — (recommended) | Default for v1.3: official serving path, full `Router` parity (routing, `lang_guess`, `max_len`, `min_confidence`, batch), numerics verified upstream. |
| Recommended | `laya-ts` ONNX in-JS (`encoder.onnx` + `head.onnx` via `onnxruntime-node`) | Only if the team later wants the classifier in the frontend process. Rejected now: ESM-only, Node-side runtime still needs bundling as a sidecar anyway (same packaging problem, less maturity), `sort_by_length` not ported, first load pulls ~1.3 GB, browser path assumes WebGPU/WASM — strictly worse packaging story than one Python binary. |
| Recommended | Rust-native ONNX (`ort` crate + exported `encoder.onnx`/`head.onnx` + reimplemented Router) | Only as a future slim-down (drops the whole Python runtime; onnxruntime ≈ 50–100 MB vs torch CPU ≈ 200 MB + interpreter). Rejected now: requires reimplementing routing/tokenizer/head-wiring in Rust against an export script (`scripts/export_onnx.py`) whose parity surface is one test file — high risk for zero v1.3 capability gain. |
| Recommended | Python `laya[onnx]` (`laya.onnx_agent`) inside the SAME sidecar | Consider as a v1.3-stretch size optimization if the torch sidecar measures too fat: same process shape, lighter runtime. Not the default because the torch path is the verified reference. |
| Recommended | Hosted inference (TypeSafe Jev API, impossibl endpoint, any cloud) | **Never for mail content** — violates the hard "mail content must never leave the machine" constraint. Usable only for non-mail smoke tests during development, if at all. |
| Recommended | `reqwest` localhost client | `ureq`/blocking client — only if the classifier module ends up fully synchronous outside tokio; currently nothing suggests that. |

**Packaging recommendation, stated plainly:** the `.deb`/AppImage will grow by roughly
**sidecar binary (~250–400 MB with CPU torch) + models resource (~650 MB)** ≈ ~1 GB installed.
That is the honest price of bundled offline ML and the user already chose "bundled over external
service" (PROJECT.md decision). Mitigations if size bites: (a) ship the `.deb` with
`Recommends:`/split package for models — deferred, Linux packaging phase decides; (b) later
migrate the sidecar interior from torch to `laya[onnx]` (same `externalBin` contract, smaller
binary); (c) never "solve" size by switching to hosted inference — that trades megabytes for
the privacy constraint.

---

## 4. What NOT to add

| Avoid | Why | Use instead |
|-------|-----|-------------|
| `laya-client` npm package / `laya-ts` | Both assume either a running HTTP server (client) or a Node ONNX runtime (ts) — the Rust backend already speaks HTTP and there is no Node runtime in the app. Adds a second integration surface for zero capability. | `reqwest` from Rust backend |
| Hosted Jev / `impossibl` API keys, `baseUrl` repointing | Mail bytes leave the machine; kills the milestone's core privacy guarantee. | Loopback sidecar |
| CUDA/GPU torch builds, `laya[fast]` (TileLang) | No GPU target; TileLang is a GPU fast path with its own toolchain. Pure bundle bloat. | CPU-only torch |
| `laya[mcp]`, `[langchain]`, `[llamaindex]`, `[crewai]`, `[structured]` extras | Agent-framework integrations — irrelevant to a deterministic classify-one-mail call. Each drags heavy transitive deps (pydantic, langchain-core, …). | Plain `POST /v1/systemone` JSON |
| Checkpoint fine-tuning toolchain (train scripts, `laya-train`, Kaggle/MPS notebooks) | Explicitly out of scope ("zero-shot + keyword-assisted only", PROJECT.md). | Zero-shot `choice` questions built from the user's taxonomy + keyword pre-filter in Rust |
| `laya-typed-decisions` / English checkpoints in the bundle | English mail is classifiable by the multilingual checkpoint; typed-decisions serves other workflows. Each adds ~800 MB–1 GB for no v1.3 gain. | Multilingual only; `model` field unset (router auto-selects, lands on multilingual for pt-BR) |
| System-Python / pip-at-install dependency | Breaks hermetic `.deb`/AppImage install; version drift between user distro python and `>=3.10` floor. | Frozen PyInstaller binary in `externalBin` |
| New credential/secret storage for the classifier | `LAYA_API_KEY` is an ephemeral per-boot token generated by the backend, not a user secret — keep it in memory only. | Existing keyring path untouched |
| `examples/server.py` as the sidecar | It's a dev playground (serves a GUI + `/predict` API), not the Jev-compatible `laya-serve` surface the client code should target. | `laya-serve` (`laya.serve`) entry point |

---

## 5. Version compatibility

| Package A | Compatible with | Notes |
|-----------|-----------------|-------|
| `laya 0.4.2` | Python 3.10–3.13, `transformers>=4.48`, `torch>=2.0`, `fastapi>=0.110`, `uvicorn>=0.27` | From live pyproject; floor is 3.10 (huggingface_hub 1.x). Build env: use 3.12 (matches upstream `uv venv --python 3.12` guidance). |
| `tauri-plugin-shell 2.4.0` | `tauri ^2.12` (incl. pinned v2.12), Rust ≥1.90 (latest docs; crates.io 2.0.x line said 1.77.2 — use the project's current toolchain, already ≥ that) | JS guest binding `@tauri-apps/plugin-shell` NOT needed (Rust-side spawn only). |
| `reqwest 0.12` | tokio 1 (already via Tauri), serde_json 1 (in tree) | Plain-HTTP localhost: no `rustls`/`native-tls` feature needed — avoids forking the native-tls story from v1.2 STACK. |
| `laya-multilingual` Hub snapshot | `transformers>=4.48` (ModernBERT/mmBERT `rope_parameters` mapping) | Pin the snapshot revision AND `verify/checkpoints.py`-style sha256 in the build script so a Hub re-upload can't silently change weights under a future rebuild. |
| PyInstallermfcp | Linux-only target `x86_64-unknown-linux-gnu` | Single-triple build; the `externalBin` suffix must equal `rustc --print host-tuple`. |

---

## 6. Open risks / probes for planners

1. **Sidecar cold start** — first spawn pays interpreter + checkpoint load (seconds). Mitigation is `LAYA_PRELOAD=1` at app start + `/health` gate before first sync-classify; measure and set the UX timeout from the number, not a guess.
2. **RAM residency** — 322M fp32 ≈ 1.3 GB resident with `MAX_LOADED=1`. `LAYA_IDLE_UNLOAD_SECONDS` can page it out after idle (next classify pays cold load). Decide policy in planning; default to resident (personal daily-use client).
3. **Port collision** — fixed loopback port must be free; backend should probe-and-retry on adjacent ports and pass the chosen one via argv (never let the sidecar pick silently).
4. **Model-resource path resolution** — must work in `tauri dev` AND bundled builds (`PathResolver::resource_dir`); plan a dev fallback to `./models` checkout dir.

## Sources

- `pyproject.toml` @ laya main (v0.4.2, live fetch) — deps, extras, entry points (`laya-serve`)
- Laya README @ main (live fetch) — Router, checkpoints table, `laya-serve` env vars, `laya-ts` packaging (~1.3 GB first load, ESM-only), `setup_laya.sh` (~2.3 GB all checkpoints)
- `laya-ts/README.md` (live fetch) — export/peer-dep shape
- PyPI `laya 0.4.2` page — version currency + extras matrix
- HuggingFace `convaiinnovations/laya-multilingual` file tree — 678 MB repo / 644 MB `model.safetensors`
- Tauri v2 docs: `develop/sidecar` (externalBin + target-triple + capabilities), `develop/resources`, `learn/sidecar-nodejs`, `reference/config` — sidecar/resources contract
- crates.io `tauri-plugin-shell` (stable 2.4.0, requires `tauri ^2.12`) — bridge version
- SGE `src-tauri/Cargo.toml`, `tauri.conf.json` (repo read) — base pins this stacks onto

---
*Stack research for: SGE v1.3 Auto-Classify (Laya bundled offline sidecar)*
*Researched: 2026-10-10*
