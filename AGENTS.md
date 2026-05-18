# AGENTS.md

## Build & Check Commands

```powershell
# Quick compile check (run before claiming success)
cargo check --workspace

# Full lint pass (fmt + clippy)
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings

# Unit tests (lib only)
cargo test --workspace --lib -- --nocapture

# Integration tests
cargo test --workspace --test '*' -- --nocapture

# All-in-one script
.\scripts\test.ps1               # all checks + tests
.\scripts\test.ps1 lint          # fmt + clippy only
.\scripts\test.ps1 unit          # unit tests only
```

## Workspace Structure

4 crates, all under single `Cargo.toml` workspace (resolver = "2"):

| Crate | Package | Type |
|-------|---------|------|
| `core/` | `tianyan-core` (lib: `tianyan`) | Pure lib, no bin |
| `server/` | `tianyan-server` | Axum HTTP server (`main.rs` + `lib.rs`) |
| `gui/` | `tianyan-gui` | Yew WASM frontend |
| `tauri/` | `tianyan-tauri` | Tauri desktop wrapper |

All shared deps are in workspace `[workspace.dependencies]`. When adding a dep, add it there first, then reference it in the crate's `Cargo.toml`.

## Lint & Code Style

Workspace-level lints in `Cargo.toml`:
- `unsafe_code = "deny"` — no unsafe anywhere
- `missing_docs = "warn"` — every `pub` item needs a doc comment
- `unused_qualifications = "warn"` — don't write `crate::foo::Bar` when `Bar` alone resolves
- Clippy: `unwrap_used`, `expect_used`, `unwrap_in_result` are all `warn`

**Do NOT add comments to code** unless asked. The codebase uses Rust doc comments (`///`, `//!`) for public API docs, but inline `//` comments are kept minimal.

## Key Architecture Decisions

### Harness Engineering Philosophy

The project follows a harness pattern: components should **fail fast and transparently** rather than silently retry or paper over errors. The router/harness layer (`ModelRouter`) handles failover and health decisions; individual providers should not retry, add backoff, or mask failures. This was an explicit design decision made to remove `RetryService` from the model provider layer.

### Model Layer Hierarchy

```
core/src/model/
├── traits.rs         ← ServiceDiscovery (base), ChatService, EmbeddingService, VlmService
├── types/            ← ChatCompletionRequest, ModelInfo, etc.
├── config.rs         ← ModelConfig
├── provider/         ← AsyncOpenAIClient + trait impls (+ LoggedService decorator)
└── services.rs       ← ModelServices: simple container to build services from config
```

Trait dependency:
```
ServiceDiscovery: Send + Sync  ← list_models(), is_available(), service_name()
ChatService: Send + Sync      ← chat_completion(), chat_completion_stream(), chat()
EmbeddingService: Send + Sync ← embed(), service_name()
VlmService: Send + Sync       ← analyze_image(), service_name()
```

`ModelServices` is a plain struct that bundles `Arc<dyn ChatService>`, `Arc<dyn EmbeddingService>`, and `Arc<dyn VlmService>`. No routing, no failover, no health tracking — the harness philosophy is "fail fast, let the caller decide."

### Provider Module (`core/src/model/provider/`)

- `client.rs` — `AsyncOpenAIClient` struct wrapping `async-openai` + `finish_reason_str()`
- `chat.rs`, `embedding.rs`, `vision.rs`, `discovery.rs` — trait impls on `AsyncOpenAIClient`
- `middleware.rs` — `LoggedService<T>` decorator (pub(crate), only used internally)
- `mod.rs` — crate-visible re-exports

**Do NOT add retry logic to provider.** Retry was intentionally removed. Logging is done through `LoggedService` wrapping in `ModelServices::from_configs()`.

### Service Initialization

`server/src/agent_builder.rs::create_model_services()` converts TOML config into `ModelServices`. Callers extract what they need:

```rust
let svc = create_model_services(config).await?;
let chat = svc.chat;           // Arc<dyn ChatService>
let embed = svc.embedding;     // Arc<dyn EmbeddingService>
// call with embed.embed(...)
```

### Config System

Two config types exist (known duplication, not yet unified):
- `config/model.rs::ModelServiceConfig` — external TOML layer (`TianyanConfig.models.services`)
- `model/types/config.rs::ModelConfig` — internal provider/router layer

Conversion happens in `server/src/agent_builder.rs::build_model_router()`. No `max_retries` field — it was removed.

### Error Types

All errors use `TianyanError` enum from `core/src/common/error.rs`. Key variants:
- `Config(String)`, `ModelService(String)`, `EmbeddingService(String)`, `VlmService(String)`
- `Io(std::io::Error)`, `JsonError(serde_json::Error)`, `TomlError(toml::de::Error)`

Never introduce new error types; extend `TianyanError` if needed.

## Testing

- Unit tests: `#[cfg(test)] mod tests { ... }` inline in source files (preferred)
- Integration tests: `tests/` directories under each crate
- Run single crate: `cargo test -p tianyan-core --lib`
- CI matrix runs on ubuntu, windows, macos with `--nocapture`

## Common Pitfalls

- `tianyan` and `tianyan-core` refer to the same package (lib name `tianyan`). Imports use `tianyan::...`.
- The `model/provider/` submodule uses `pub(crate)` visibility for internal items; external code should go through `ModelRouter`.
- `ServiceDiscovery::is_available()` is `async` — don't forget `await`.
- Config file search order: `./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`.
