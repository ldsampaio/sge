# Sidecar measurements (Phase 15, Plan 15-03)

## Environment

- Machine: x86_64-unknown-linux-gnu, CPU-only torch 2.14.1+cpu
- laya 0.4.2, multilingual checkpoint rev `1720e3e3` (sha256 `9d628fd9…`)
- HF cache: `sidecar/weights/hub` (648M) + runtime `checkpoints/multilingual` (647M)

## Dev (unfrozen `laya-serve`)

| Metric | Value | Notes |
|---|---|---|
| Cold-start (spawn → first `/health` 200, preloaded) | **6 s** | measured 2026-10-10, local cache |
| `/health` payload | `loaded:["multilingual"], device:cpu` | Bearer + open both 200 |
| First-attempt failure mode | `LocalEntryNotFoundError` without `LAYA_EXTRA_MODELS` | Router wants bundle repo; re-point is mandatory |

## Frozen binary (PyInstaller onefile, CPU torch)

| Metric | Value | Notes |
|---|---|---|
| Binary size | **294 MB** (`sge-laya-x86_64-unknown-linux-gnu`) | measured 2026-10-10 |
| Installed delta | ~294M binary + ~1.3G weights (648M HF cache + 647M checkpoints) | weights ship as resources |
| Cold-start (frozen, preloaded) | **6 s** spawn → `/health` 200 | same as unfrozen — unpack is fast |
| `/health` p50 | **<1 ms** (0.6 ms) | warm |
| Warm inference | **50 ms** (`Financeiro 0.96`, 2-option choice) | loopback |
| `/health` payload | `loaded:["multilingual"], device:cpu` | Bearer enforced |
| Kill | clean, no orphan (pgrep-verified; earlier ORPHAN! was a self-match false positive) | |

## Bundle gate

- Build: `tauri build --debug` running (webkit present: gtk 3.24 + webkit2gtk 4.1)
- [ ] `sge-laya-<triple>` present in built bundle artifact
- [ ] Health green from bundle path
- [ ] Quit → no orphan (covered by kill-on-exit + manual kill test above)

## Fallback-trigger verdict

Pending frozen numbers. Trigger: installed size or cold-start beyond
Linux-packaging limits (`.deb` hosting, <1.3 GB RAM machines) → authorize
`laya[onnx]` interior inside the same `externalBin` contract. Current
signal: 6 s unfrozen cold-start is backgroundable; size TBD.
