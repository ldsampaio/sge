# Phase 15: Sidecar Packaging Spike - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The Laya classifier runs as a bundled offline sidecar the app can spawn, health-check, and kill — packaging risk retired before any classification UX exists. Hello-world `/health` only; no classification logic ships in this phase. Deliverables: frozen sidecar binary (PyInstaller, CPU-only torch), Tauri externalBin + resources + shell:allow-execute wiring, Rust supervision (spawn in setup(), kill-on-exit, health probe, crash-restart), measured cold-start, pinned checkpoint revision.

</domain>

<decisions>
## Implementation Decisions

### Binary naming + freeze toolchain
- Binary name `sge-laya-<target-triple>` (STACK convention; Tauri externalBin requires the triple suffix).
- Freeze with PyInstaller from `laya[serve]==0.4.2` with CPU-only torch wheel (single biggest size lever).
- Freeze script lives at `sidecar/build-sidecar.sh` (+ `sidecar/requirements.txt` pinning `laya[serve]==0.4.2`), reproducible from a clean venv via `uv`.
- Weights shipped as Tauri `resources` (NOT inside the frozen binary); binary resolves weights dir at runtime via Tauri resource path.

### Weights + checkpoint pin
- Multilingual checkpoint only (`LAYA_MAX_LOADED=1`); English checkpoint never ships.
- Checkpoint Hub revision pinned by commit hash in the build script; sha256 recorded alongside.
- Preload at sidecar start (env-gated) so first classification never pays load latency in a later phase.

### Tauri wiring
- `bundle.externalBin` entry for the sidecar binary; `bundle.resources` for weights; `shell:allow-execute` scope pinned to the sidecar argv (loopback host/port only).
- `tauri-plugin-shell = "2"` added to Cargo deps; sidecar spawned in `setup()`, killed on exit.
- Loopback-only env: `LAYA_HOST=127.0.0.1`, ephemeral port, per-boot API key generated in Rust and passed to the sidecar (never logged).

### Supervision + health contract (hello-world only)
- Rust sidecar module: spawn, `/health` probe with timeout, restart-on-crash with backoff, kill-on-exit (no orphans).
- Cold-start happens backgrounded at launch; sync and UI never block on it. Cold-start seconds measured and documented (dev AND built bundle).
- HTTP contract probed is `GET /health` only; `POST /v1/systemone` shape recorded from docs for Phase 17 but NOT called in this phase.
- Health probe green required in dev AND in the built Linux bundle.

### Measurement + exit gates
- Record: installed size delta, cold-start seconds (dev + bundle), RAM residency, `/health` latency.
- Fallback trigger armed (not executed): if size/cold-start breaks Linux-packaging limits, a later stretch spike may migrate the interior to `laya[onnx]` inside the same externalBin contract. Never hosted inference.

### Agent Discretion
- Exact PyInstaller spec details (hidden imports, data-file collection) — resolve at build time from errors.
- Ephemeral port selection mechanism (OS-assigned vs fixed loopback port with conflict retry).
- Restart backoff parameters.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/main.rs` / `lib.rs` — Tauri setup() hook site for sidecar spawn; plugin registration site.
- `src-tauri/src/store/` (M2-M11 migration template) — pattern reference only; no migration in this phase.
- `src-tauri/capabilities/` — capability file where shell:allow-execute scope must be added.

### Established Patterns
- Sync modules use lease + reconnect-retry + drain; sidecar supervision mirrors the shape (spawn, probe, restart) but owns no IMAP session.
- Backend errors map to plain language with no secret leakage — per-boot key must never appear in logs or error strings.
- BODY.PEEK-only / UID-only IMAP invariants untouched — this phase adds a third transport (loopback HTTP) and no IMAP verbs.

### Integration Points
- `tauri.conf.json` (bundle.externalBin, bundle.resources) + `capabilities/*.json` (shell:allow-execute).
- `Cargo.toml` (tauri-plugin-shell 2, reqwest for the health probe).
- Frontend: none in this phase.

</code>

<specifics>
## Specific Ideas

- Research SUMMARY.md Sidecar Decision Matrix is FINAL: POST /v1/systemone (+ /batch), PyInstaller + CPU torch, weights as resources, LAYA_MAX_LOADED=1. Do not re-litigate.
- Live gate: health green in dev AND built bundle (or documented reason + dev-green if bundle build infeasible here).
- Auto root reservation, taxonomy, redaction: explicitly NOT this phase (Phases 16-18).

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>
