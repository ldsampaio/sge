# Phase 15: Sidecar Packaging Spike — Summary

**Status:** nearly complete (frozen binary + bundle gate pending)
**Date:** 2026-10-10 (autonomous run)

## What shipped

- `src-tauri/src/sidecar.rs` — supervision: loopback-only config (non-loopback
  refused), ephemeral port picker, per-boot 256-bit key (`/dev/urandom`),
  spawn/health/restart/kill policy (MAX 3 restarts → Down), `SidecarError`
  plain-language, key never in Display/logs/IPC. 13 unit tests.
- Tauri wiring: `externalBin` (`binaries/sge-laya`), `resources`
  (`sidecar/weights`), `shell:allow-spawn` sidecar-pinned scope,
  `tauri-plugin-shell 2`, spawn in `setup()` backgrounded, kill-on-exit,
  `sidecar_status` command.
- Freeze toolchain: `sidecar/build-sidecar.sh` (venv → CPU torch install →
  revision pin → weights snapshot → offline smoke → PyInstaller),
  `requirements.txt` (`laya[serve]==0.4.2`), `laya_serve_entry.py`,
  `pins.json` (rev `1720e3e3`, sha256 recorded).
- `sidecar/CONTRACT.md` — `/v1/systemone` wire shape verified against the
  installed 0.4.2 source (choice/score/noul, usage, `x_jev_confidence`).
- `sidecar/MEASUREMENTS.md` — numbers below.

## Measurements (dev, unfrozen)

- Cold-start spawn → `/health` 200 (preloaded multilingual, CPU): **6 s**
- `/health`: `loaded:["multilingual"], device:cpu`, Bearer + open both 200
- Live classify (real calls): boleto → **Financeiro 1.0**; prova adiada →
  **Acadêmico 0.58** (correct, sub-0.6 → review bucket; conservative default
  holds), 109 ms warm inference
- Weights: 648M HF cache + 647M runtime checkpoints = ~1.3G on disk
- Frozen binary: pending (PyInstaller running)

## Hard-won findings (do not re-learn)

1. `laya-serve` is ENV-ONLY (no CLI host/port args) — `LAYA_HOST` defaults
   to `0.0.0.0`, must set `127.0.0.1`.
2. Router resolves `multilingual` to the BUNDLE repo, not standalone —
   `LAYA_EXTRA_MODELS='{"multilingual": "<local snapshot dir>"}'` re-point
   is mandatory (documented mechanism).
3. `laya-serve --help` DOWNLOADS the checkpoint (no cheap `--help`).
4. HF downloads land read-only; tauri-build `fs::copy` preserves mode →
   read-only dests poison ALL later builds with EACCES. Fix: `chmod -R u+rw`
   post-download + never `cargo build` mid-download (atomic-swap pattern in
   script header).
5. tauri-build HARD-FAILS on missing externalBin — dev shim committed at
   `src-tauri/binaries/sge-laya-<triple>`, overwritten in place by freeze.

## Deferred

- Frozen cold-start/size/RAM + bundle artifact gate (freeze running).
- `tauri build` Linux bundle leg (env lacks webkit deps? — attempt, record).
