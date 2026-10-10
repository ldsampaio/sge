# Laya sidecar HTTP contract (Phase 15 record, Phase 17 consumer)

Source: `laya==0.4.2` installed package (`laya/serve.py`, 1393 lines) read
2026-10-10. This file is the verified contract the Phase 17 `bridge.rs`
must be written against. Do NOT use the older `/predict` dev-example shape.

## Server

- Entry: `laya-serve` → `laya.serve.main()` → uvicorn. **Env-only config**
  (no CLI host/port args). Heavy imports (fastapi, uvicorn, torch via
  Router) deferred — `import laya.serve` stays cheap, touches no GPU.
- Single-worker pool: one large request starves `/health`. Prefer the batch
  route for bulk work; keep single requests narrow.

## Env (verified names)

| Var | Meaning | Our value |
|---|---|---|
| `LAYA_HOST` | bind address (default `0.0.0.0` — must override) | `127.0.0.1` |
| `LAYA_PORT` | bind port (default 8000) | ephemeral per boot |
| `LAYA_API_KEY` | if set, require `Authorization: Bearer <it>` | per-boot random |
| `LAYA_MODELS` | comma list to preload (`english,multilingual,…`) | `multilingual` |
| `LAYA_DEFAULT_MODEL` | fallback when a state carries no language evidence | `multilingual` |
| `LAYA_MAX_LOADED` | checkpoints resident at once | `1` |
| `LAYA_PRELOAD` | build checkpoints at startup, not lazily (default 1) | `1` |
| `LAYA_IDLE_UNLOAD_SECONDS` | unload after idle (default 0 = off) | unset (resident) |
| `LAYA_MAX_TOKEN_BUDGET` | server ceiling on per-request `max_len`/`head_max_len` | default 8192 |
| `LAYA_EXTRA_MODELS` | JSON `{name: repo-or-local-path-or-[repo,subfolder]}` | unset |
| `LAYA_DEVICE` | torch device | default (auto → CPU here) |
| `LAYA_THREADS` | torch thread limit | default |
| `LAYA_LOG_LEVEL` | uvicorn log level | default |
| `LAYA_AUTO_TASK` | auto-route to typed-decisions checkpoint | default 0 |
| `HF_HUB_OFFLINE=1` + `HF_HOME=<resources>/weights` | offline weights (our addition, not laya's) | set at spawn |

## Routes

- `GET /health` — liveness is open even with `LAYA_API_KEY` set; the full
  payload (checkpoints, revision SHAs, device) needs the Bearer header.
  Our probe sends the header and accepts any 2xx.
- `POST /v1/systemone` — single request: `{states, questions}` with typed
  `choice` / `score` / `noul` questions. Response has exactly three
  top-level Jev fields per answer kind:
  - `choice`: `type` + `choice` + `probabilities` + `confidence`
  - `score`: `type` + `score` + `probabilities` + `confidence` + `legend`
  - `noul`: `type` + `noul`
  plus `usage` (`{input_tokens, …}` + truncation report) and a root
  `routing` report. Every `choice`/`score` also carries `x_jev_confidence`
  (normalized entropy); `noul` carries `confidence`, never the other field.
- `POST /v1/systemone/batch` — bulk sibling: collates `states × questions`
  rows into ONE tensor (one forward pass). Body controls
  (`batch_size`, `min_confidence`, `sort_by_length`); per-item controls
  (`max_len`, `head_max_len`, `task`, `lang`, `lang_guess`). Use for batch
  (Phase 20); ~127–142 ms/row CPU measured on english, flat to ~128 rows.
- Inference failure → fixed HTTP 500 (`"inference failed"`) — no paths or
  internals leak to the client.

## Request shaping notes (for Phase 17 planner)

- Per-question body controls: `model`, `max_len`, `head_max_len`, `task`,
  `lang`, `lang_guess`, `min_confidence`.
- Row width = `max_len`; multilingual checkpoint context is 1024 tokens
  (default row width ships the english 512 when unset — Phase 17 must set
  an explicit budget from the evidence-selection pipeline).
- `choice` option budget: keep every `choice` < 11 options (temperature-0.10
  overconfidence pitfall past that; hierarchical routing instead).
- Checkpoint ids: `multilingual` → standalone repo
  `convaiinnovations/laya-multilingual` (322M params, 1024 tokens, 100+ langs).
