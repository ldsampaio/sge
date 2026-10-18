# Coding Style Guide (Synchronous, Minimal)

This project is written in synchronous Rust with async-imap for IMAP operations; all IPC calls are blocking and must complete within ~0.2s or the UI hangs visibly (Tauri runs on a single main thread). Keep functions short, use `async` only for network I/O, and block until completion — no fire-and-forget promises.

## Formatting

```bash
rustfmt --edition 2021 src-tauri/src/*.rs src-tauri/src/**/*.rs
cargo fmt -- src/tauri/src/frontend
```

Run these before committing.

## Naming Conventions

| Convention | Example | Notes |
|---|---|---|
| structs/snake_case `send_queue` | `enqueue_row`, `flush_one_row` | Lower camel for public types |
| traits snake_case `SyncSession` trait | impl lines 32–807 in `mod.rs` | Match Rust style guide |
| constants caps `SENT_FLAGS = "(\\Seen)"` | Single- or double-quoted per need | Prefix with `pub` if exported |
| functions `fn fetch_message() -> Result<Message, SyncError>` | One-line doc: param → return error | Keep bodies under 15 lines; nest ≤2 |

## Error Handling

```rust
Ok(result) => { /* happy path */ }
Err(e) => eprintln!("[{name}] {e}"), // log the user-facing text; never unwrap, always `?` or handle explicitly
```

Never print credentials. Log only error messages from the server or your own wrapper text.

## Async Patterns

```rust
async fn send_message(...) -> Result<..., SyncError> {
    Box::pin( async.move { /* network I/O; yield as needed */ }.await)
}
```

Use `Box::pin` for long futures in the background or inside `block_on`. For blocking calls (fetch_body, APPEND), wrap in `Pin` before awaiting.

## IPC Calls (Tauri)

Keep below ~50 lines; return typed errors with plain-language messages:

- ❌ `"network error: reqwest::Error { code=InvalidHeader }"`
- ✅ `"401: chave inválida — verifique se o modelo está disponível."`

## Testing

```bash
cargo test --lib                     # 530 unit tests
cargo test --bin live_gate_reply    # SMTP send + threading verification
cargo clippy                        # lint with `allow(dead_code)` only where needed
```

Lint passes CI; disable warnings per the `.github/stable.yml` profile.

## Documentation

Keep doc lines brief: "param → return error". Write a separate `*.md` for each feature (LLM-06 Ollama preset, LLM-10 Regenerate, etc.). Link back to project roadmap or PRD in planning docs.

---

See [PROJECT.md](PROJECT.md) for overall style notes and `.planning/ARCHITECTURE.md` for layer diagrams.
