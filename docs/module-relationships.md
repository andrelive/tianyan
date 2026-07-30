# 天演模块关系图

> 本文档描述天演项目各模块间的依赖关系、调用流程和数据交互机制。
>
> **相关文档**：[模块功能说明](./module-descriptions.md) | [系统架构文档](./system-architecture.md)

---

## 1. 顶层模块依赖关系

### 1.1 Crate 依赖图

```
┌─────────────────────────────────────────────────────────────────┐
│                      tauri (tianyan-tauri)                       │
│  依赖：tianyan-core, tianyan-server                             │
│  职责：桌面应用入口、窗口管理、服务器生命周期                       │
└───────────────────────────┬─────────────────────────────────────┘
                            │ depends on
              ┌─────────────┼─────────────┐
              ▼             ▼
┌──────────────────────┐  ┌──────────────────────────────────────┐
│ server (tianyan-     │  │ core (tianyan-core)                  │
│   server)            │  │  无外部项目依赖                        │
│ 依赖：tianyan-core   │  │ 职责：AI Agent 核心、VFS 存储、        │
│ 职责：HTTP API、     │──│   会话管理、上下文管线、技能系统        │
│   应用状态管理       │  │                                      │
└──────────────────────┘  └──────────────────────────────────────┘
              ┌────────────────────────┐
              │ gui (tianyan-gui)      │
              │  无 Rust 层项目依赖     │
              │ 职责：React TypeScript 前端    │
              │ 通过 HTTP 与 server 通信│
              └────────────────────────┘
```

### 1.2 依赖关系矩阵

| 模块 | tianyan-core | tianyan-server | tianyan-gui | tianyan-tauri |
|------|:---:|:---:|:---:|:---:|
| **tianyan-core** | - | ❌ | ❌ | ❌ |
| **tianyan-server** | ✅ | - | ❌ | ❌ |
| **tianyan-gui** | ❌ | ❌ | - | ❌ |
| **tianyan-tauri** | ✅ | ✅ | ❌* | - |

> \* tauri 通过加载 `gui/dist` 静态资源间接依赖 gui，但无 Rust 编译期依赖。

---

## 2. Core 内部模块依赖关系

### 2.1 模块目录结构

```
core/src/
├── agent/       (coordinator, loop, session_state, tool_registry, tools, types, builder)
├── common/      (error, types/ 子模块)
├── config/      (TOML 配置)
├── context/     (pipeline, assembler, compression/, retrieval/)
├── executor/    (工具执行：Action、审批、LLM-as-Judge、验证门控)
├── knowledge/   (parser, image, ingestor/, types)
├── memory/      (extractor.rs)
├── model/       (traits, services.rs, provider/)
├── observability/ (空壳)
├── scheduler/   (task_scheduler, tasks/)
├── session/     (manager, types)
├── skills/      (definition, executor, manager, handlers/, learning/, registry, types)
├── vfs/         (traits, vfs_impl, backend/, vector/, summary/)
└── lib.rs
```

### 2.2 模块依赖图

```
                        ┌──────────────┐
                        │   config     │  配置管理
                        └──────┬───────┘
                               │
                        ┌──────┴───────┐
                        │   common     │  通用类型、错误处理
                        └──────┬───────┘
                               │
          ┌────────────────────┼────────────────────┐
          ▼                    ▼                     ▼
    ┌───────────┐      ┌─────────────┐       ┌──────────────┐
    │   model   │      │     vfs     │       │   session    │
    │ 模型服务   │      │ 虚拟文件系统 │       │  会话管理    │
    └─────┬─────┘      └──────┬──────┘       └──────┬───────┘
          │                   │                     │
          │        ┌──────────┼──────────┐          │
          │        ▼          ▼           ▼          │
          │  ┌─────────┐ ┌────────┐ ┌─────────┐     │
          │  │ context │ │ memory │ │knowledge│     │
          │  │管线/    │ │ 提取器  │ │知识管理 │     │
          │  │组装/    │ │        │ │         │     │
          │  │检索     │ │        │ │         │     │
          │  └────┬────┘ └────────┘ └─────────┘     │
          │       │                                 │
          │       ▼                                 │
          │  ┌──────────────────────────┐           │
          └──│          agent           │───────────┘
             │     AgentCoordinator     │
             │  (ContextPipeline/       │
             │   AgentLoop/             │
             │   ToolRegistry)          │
             └────────────┬─────────────┘
                          │
         ┌────────────────┼────────────────┐
         ▼                                  ▼
   ┌───────────┐                     ┌────────────┐
   │  skills   │                     │ scheduler  │
   │ 技能系统   │                     │ 定时任务    │
   │ (GEPA)    │                     │ 调度器      │
   └───────────┘                     └────────────┘
```

**依赖方向说明**：
- `common` → 所有模块的基础层（类型、错误）
- `model / vfs / session` → 中坚层，分别提供 LLM 服务、存储、会话管理
- `context` → 依赖 vfs 和 model，提供检索+组装+压缩管线
- `agent` → 协调者，聚合 context、model、session、vfs
- `skills / scheduler` → 被 agent 调用，scheduler 独立运行后台任务

---

## 3. 核心调用流程

### 3.1 应用启动流程

```
Tauri App 启动
    │
    ├─ 1. init_logging()                    [tauri/src/lib.rs]
    │     └─ 初始化日志系统（文件 + 控制台）
    │
    ├─ 2. TianyanConfig::load()             [core/src/config/mod.rs]
    │     └─ 加载配置（TOML 文件 + 环境变量）
    │
    ├─ 3. start_axum_server(config)         [tauri/src/server.rs → server/src/lib.rs]
    │     │
    │     ├─ 3.1 initialize_vfs_for_app()   [server/src/lib.rs]
     │     │     ├─ SqliteBackend::new()
    │     │     ├─ LanceDbVectorStore::new() + initialize()
    │     │     ├─ ModelServices::from_configs() — 替代旧 ModelRouter
    │     │     └─ VirtualFileSystemBuilder::build() + initialize()
    │     │
    │     ├─ 3.2 AppState::new(config, vfs) [server/src/state.rs]
    │     │     ├─ create_summary_service()
    │     │     ├─ create_skill_registry/executor()
    │     │     └─ AgentBuilderFactory::build_agent_or_wizard()
    │     │         ├─ validate_config()
    │     │         ├─ ModelServices::from_configs()
    │     │         ├─ DualLayerRetriever::new()
    │     │         └─ AgentBuilder::build()
    │     │
    │     ├─ 3.3 TaskScheduler 启动
    │     │     ├─ SummaryTask（每 5 分钟）
    │     │     └─ MemoryTask（每 10 分钟）
    │     │
    │     └─ 3.4 axum::serve()              [监听 127.0.0.1:3000]
    │
    ├─ 4. wait_for_server_ready()           [tauri/src/lib.rs]
    │     └─ 轮询 GET /health（30s 超时）
    │
    └─ 5. Tauri 窗口创建
          └─ 加载 gui/dist/index.html
```

### 3.2 聊天请求流程

```
用户输入 (React Frontend)
    │
    ▼ HTTP POST /api/v1/chat/stream
Axum chat_stream_handler                  [server/src/api/chat/handlers.rs]
    │
    ▼ ChatService::process_message_stream()  [server/src/api/chat/services.rs]
    │  ├─ 持久化用户消息（SessionManager）
    │  ├─ 自动生成会话标题
    │  └─ AgentCoordinator::process_message()
    │
Agent::process_message()                  [core/src/agent/coordinator.rs]
    │
    ├─ SessionStateManager::with_state() 获取/创建 SessionState
    ├─ SessionState::add_user_message()
    │
    ├─ ContextPipeline::run()             [core/src/context/pipeline.rs]
    │     ├─ 加载 soul（首次缓存）
    │     ├─ 加载 learned rules（tianyan://agent/learned/）
    │     ├─ DualLayerRetriever::retrieve()（L0+L1 RRF 融合）
    │     └─ ContextCompressor::compress_if_needed()
    │
    ├─ AgentLoop::run()                   [core/src/agent/loop.rs]
    │     ├─ ChatCompletionRequest::new(model, messages)
    │     │     .with_tools(ToolRegistry.definitions())
    │     ├─ ChatService::chat_completion_stream()
    │     ├─ LLM 返回 tool_calls → ToolRegistry::execute_parallel()
    │     │     ├─ 并行执行工具（文件读写、命令执行、代码搜索等）
    │     │     └─ 结果作为 Message::tool 追加到 messages
    │     ├─ LLM 返回 content → 返回 Answer，循环结束
    │     └─ 调用 ask_user → 返回 NeedsClarification，中断循环
    │
    ├─ SessionState::cleanup()（防止无限增长）
    │
    ├─ 后台异步（不阻塞响应）:
    │     └─ SkillLearningEngine::learn_from_history()（GEPA 进化）
    │
    └─ AgentStreamChunk 通过 mpsc 通道逐块输出
          │ 每个 chunk 含 chunk_type (Thought/ToolCall/Observation/Answer/Error)
          │
          ▼ ChatService 构造 ChatStreamEvent (含 chunk_type)
          │
          ▼ SSE 流式响应
React Frontend 按 chunk_type 差异化渲染
```

> **注意**：RuleRecorder/RuleSuggester 已移至 `scheduler/tasks/`，作为定时任务独立运行；
> MemoryExtraction 由 `scheduler/tasks/memory_task.rs` 定时触发。

### 3.3 消息编辑/重新生成流程

```
用户点击 "编辑" 或 "重新生成"
    │
    ├─ 前端本地：EditMessage / RegenerateFrom + DeleteMessagesFrom
    │
    ├─ HTTP POST /api/v1/chat/edit 或 /chat/regenerate
    │     └─ 后端更新服务端会话状态（修改消息、截断历史）
    │
    └─ HTTP POST /api/v1/chat/stream
          └─ SSE 流式获取新回复（同 3.2 流程）
```

### 3.4 VFS 数据交互流程

```
                    ┌──────────────────────────────────────┐
                    │        VirtualFileSystem (trait)       │
                    │  聚合 VfsCore + ContentStore +         │
                    │        VfsSearch                      │
                    │  位置：core/src/vfs/traits.rs          │
                    └───────────────┬──────────────────────┘
                                    │
                ┌───────────────────┼───────────────────┐
                ▼                   ▼                    ▼
    ┌─────────────────┐  ┌─────────────────┐  ┌──────────────────┐
│   SqliteBackend   │  │   ContentStore   │  │    VfsSearch     │
│  (SQLite 存储)    │  │  L0/L1/L2 读写   │  │  RRF 融合检索    │
    └────────┬────────┘  └────────┬────────┘  └────────┬─────────┘
             │                    │                     │
             ▼                    ▼                     ▼
    ┌─────────────────┐  ┌─────────────────┐  ┌──────────────────┐
│  backend/sqlite/  │  │  summary/       │  │  vector/lancedb.rs │
│  SqliteBackend    │  │  SummaryEngine  │  │  LanceDbVectorStore │
    └─────────────────┘  └─────────────────┘  └──────────────────┘
```

### 3.5 记忆持久化流程（定时任务）

```
TaskScheduler 触发
    │
    ├─ MemoryTask::handle()               [core/src/scheduler/tasks/memory_task.rs]
    │     └─ MemoryExtractor::extract_from_text()
    │           ├─ ChatService::chat_completion()（提取记忆）
    │           └─ 返回 Vec<MemoryEntry>
    │           └─ VFS::write() 持久化到 tianyan://memory/
    │
    └─ SummaryTask::handle()              [core/src/scheduler/tasks/summary_task.rs]
          └─ SummaryEngine::process_all()
                ├─ VFS::list()（扫描条目）
                ├─ VFS::read()（读取原始内容）
                ├─ LLM 生成 L0/L1 摘要
                ├─ VFS::write_abstract/overview()
                └─ VFS::update_summary_vectors()
                      └─ EmbeddingService::embed_single()
                            └─ VectorStorage::upsert_point()
```

---

## 4. 跨层数据交互机制

### 4.1 Tauri → Server

| 交互方式 | 说明 |
|---------|------|
| **函数调用** | `start_axum_server(config)` — 直接调用 server crate 的公开函数 |
| **健康检查** | `wait_for_server_ready()` — HTTP GET /health |
| **配置传递** | `TianyanConfig` 通过函数参数传递 |

### 4.2 Server → Core

| 交互方式 | 说明 |
|---------|------|
| **Rust 函数调用** | 直接调用 `tianyan::` 命名空间下的所有公开 API |
| **Trait 对象** | 通过 `Arc<dyn AgentCoordinator>`、`Arc<dyn VirtualFileSystem>` 等接口交互 |
| **依赖注入** | VFS 从应用层注入到 AppState，再传递到 Agent |

### 4.3 GUI → Server

| 交互方式 | 说明 |
|---------|------|
| **HTTP REST API** | 通过 `gloo-net` 发起 HTTP 请求 |
| **SSE 流式** | 通过 `ReadableStream` 处理 Server-Sent Events |
| **API 路径** | 基地址 `http://localhost:3000/api/v1`，向导用 `/api`（无版本） |
| **错误处理** | 统一解析后端 `ErrorResponse { error: String }` 格式 |

### 4.4 关键数据流路径（一图流）

```
用户输入
  → [HTTP] → Server API Handler
  → [Rust] → ChatService（持久化 + 标题生成）
  → [Trait] → AgentCoordinator::process_message()
  → [Rust] → ContextPipeline::run()（soul → rules → retrieval → compression）
  → [Rust] → AgentLoop::run()（LLM + ToolRegistry 迭代循环）
  → [Trait] → ChatService::chat_completion_stream()
  → [Rust] → ToolRegistry::execute_parallel()（并行工具执行）
  → [Rust] → AgentStreamChunk (含 chunk_type)
  → [Rust] → ChatStreamEvent（透传 chunk_type）
  → [HTTP SSE] → Frontend 按 chunk_type 差异化渲染
```

---

## 5. 接口契约

### 5.1 核心 Trait 接口

| Trait | 定义位置 | 实现者 | 消费者 |
|-------|---------|--------|--------|
| `AgentCoordinator` | `core/src/agent/coordinator.rs` | `Agent` | `server/state.rs`, `server/api/chat/services.rs` |
| `ChatService` | `core/src/model/traits.rs` | `AsyncOpenAIClient`（经 `LoggedService` 装饰） | `agent/coordinator.rs`, `agent/loop.rs` |
| `EmbeddingService` | `core/src/model/traits.rs` | `AsyncOpenAIClient`（经 `LoggedEmbeddingService` 装饰） | `vfs/vfs_impl.rs`, `context/retrieval/` |
| `VlmService` | `core/src/model/traits.rs` | `AsyncOpenAIClient`（经 `LoggedVlmService` 装饰） | `knowledge/image/` |
| `ServiceDiscovery` | `core/src/model/traits.rs` | `AsyncOpenAIClient` | `server/state.rs`（健康检查） |
| `VfsCore` | `core/src/vfs/traits.rs` | `VfsImpl` | `vfs/vfs_impl.rs`, `context/pipeline.rs` |
| `ContentStore` | `core/src/vfs/traits.rs` | `VfsImpl` | `vfs/vfs_impl.rs`, `scheduler/tasks/` |
| `VfsSearch` | `core/src/vfs/traits.rs` | `VfsImpl` | `context/retrieval/retriever.rs` |
| `VirtualFileSystem` | `core/src/vfs/traits.rs` | 实现 VfsCore+ContentStore+VfsSearch 的类型自动获得 | `server/state.rs`, `agent/coordinator.rs`, `session/manager.rs` |
| `SqliteBackend` | `core/src/vfs/backend/sqlite.rs` | SQLite 存储后端（具体类型） | `vfs/vfs_impl.rs` |
| `VectorStorage` | `core/src/vfs/vector/traits.rs` | `LanceDbVectorStore` | `vfs/vfs_impl.rs`, `context/retrieval/` |
| `SessionManager` | `core/src/session/manager.rs` | `PersistentSessionManager`, `PlaceholderSessionManager` | `server/state.rs`, `agent/coordinator.rs` |
| `SkillExecutor` | `core/src/skills/executor.rs` | `SkillExecutor` | `agent/tool_registry.rs`（通过 call_skill 工具桥接） |

> **注意**：`ModelServices` 不是 trait，是 `core/src/model/services.rs` 中的 struct，聚合 `Arc<dyn ChatService>` + `Arc<dyn EmbeddingService>` + `Arc<dyn VlmService>`。

### 5.2 HTTP API 契约

详见 [模块功能说明 - API 端点清单](./module-descriptions.md#22-api-端点清单)。

### 5.3 流式数据契约（ChatStreamEvent）

```json
{
    "id": "chatcmpl-0",
    "session_id": "uuid",
    "delta": "增量文本",
    "finish_reason": null,
    "chunk_type": "Answer",
    "skill_calls": null
}
```

- `chunk_type` 6 种：`Thought` / `ToolCall` / `Observation` / `Answer` / `Error` / `Clarification`
- 后端从 `AgentStreamChunk.chunk_type` 透传，前端据此差异化渲染
- `[DONE]` 或 `finish_reason` 有值时表示流结束

### 5.4 错误响应契约

```json
{
    "error": "错误描述信息"
}
```

- 后端 `ApiError::IntoResponse` 生成，HTTP 状态码映射：NotFound→404, BadRequest→400, Internal→500
- 前端 `parse_error_body()` 解析此格式，兼容原始 HTTP 状态文本回退

---

## 6. 架构偏差分析

以下为当前与理想架构仍存在的偏差：

| 偏差项 | 描述 | 实际状态 | 影响 |
|--------|------|---------|------|
| SessionManager | VFS 持久化会话管理 | `PlaceholderSessionManager`（空实现） | 会话不持久化 |
| 前端 Knowledge UI | 知识管理面板 | 无前端 UI 对应 | 后端端点只被 API 工具使用 |
| 前端 Runtime Config UI | 运行时配置面板 | 无前端 UI 对应 | 后端 3 个端点闲置 |

---

## 7. 核心架构决策与模块关系

以下 4 个架构决策决定了模块间的职责划分和依赖方向：

### 决策 1：VFS 双层摘要索引 — 底层基础

`vfs/` 是统一存储与检索层。所有上下文（知识库、记忆、技能、规则）通过 VFS 管理，采用三层内容 + 双向量 RRF 融合：

| 层级 | Token | 向量 | 用途 |
|------|-------|------|------|
| L0 Abstract | ~100 | `abstract_vector` | 向量搜索、快速过滤 |
| L1 Overview | ~2K | `overview_vector` | 内容导航、重排序 |
| L2 Detail | 无限制 | — | 完整内容，按需加载 |

**模块影响**：`context/retrieval/` 依赖 `vfs::VfsSearch` 做双层检索；`scheduler/tasks/summary_task.rs` 定时生成 L0/L1 摘要。

### 决策 2：StructuredMessage — 单一真相源

`common/types/structured_message.rs` 定义 `StructuredMessage`，贯穿持久化 → 会话组装 → 跟踪 → 统计全流程。`compression_marker` 字段标记压缩锚点。

**模块影响**：`session/manager.rs` 持久化 StructuredMessage；`context/assembler.rs` 将其转为 LLM 传输格式；`agent/loop.rs` 每轮生成后实时写入。

### 决策 3：组件工具化

`agent/tool_registry.rs` 注册 9 个工具（`read_file`、`write_file`、`execute_command`、`search_code`、`call_skill`、`run_tests`、`verify_build`、`ask_user`、`delegate_to_agent`），其中 `call_skill` 桥接到 `skills/executor.rs`。

**模块影响**：`agent/tool_registry.rs` 依赖 `skills::SkillExecutor` 实现 call_skill 工具，形成 agent → skills 单向依赖。

### 决策 4：前缀匹配缓存顺序

`context/assembler.rs` 的 `assemble()` 严格遵循固定前缀 → 可变后缀：

```
soul → rules+memories → history(from compression_marker) → current input
```

**模块影响**：soul 首次加载后缓存（`context/pipeline.rs` 中的 `cached_soul`）；compression_marker 由 `context/compression/` 模块管理；顺序不可变更以保证 LLM 前缀缓存命中率。

---

## 8. 已知待办事项

| 位置 | 内容 | 优先级 |
|------|------|:---:|
| sessions/services.rs | TODO: 与核心存储集成 | 🔴 |
| knowledge/services.rs | TODO: 与核心摄入管道/检索引擎集成 (4处) | 🔴 |
| config/services.rs | TODO: 与核心配置存储集成 (2处) | 🔴 |
| gui 知识管理面板 | 前端无 Knowledge UI，后端端点闲置 | 🟡 |
| gui 运行时配置 | 前端无 Runtime Config UI | 🟡 |
| 记忆持久化 | MemoryExtractor 已就绪，server 层接入待实现 | 🟡 |

---

**文档版本**: 2026-06-04
**最后更新**: 2026-06-04（全面修正：移除不存在模块、更新 trait 契约、修剪历史偏差、新增架构决策章节）
