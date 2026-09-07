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

> \* tauri 通过加载 `gui-vite/dist` 静态资源间接依赖 gui，但无 Rust 编译期依赖。

---

## 2. Core 内部模块依赖关系

### 2.1 模块目录结构

```
core/src/
├── agent/       (coordinator, loop, session_state, tool_registry/, tools, types, builder)
├── common/      (error, logging, token_estimator, types/ 子模块，含 content_part.rs)
├── config/      (TOML 配置；LoggingConfig 自 common::logging re-export)
├── context/     (pipeline, assembler, compression/, retrieval/)
├── db/          (统一写入门面：Database 门面 + SqliteDb + stats/trace/execution/usage Repository；只依赖 common)
├── executor/    (工具执行：Action、审批、LLM-as-Judge、验证门控)
├── knowledge/   (parser, image/, ingestor/, types)
├── memory/      (extractor.rs)
├── model/       (traits, services.rs, provider/)
├── observability/ (AgentMetrics, usage_stats/trace/execution_log/usage_log/rule_recorder；SQL 经 db 收敛)
├── roles/       (角色基础类型：AgentRole/RoleSource/RoleStatus 等，纯类型层)
├── role_store.rs (角色 VFS 存储，独立存储层)
├── scheduler/   (task_scheduler, tasks/)
├── session/     (store/search 直接实现 SQL（ADR-018 专属），manager, types)
├── skills/      (definition, executor, manager, handlers/, learning/, registry, types)
├── vfs/         (traits, vfs_impl, backend/ 含 sqlite.rs；SqliteDb 已移入 db/)
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
- `common` → 所有模块的基础层（类型、错误、日志配置、token 估算），自洽无环（不依赖 config）
- `model / vfs / session` → 中坚层，分别提供 LLM 服务、存储、会话管理
- `context` → 依赖 vfs 和 model，提供检索+组装+压缩管线
- `agent` → 协调者，聚合 context、model、session、vfs
- `skills / scheduler` → 被 agent 调用，scheduler 独立运行后台任务

**依赖环现状（阶段 1-3 分层重构后，详见 [ADR-007](architecture/decisions/007-core-dependency-cycle-removal.md) + [ADR-020](architecture/decisions/020-database-facade.md) + [ADR-021](architecture/decisions/021-layered-refactor.md)）**：

- ✅ **生产代码零模块环**（文件级 SCC 检测 = 0）：
  - 基础类型层：`roles`（角色纯类型）、`common`（错误/估算器等纯类型）——config/agent/scheduler 共用，不依赖领域
  - 存储层：`db`（Database 门面 + SqliteDb + 业务域 Repository）——**只依赖 `common`**（纯底层）
  - 领域层：`session`（SQL 收敛本模块，经 db 单连接）、`vfs`（SqliteBackend 经 db）、`observability`（SQL 经 db Repository）——单向依赖 db
  - 顶层：`agent` / `scheduler`（工具化装配，依赖领域层 + 基础类型）
- ⬇️ 残留单向依赖（均非环，已接受）：`vfs→model`（`SummaryEngine` 摘要生成依赖 `ChatService`）
- ⚠️ cfg(test) 测试工具环（`context↔test_utils`、`test_utils↔vfs`）：`test_utils` 是 `#[cfg(test)]` 专用模块，环只存在于测试编译，发布编译无环（测试 mock 的天然模式）

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
    │     │     ├─ SummaryTask（每 6 小时；ADR-017 降频）
    │     │     ├─ EvolutionTask（每日；ADR-017 统一演化）
    │     │     └─ GcTask / SnapshotGcTask / ReminderTask / UsageStatsFlushTask
    │     │
    │     └─ 3.4 axum::serve()              [监听 127.0.0.1:3000]
    │
    ├─ 4. wait_for_server_ready()           [tauri/src/lib.rs]
    │     └─ 轮询 GET /health（30s 超时）
    │
    └─ 5. Tauri 窗口创建
          └─ 加载 gui-vite/dist/index.html
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
          │ 每个 chunk 含 chunk_type (Thought/ToolCall/Observation/Answer/Error/Clarification/Message)
          │
          ▼ ChatService 构造 ChatStreamEvent（chunk_type + tool_call/tool_result/usage）
          │
          ▼ SSE 流式响应：流结束携带 message 边界事件（完整 ChatMessage + segments 时间线）
React Frontend：增量按 chunk_type 渲染；历史/流式共用服务端 segments 时间线统一渲染（ADR-019）
```

> **注意**：规则记录经工具执行管线内联写入（`RuleRecorder`）；记忆/技能/规则/组织形态的演化统一由 `EvolutionTask` 承担（ADR-017；`memory_task.rs`/`rule_task.rs`/`rule_suggester.rs` 已删除，ADR-018 落实）。

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
    ┌─────────────────────┐  ┌──────────────┐  ┌───────────────────┐
    │  SqliteBackend      │  │ ContentStore │  │     VfsSearch     │
    │  (SQLite 存储)      │  │ L0/L1/L2 读写│  │   RRF 融合检索    │
    └─────────┬───────────┘  └──────┬───────┘  └─────────┬─────────┘
              │                     │                     │
              ▼                     ▼                     ▼
    ┌─────────────────────┐  ┌──────────────┐  ┌───────────────────┐
    │ backend/sqlite.rs   │  │ summary/     │  │ vector/lancedb/   │
    │ backend/sqlite_db.rs│  │ SummaryEngine│  │ LanceDbVectorStore│
    │ (共享 SQLite 连接)  │  │ (依赖 ChatService) │  │ (batch/mod/tests)│
    └─────────────────────┘  └──────────────┘  └───────────────────┘
```

> **注**：`SqliteDb` 已移入 `db` 模块（`core/src/db/sqlite_db.rs`，`backend/sqlite_db.rs` 仅 re-export 兼容）——全系统唯一 SQLite 连接（ADR-005 单连接语义），经 `db::Database` 门面统一访问（ADR-020）。`SummaryEngine` 经 `model::ChatService` 生成 L0/L1 摘要，是 vfs→model 的残留单向依赖（非环）。

### 3.5 记忆持久化流程（定时任务）

```
TaskScheduler 触发
    │
    ├─ EvolutionTask::handle()            [core/src/scheduler/tasks/evolution_task.rs]
    │     ├─ 采集：注册表清单 + 会话回忆（session_recall，FTS5）
    │     ├─ 综述：演化智能体（delegate 到 evolution_reviewer）产出 diff 计划
    │     └─ 记账提交：记忆/技能/规则/角色增删改 → VFS 写入
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
  → [Rust] → ChatStreamEvent（透传 chunk_type + tool_call/tool_result/usage）
  → [HTTP SSE] → Frontend：增量按 chunk_type 渲染；边界 message 按服务端
    segments 时间线统一渲染（历史/流式同构，ADR-019）
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
| `VirtualFileSystem` | `core/src/vfs/traits.rs` | 实现 VfsCore+ContentStore+VfsSearch 的类型自动获得 | `server/state.rs`, `agent/coordinator.rs`（session/manager 已迁出 VFS，见 ADR-018） |
| `SqliteBackend` | `core/src/vfs/backend/sqlite.rs` | SQLite 存储后端（具体类型） | `vfs/vfs_impl.rs` |
| `VectorStorage` | `core/src/vfs/vector/traits.rs` | `LanceDbVectorStore`（`vfs/vector/lancedb/`） | `vfs/vfs_impl.rs`, `context/retrieval/` |
| `SessionManager` | `core/src/session/manager.rs` | `PersistentSessionManager`（基于 `SessionStore`，ADR-018） | `server/state.rs`, `agent/coordinator.rs` |
| `SkillExecutor` | `core/src/skills/executor.rs` | `SkillExecutor` | `agent/tool_registry/`（通过 call_skill 工具桥接） |

> **注意**：`ModelServices` 不是 trait，是 `core/src/model/services.rs` 中的 struct，聚合 `Arc<dyn ChatService>` + `Arc<dyn EmbeddingService>` + `Arc<dyn VlmService>`。

### 5.2 HTTP API 契约

详见 [模块功能说明 - API 端点清单](./module-descriptions.md#22-api-端点清单)。

### 5.3 流式数据契约（ChatStreamEvent）

SSE 事件（`server/src/api/chat/types.rs`，逐行 `data:` JSON），完整字段：

```json
{
    "id": "chatcmpl-0",
    "session_id": "uuid",
    "chunk_type": "answer",
    "delta": "增量文本",
    "finish_reason": null,
    "thinking": null,
    "message": null,
    "tool_call": null,
    "tool_result": null,
    "skill_calls": null,
    "usage": null
}
```

- `chunk_type` 7 种（核心 `StreamChunkType`，snake_case 序列化）：
  `thought` / `tool_call` / `observation` / `answer` / `error` / `clarification` / `message`；
  后端从 `AgentStreamChunk.chunk_type` 透传
- 字段按 chunk_type 填充：`delta`（正文增量）、`thinking`（思考增量，与正文分开渲染）、
  `tool_call`（A2 展示契约，前端渲染 tool card）、`tool_result`（耗时/成败 + 结果，
  实时挂到对应卡片）、`skill_calls`、`usage`（完成 chunk 携带本轮 token 用量）
- **消息边界事件（chunk_type=message）**：流开始携带用户消息、流结束携带 assistant
  消息的完整 `ChatMessage`——与历史加载同构（含 `segments` 时间线，ADR-019）。
  前端本地消息 id/内容直接来自服务端统一结构，不做两套形态补丁
- 流结束语义：`[DONE]` 或 `finish_reason` 有值
- 前端解析与归约集中在 `gui-vite/src/lib/chat-stream.ts`（唯一 SSE 解析器 +
  事件归约器，主对话流/追问流共用）

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

以下偏差已在历次迭代中消除（以代码为准核验）：

| 原偏差项 | 原描述 | 现状 |
|--------|------|------|
| SessionManager | `PlaceholderSessionManager`（空实现） | 已删除；仅存 `PersistentSessionManager`，会话经 `SessionStore`（SQLite 权威存储，ADR-018）持久化（`core/src/session/manager.rs`） |
| 前端 Knowledge UI | 无前端 UI 对应 | 已有知识管理面板 `gui-vite/src/components/knowledge/KnowledgePanel.tsx` |
| 前端 Runtime Config UI | 无前端 UI 对应 | 设置面板 `gui-vite/src/components/settings/`（13 Tab）经 `lib/api-client.ts` 覆盖运行时配置 |

当前无已知架构偏差。

---

## 7. 核心架构决策与模块关系

以下 4 个架构决策决定了模块间的职责划分和依赖方向（完整决策清单见 [decisions/](architecture/decisions/)，另含 ADR-005 SQLite 后端、ADR-006 快照例外、ADR-007 依赖环消除）：

### 决策 1：VFS 双层摘要索引 — 底层基础

`vfs/` 是统一存储与检索层。所有上下文（知识库、记忆、技能、规则）通过 VFS 管理，采用三层内容 + 双向量 RRF 融合：

| 层级 | Token | 向量 | 用途 |
|------|-------|------|------|
| L0 Abstract | ~100 | `abstract_vector` | 向量搜索、快速过滤 |
| L1 Overview | ~2K | `overview_vector` | 内容导航、重排序 |
| L2 Detail | 无限制 | — | 完整内容，按需加载 |

**模块影响**：`context/retrieval/` 依赖 `vfs::VfsSearch` 做双层检索；`scheduler/tasks/summary_task.rs` 定时生成 L0/L1 摘要（短内容判断用 `common::token_estimator::estimate_tokens`，非字节数）。`SummaryEngine` 构造仅依赖 `model::ChatService`（2 参数），构成 vfs→model 的残留单向依赖（非环，已接受）。

### 决策 2：StructuredMessage — 单一真相源

`common/types/structured_message.rs` 定义 `StructuredMessage`，贯穿持久化 → 会话组装 → 跟踪 → 统计全流程。`compression_marker` 字段标记压缩锚点。

**模块影响**：`session/manager.rs` 持久化 StructuredMessage；`context/assembler.rs` 将其转为 LLM 传输格式；`agent/loop.rs` 每轮生成后实时写入。

### 决策 3：组件工具化

`agent/tool_registry/`（目录模块）注册内置工具（当前 25 个，完整清单见自动生成的 [`tool-catalog.md`](./tool-catalog.md)，freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁），其中 `call_skill` 桥接到 `skills/executor.rs`。工具执行器按域拆分为 8 个文件（`file_ops.rs` / `code_ops.rs` / `knowledge_ops.rs` / `agent_ops.rs` / `fs_ops.rs` / `lsp_ops.rs` / `symbol_ops.rs` / `test_ops.rs`），公开 API 与 dispatch 不变。工具执行管线为可插拔瀑布（`pipeline.rs`：pre-execute 监听器 / 单调守卫 / post-execute 监听器，A1/A4 吸收），内置可观测性监听器（`observability.rs`）承担统计/Trace/GEPA 历史/规则学习。

**模块影响**：`agent/tool_registry/` 依赖 `skills::SkillExecutor` 实现 call_skill 工具，形成 agent → skills 单向依赖。

### 决策 4：前缀匹配缓存顺序

`context/assembler.rs` 的 `assemble()` 严格遵循固定前缀 → 可变后缀：

```
soul → rules+memories → history(from compression_marker，含当前用户输入)
```

**模块影响**：soul 首次加载后缓存（`context/pipeline.rs` 中的 `cached_soul`）；compression_marker 由 `context/compression/` 模块管理；顺序不可变更以保证 LLM 前缀缓存命中率。

### 决策 5：依赖环消除与共享基础设施归属（ADR-007）

共享基础设施归属**被依赖方/叶模块**，消费方保留 re-export 保留下游兼容：

- `SqliteDb` → `db/sqlite_db.rs`（ADR-020 统一写入门面：全系统唯一 SQLite 连接归属 `db` 层，vfs/observability 经 `db::Database` 门面访问；`vfs/backend/sqlite_db.rs` 保留 re-export 兼容）
- `LoggingConfig` → `common/logging.rs`（`config::LoggingConfig` re-export 仍可用）
- `TokenEstimator`/`estimate_tokens` → `common/token_estimator.rs`（全系统唯一估算入口；`context::compression` re-export）

**模块影响**：3 个依赖环（vfs→observability、context↔observability、common↔config）全部消除；残留单向依赖 `observability→vfs`、`observability→common`、`vfs→model` 均为非环。详见 [ADR-007](architecture/decisions/007-core-dependency-cycle-removal.md)。

---

## 8. 已知待办事项

> 当前无已知架构待办（2026-08-13 架构深化核查：sessions/knowledge/config 的旧 TODO 均已随迭代消除——会话 CRUD 经 `PersistentSessionManager`（`SessionStore` SQLite 权威存储，ADR-018），知识导入/检索经 `KnowledgeIngestor` + `DualLayerRetriever`，配置读写经 `ConfigService` → core `TianyanConfig`）。

---

**文档版本**: 2026-08-30
**最后更新**: 2026-09-06（ADR-029/030 同步：事件订阅、统一循环框架）。此前 2026-08-30（分层重构同步：阶段 1 拆基础类型打破循环（roles/role_store/observability 类型层）、阶段 2 统一写入门面（db::Database + 业务域 Repository）、阶段 2b SQL 收敛（session/stats/trace/execution/usage）、阶段 3 打破 db 层依赖环（SqliteDb 移入 db、SessionRepo 归位 session、RetrievalTrace 引用 common）——生产代码零模块环；依赖清理（cargo-machete 移除 7 个未用依赖））

