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

## Frozen binary (PyInstaller)

| Metric | Value | Notes |
|---|---|---|
| Binary size | TBD (freeze running) | onefile, CPU torch |
| Installed delta | TBD | |
| Cold-start (frozen) | TBD | expected ≥ unfrozen (unpack) |
| RSS after preload | TBD | est. ~1.3 GB (322M-fp32 residency) |
| `/health` p50 | TBD | |

## Bundle gate

- [ ] `sge-laya-<triple>` present in built bundle artifact
- [ ] Health green from bundle path
- [ ] Quit → no orphan (pgrep)
- [ ] Kill → supervisor restarts → green (≤3 then Down)

## Fallback-trigger verdict

Pending frozen numbers. Trigger: installed size or cold-start beyond
Linux-packaging limits (`.deb` hosting, <1.3 GB RAM machines) → authorize
`laya[onnx]` interior inside the same `externalBin` contract. Current
signal: 6 s unfrozen cold-start is backgroundable; size TBD.
