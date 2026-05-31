# AGENTS.md

**代码是唯一事实来源。** `docs/` 中的文档提供架构引导和设计意图，但可能过时。当文档与代码冲突时，以代码为准。文档的价值是指向正确方向——实际结构请用 `grep` / `glob` 验证。

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
    ├── mod.rs         ← re-exports GcTask, MemoryTask, RuleTask, SummaryTask
    ├── gc_task.rs     ← GcTask (garbage collection, stale rule/memory cleanup)
    ├── memory_task.rs ← MemoryTask (session memory extraction)
    ├── rule_task.rs   ← RuleTask (scheduled cron shell for rule pipeline)
    ├── rule_suggester.rs ← RuleSuggester: scan patterns/failed_tasks, LLM cluster → promote
    ├── rule_recorder.rs  ← RuleRecorder: dedup + write learned rules with version metadata
    └── summary_task.rs← SummaryTask (VFS summary generation)
```

- `tasks/` was merged into `scheduler/` as a submodule. Import paths:
  - `tianyan::scheduler::{TaskScheduler, TaskHandler, TaskContext, ...}`
  - `tianyan::scheduler::tasks::{GcTask, MemoryTask, RuleTask, SummaryTask}`
- There is no `tianyan::tasks` — it was removed.

### Config System

Two config types exist (known duplication, not yet unified):
- `config/model.rs::ModelServiceConfig` — TOML layer (`TianyanConfig.models.services`)
- `model/config.rs::ModelConfig` — internal provider layer

No `max_retries` field — it was removed.

### Session & Context Layer

```
core/src/common/types/
├── message.rs              ← Message (传输层, 含 reasoning_content)
├── structured_message.rs   ← StructuredMessage (存储层, 含 compression_marker 字段)
└── ...

core/src/context/
├── assembler.rs            ← ContextAssembler: 存储层 → 传输层转换 (纯函数)
├── compression/            ← ContextCompressor (含 cached_summary 增量摘要), TokenEstimator
├── pipeline.rs             ← ContextPipeline: load_injectable() + compress_if_needed()
└── retrieval/
    ├── types.rs            ← RetrievalResult, RetrievalStep, RetrievalTrace
    ├── retriever.rs        ← DualLayerRetriever (持有 vfs, intent_analyzer)
    ├── loader.rs           ← ContentLoadStrategy (TokenBudget 已删除)
    ├── intent.rs           ← IntentAnalyzer, Intent, QueryType
    └── trace.rs            ← RetrievalTraceBuilder

core/src/agent/
├── coordinator.rs          ← Agent: 自闭环管理 session (加载→执行→持久化→压缩)
├── loop.rs                 ← AgentLoop: 持有 SessionManager，实时持久化每条消息
├── session_state.rs        ← SessionState (structured_messages + injectable_context)
└── ...
```

#### Data flow (updated)

```
Server: agent.process_message(session_id, msg)  ← 传入 session_id, 非 SessionState
  │
  └─ Agent:
       1. session_manager.get_session(session_id) → 构建 SessionState (marker 截断)
       2. prepare_context() → Vec<Message> (soul 首次加载后缓存)
       3. agent_loop.run(&mut messages, session_id, parent_id)
            → loop 内每产生一条消息，实时调 session_manager.add_structured_message()
            → 工具调用消息不丢弃，全部持久化
       4. maybe_compress_and_persist()
            → 从后向前扫描 compression_marker
            → marker 后消息量 > 6 → LLM 摘要 → StructuredMessage { compression_marker: true }
            → 持久化到 session 文件
```

#### Key design points

- **Storage/transmission separation**: `StructuredMessage` (含 `Part: Text|Reasoning|ToolCall|ToolResult`, `compression_marker`, token stats, cost) 用于持久化; `Message` 仅用于 LLM 传输. 通过 `ContextAssembler` 转换.
- **`compression_marker` 字段**: `StructuredMessage` 上的 `bool` 标记。压缩产生的摘要 System Message 设为 `true`。加载 session 时从后向前扫描找到最近 marker，只加载 marker 及之后的消息（旧消息在文件中保留但不加载）。
- **`SessionManager::add_structured_message()`**: 直接持久化 `StructuredMessage`（不经过 `Message` 转换）。AgentLoop 用此方法实时落盘。
- **`Agent::process_message(session_id, msg)`**: Agent 持 `Arc<dyn SessionManager>`，自闭环：加载 session → 构建状态 → 执行 loop → 持久化所有消息 → 压缩 → 返回响应。Server 不再需要 `bootstrap_session` 或手动持久化。
- **`AgentLoop` 实时持久化**: 持有 `SessionManager`，loop 中每条 assistant/tool_result/answer 消息在 push 到内存后立刻调 `add_structured_message()` 落盘。持久化失败仅 `tracing::warn`，不中断 loop。
- **`ContextPipeline` 拆分**: `load_injectable(query)` 加载 soul+rules+memories（soul 内部缓存，一次会话只读一次）；`compress_if_needed(conversation)` 独立执行压缩；`compress_for_session(messages, session_id)` 返回带 `compression_marker` 的 `StructuredMessage`。`run()` 保留为便捷方法（测试使用）。
- **`cached_summary` 增量摘要现在可用**: `ContextCompressor` 通过 `Arc<TokioMutex<ContextCompressor>>` 共享，`compress_if_needed` 不再 clone 导致状态丢失。`incremental_summarize` 路径可达。
- **`ContextAssembler::assemble()`** builds `Vec<Message>` in cache-optimal order: soul → rules+memories → history → current input.
- **`InjectableContext`** lives in `agent/session_state.rs`. Fields: `soul`, `rules_and_experiences`, `memories`, `last_updated`.
- **`TokenBudget` 已删除**: `loader.rs` 只保留 `ContentLoadStrategy`，`from_score` 是唯一入口。`retrieve_with_budget` / `load_content_with_budget` / `TokenLimitExceeded` 均已删除。
- **Compress 简化**: `preserve_recent_messages` 分割逻辑已移除。`compress()` 对所有传入消息做全量/增量摘要，不再保留最近 N 条。

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
  - `ContextRetriever` trait was removed — `DualLayerRetriever` is the only adapter; callers use the concrete type.
  - `SummaryEngine` (`core/src/vfs/summary/engine.rs`) is a concrete struct for good reason — no second summary strategy exists. Mock dependencies (`ChatService`, `EmbeddingService`) which already have traits instead.
  - `ConversationSummarizer` was removed — it was a shallow wrapper (15 lines of logic) over `ChatService` with zero external callers. The LLM summary calls were inlined into `ContextCompressor`.
- `DEFAULT_SOUL` constant lives at `core/src/agent/mod.rs` (built via `include_str!`). Use it instead of reaching for the raw file.
- **Rule pipeline** (`RuleTask → RuleSuggester → RuleRecorder`) lives entirely in `scheduler/tasks/`. It was moved out of `context/` because it's a scheduled cron task (every 15 min, after `MemoryTask`), not context-assembly logic.
- **`AgentHarness`** (`agent/harness.rs`) is minimal — only holds `AgentMetrics`. Rule recording happens exclusively through the scheduler's `RuleTask`, not from the agent loop.
