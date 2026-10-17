# Phase 40: Generation Flow — CONTEXT

## Previous Phase (39) Decisions
- `LlmSettingsError` variants are payload-free (no secret interpolation by construction)
- `DEFAULT_TEMPLATE` shared byte-identically with M23 seed (drift guarded)
- `pub mod llm;` in lib.rs; `SCHEMA_VERSION` fixed to 23; native-tls (not rustls)
- Keyring entry `sge-llm-api-key`: presence-bool only (`has_api_key`); never in SQLite/logs
- Template-only reset (`reset_llm_template`) preserves URL/model/defaults/key
- https-only + loopback exception (`http://localhost`) for base URL
- Timeout unclamped (positive only); temp clamped 0–2; max tokens 1–4096
- Strict `{sender}`/`{subject}`/`{body}` placeholders; unknown tokens stay literal
- Body fenced as untrusted data; invisible HTML stripped; ~8k char truncation + disclosure marker
- Unit-tests-only verification (no live endpoint calls); mock transport for CI
- Commands (`get_llm_settings`, `save_llm_settings`, etc.) ship in Phase 39 with zero I/O

## Key Implementation Patterns (from PATTERNS.md)
- `spawn_blocking` for all keyring I/O; `Arc<Mutex<Store>>` for SQLite
- `reqwest::Url` parse only for validation; no `Client` construction in Phase 39
- Substitution: single-pass positional (`.replace()` chain, no backreference/dedupe)
- Hidden strip: nesting-counted tag scanner (not regex); void tags cut alone; unclosed opener drops tail
- Ammonia: `tags(HashSet::new())` for flatten; `clean_content_tags` for `script`/`style` drop

## Phase 40 Focus
- Add single backend command: validated settings + original → draft text
- `test_llm_connection` (`GET /models`) proves endpoint before first generation
- `generate_reply` (`POST /chat/completions`): non-streaming, configurable timeout, cancel, per-window single-flight
- Every failure class → plain-language error; retry only on 408/429/5xx
- Mock-transport tests cover OpenAI/Ollama envelopes, bare/JSON errors, redirect refusal, truncation-on-wire
- Key/body never in errors/logs; metadata-only: status, latency, token usage
