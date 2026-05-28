# AGENTS.md

## Build & Check Commands

```powershell
# Quick compile check (run before claiming success)
cargo check --workspace

# Format check
cargo fmt --all -- --check
cargo fmt --all             # auto-fix in place

# Clippy (expect failures due to missing_docs; see Lint section)
cargo clippy --workspace -- -D warnings

# Run tests for a single crate
cargo test -p tianyan-core --lib -- --nocapture
cargo test -p tianyan-server --lib -- --nocapture

# Run a specific test module
cargo test -p tianyan-core vfs::backend::local -- --nocapture

# All-in-one script
.\scripts\test.ps1 lint          # fmt + clippy
.\scripts\test.ps1 unit          # unit tests only
.\scripts\test.ps1 integration   # integration tests
.\scripts\test.ps1 e2e           # E2E tests
.\scripts\test.ps1 all           # lint + unit + integration + e2e + bench
```

## Workspace Structure

4 crates under single `Cargo.toml` workspace (resolver = "2"):

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

**`cargo clippy -- -D warnings` will fail** due to the large number of pre-existing `missing_docs` violations. For PR-level lint, run clippy without `-D warnings` and focus on new issues in changed files.

**Do NOT add inline `//` comments** unless asked. Use Rust doc comments (`///`, `//!`) for public API docs. Inline comments are kept minimal.

## Key Architecture

### Model Layer

```
core/src/model/
├── traits.rs         ← ServiceDiscovery (base), ChatService, EmbeddingService, VlmService
├── types/            ← ChatCompletionRequest, ModelInfo, etc.
├── config.rs         ← ModelConfig
├── provider/         ← AsyncOpenAIClient + trait impls (+ LoggedService decorator)
└── services.rs       ← ModelServices: bundles Arc<dyn ChatService>, EmbeddingService, VlmService
```

- `ModelServices` (NOT `ModelRouter` — that was replaced) is a plain container. No routing, no failover, no health tracking.
- Failover belongs at the caller/harness layer. Providers should **fail fast**, not retry or backoff.
- `RetryService` was intentionally removed. Do not reintroduce.
- Conversion from TOML config: `server/src/agent_builder.rs::create_model_services()`.
- `model/provider/` uses `pub(crate)` for internal items; external code goes through `ModelServices`.
- `ServiceDiscovery::is_available()` is `async` — don't forget `await`.

### VFS Module

```
core/src/vfs/
├── traits.rs         ← VfsCore, ContentStore, VfsSearch, VirtualFileSystem (super-trait)
├── types.rs          ← ContextEntry, VectorPoint, etc.
├── vfs_impl.rs       ← VirtualFileSystemImpl (the main implementation)
├── backend/
│   ├── traits.rs     ← StorageBackend trait (9 methods)
│   └── local.rs      ← LocalFileBackend (filesystem adapter)
├── vector/
│   ├── traits.rs     ← VectorStorage trait (12 methods)
│   └── qdrant.rs     ← QdrantVectorStore (Qdrant adapter)
└── summary/
    └── engine.rs     ← SummaryEngine (LLM-based summary generation)
```

**Trait implementation rules for backends:**
- Implement trait methods **directly** in the `impl Trait for Struct` block. Do NOT define inherent methods that the trait impl delegates to. This avoids ~40% duplication.
- `LocalFileBackend` has one `impl LocalFileBackend` block (constructors + private helpers) and one `impl StorageBackend for LocalFileBackend` block (real implementations). No second inherent block.
- Same pattern applies to `QdrantVectorStore` / `VectorStorage`.
- Pass-through wrappers like `fn foo(x) -> X { x.bar() }` are waste — call the underlying method directly.

**VirtualFileSystem trait hierarchy:**
- `VfsCore` (entry lifecycle + metadata), `ContentStore` (layered I/O), `VfsSearch` (vector search)
- `VirtualFileSystem` is an auto-implemented super-trait: any type implementing all three sub-traits gets it.
- Consumers should prefer `Arc<dyn VirtualFileSystem>` injection.

### Scheduler & Tasks

```
core/src/scheduler/
├── mod.rs             ← re-exports TaskScheduler, TaskHandler, TaskContext, etc.
├── task_scheduler.rs  ← TaskScheduler, TaskHandler trait, TaskContext, TaskDefinition, TaskResult
└── tasks/
    ├── mod.rs         ← re-exports GcTask, MemoryTask, SummaryTask
    ├── gc_task.rs     ← GcTask (garbage collection, stale rule/memory cleanup)
    ├── memory_task.rs ← MemoryTask (session memory extraction)
    └── summary_task.rs← SummaryTask (VFS summary generation)
```

- `tasks/` was merged into `scheduler/` as a submodule. Import paths:
  - `tianyan::scheduler::{TaskScheduler, TaskHandler, TaskContext, ...}`
  - `tianyan::scheduler::tasks::{GcTask, MemoryTask, SummaryTask}`
- There is no `tianyan::tasks` — it was removed.

### Config System

Two config types exist (known duplication, not yet unified):
- `config/model.rs::ModelServiceConfig` — TOML layer (`TianyanConfig.models.services`)
- `model/config.rs::ModelConfig` — internal provider layer

No `max_retries` field — it was removed.

### Error Types

All errors use `TianyanError` enum from `core/src/common/error.rs`. Extend `TianyanError` if needed; never introduce new error types.

## Testing

- Unit tests: `#[cfg(test)] mod tests { ... }` inline in source files (preferred)
- Integration tests: `tests/` directories under each crate
- CI matrix runs on ubuntu, windows, macos with `--nocapture`
- The test suite expects no external services — `MockVectorStorage` and temp directories are used for isolation

## Common Pitfalls

- `tianyan` and `tianyan-core` refer to the same package (lib name `tianyan`). Imports use `tianyan::...`.
- `model/provider/` uses `pub(crate)` visibility; go through `ModelServices` from outside.
- `ServiceDiscovery::is_available()` is `async` — don't forget `await`.
- Config file search order: `./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`.

### VFS Initialization Split

Application bootstrap logic lives in the server crate, NOT in core VFS:

- `VfsCore::initialize()` (`core/src/vfs/vfs_impl.rs`) handles pure infrastructure: storage + vector init + creating namespace root directories for each `ContextNamespace`.
- `bootstrap_app_vfs()` in `server/src/lib.rs` handles application content: default soul.md, learned directory, user profile stub. This is where `AgentPath`, `DEFAULT_SOUL`, and user-facing content belong.

When adding VFS init logic, decide: is it infrastructure (put it in `initialize()`) or application content (put it in server's `bootstrap_app_vfs`)?

### Design Principles

- **One adapter = hypothetical seam. Two adapters = real seam.** Don't introduce a trait solely for testability or future flexibility. Extract a trait only when a second adapter exists or is being built right now.
- `SummaryEngine` (`core/src/vfs/summary/engine.rs`) is a concrete struct for good reason — no second summary strategy exists. Mock dependencies (`ChatService`, `EmbeddingService`) which already have traits instead.
- `DEFAULT_SOUL` constant lives at `core/src/agent/mod.rs` (built via `include_str!`). Use it instead of reaching for the raw file.
