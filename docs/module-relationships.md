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
│ 依赖：tianyan-core   │  │ 职责：AI Agent 核心、模型路由、        │
│ 职责：HTTP API、     │──│   存储、记忆、检索、规划、执行          │
│   应用状态管理       │  │                                      │
└──────────────────────┘  └──────────────────────────────────────┘
              ┌────────────────────────┐
              │ gui (tianyan-gui)      │
              │  无 Rust 层项目依赖     │
              │ 职责：Yew WASM 前端    │
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

### 2.1 模块依赖图

```
                         ┌─────────────┐
                         │   config    │  配置管理（被所有模块依赖）
                         └──────┬──────┘
                                │
                         ┌──────┴──────┐
                         │   common    │  通用类型、错误处理
                         └──────┬──────┘
                                │
              ┌─────────────────┼─────────────────┐
              ▼                 ▼                   ▼
        ┌──────────┐    ┌────────────┐      ┌────────────┐
        │  model   │    │  storage   │      │  session   │
        │ 模型服务  │    │ 存储与 VFS │      │ 会话管理   │
        └────┬─────┘    └─────┬──────┘      └─────┬──────┘
             │                │                    │
             │    ┌───────────┼────────────┐       │
             │    ▼           ▼            ▼       │
             │ ┌────────┐ ┌─────────┐ ┌─────────┐ │
             │ │context │ │ storing │ │knowledge│ │
             │ │检索/   │ │ 显提取  │ │知识管理 │ │
             │ │管线/   │ │ trait   │ │         │ │
             │ │规则    │ │         │ │         │ │
             │ └───┬────┘ └─────────┘ └─────────┘ │
             │     │                               │
             │     ▼                               │
             │  ┌──────────────────────┐            │
             └──│       agent          │────────────┘
                │  智能体协调器         │
                │  (ContextPipeline/   │
                │   AgentHarness/       │
                │   AgentSkills)        │
                └──────────┬───────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
        ┌──────────┐ ┌──────────┐ ┌────────────────┐
        │ planner  │ │ executor │ │    skills       │
        │ 任务规划  │ │ 步骤执行  │ │ 技能系统(GEPA)  │
        └──────────┘ └──────────┘ └────────────────┘

   ┌──────────────┐ ┌──────────┐
   │observability │ │  tasks   │
   │ AgentMetrics │ │ 后台任务  │
   └──────────────┘ └──────────┘
              │           │
              └─────┬─────┘
                    ▼
            ┌──────────┐
            │ scheduler│
            │ 任务调度器│
            └──────────┘
```

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
    │     │     ├─ LocalStorageBackend::new()
    │     │     ├─ QdrantVectorStore::new() + initialize()
    │     │     ├─ ModelRouter::with_defaults() + register_services()
    │     │     └─ VirtualFileSystemBuilder::build() + initialize()
    │     │
    │     ├─ 3.2 AppState::new(config, vfs) [server/src/state.rs]
    │     │     ├─ create_summary_service()
    │     │     ├─ create_skill_registry/executor()
    │     │     └─ AgentBuilderFactory::build_agent_or_wizard()
    │     │         ├─ validate_config()
    │     │         ├─ ModelRouter::register_services()
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
用户输入 (Yew Frontend)
    │
    ▼ HTTP POST /api/v1/chat/stream
Axum chat_stream_handler                  [server/src/api/chat/handlers.rs]
    │
    ▼ ChatService::process_message_stream()  [server/src/api/chat/services.rs]
    │  ├─ 持久化用户消息（SessionManager）
    │  ├─ 自动生成会话标题
    │  └─ AgentCoordinator::process_message_stream()
    │
Agent::process_message_stream()           [core/src/agent/coordinator.rs]
    │
    ├─ SessionStateManager::with_state() 获取/创建 SessionState
    ├─ SessionState::add_user_message()
    │
    ├─ ContextPipeline::run()             [core/src/context/pipeline.rs]
    │     ├─ 加载 learned rules（tianyan://agent/learned/）
    │     ├─ DualLayerRetriever::retrieve()（L0+L1 RRF 融合）
    │     └─ ContextCompressor::compress()（Token 预算管理）
    │
    ├─ SessionState::to_planner_context() 构建只读上下文快照
    │
    ├─ Planner::run_with_stream()         [core/src/planner/mod.rs]
    │     ├─ PlannerContext::build_prompt()
    │     ├─ ModelService::chat_completion_stream() [通过 ModelRouter 路由]
    │     ├─ parse_llm_output() → Plan
    │     ├─ SubPlanner 步骤由 Planner 自行递归执行
    │     ├─ 普通步骤委托 ExecutorTrait::execute_steps()
    │     └─ 返回 (PlannerOutput, Vec<PlannerMutation>)
    │
    ├─ SessionState::apply_mutations()    将 Planner 变更写回会话状态
    │
    ├─ 失败时 → RuleRecorder::record() [Failures → Rules → VFS]
    │
    ├─ SessionState::add_turn() + cleanup()
    │
    ├─ 后台异步（不阻塞响应）:
    │     ├─ MemoryExtractionTrait::extract_and_store()
    │     ├─ SkillLearningEngine::learn_from_history()（GEPA 进化）
    │     └─ RuleSuggester::scan_and_promote()（记忆聚类 → 规则）
    │
    └─ AgentStreamChunk 通过 mpsc 通道逐块输出
          │ 每个 chunk 含 chunk_type (Thought/ToolCall/Observation/Answer/Error)
          │
          ▼ ChatService 构造 ChatStreamEvent (含 chunk_type)
          │
          ▼ SSE 流式响应
Yew Frontend 按 chunk_type 差异化渲染
```

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
                    ┌──────────────────────────────────┐
                    │    VirtualFileSystem (trait)       │
                    │  统一接口：CRUD + 搜索 + 摘要      │
                    └───────────┬──────────────────────┘
                                │
                    ┌───────────┴───────────┐
                    ▼                       ▼
        ┌───────────────────┐   ┌───────────────────────┐
        │  StorageBackend   │   │   VectorStorage        │
        │  (本地文件系统)    │   │   (Qdrant 向量数据库)  │
        └───────────────────┘   └───────────────────────┘
                    │                       │
                    ▼                       ▼
        ┌───────────────────┐   ┌───────────────────────┐
        │  UriMapper        │   │  EmbeddingService      │
        │  URI ↔ 文件路径   │   │  (ModelRouter 实现)    │
        └───────────────────┘   └───────────────────────┘
```

### 3.5 记忆持久化流程

```
会话结束 / 定时任务触发
    │
    ├─ MemoryTask::handle()               [core/src/tasks/memory_task.rs]
    │     └─ MemoryExtractionService::extract_from_session()
    │           ├─ ModelService::chat_completion() (提取记忆)
    │           └─ VFS::write_content() (持久化到 tianyan://memory/)
    │
    └─ SummaryTask::handle()              [core/src/tasks/summary_task.rs]
          └─ SummaryService::process_all()
                ├─ VFS::list_all_uris() (扫描条目)
                ├─ VFS::read_content() (读取原始内容)
                ├─ SummaryEngine::generate_abstract/overview()
                │     └─ ModelService::chat_completion()
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

### 4.4 关键数据流路径

```
用户输入
  → [HTTP] → Server API Handler
  → [Rust] → ChatService (持久化 + 标题生成)
  → [Trait] → AgentCoordinator::process_message_stream()
  → [Rust] → ContextPipeline::run()（规则注入 → 检索 → 压缩）
  → [Rust] → SessionState → PlannerContext（上下文转换）
  → [Trait] → PlannerTrait::run()（接收 PlannerContext，返回 PlannerMutation[]）
  → [Trait] → ModelService::chat_completion()
  → [Rust] → ModelRouter::route() → OpenAIClient
  → [Trait] → ExecutorTrait::execute_steps()
  → [Rust] → SessionState::apply_mutations()（应用 Planner 状态变更）
  → [Rust] → AgentStreamChunk (含 chunk_type)
  → [Rust] → ChatStreamEvent (透传 chunk_type)
  → [HTTP SSE] → Frontend 按 chunk_type 差异化渲染
```

---

## 5. 接口契约

### 5.1 核心 Trait 接口

| Trait | 定义位置 | 实现者 | 消费者 |
|-------|---------|--------|--------|
| `AgentCoordinator` | core/src/agent/coordinator.rs | `Agent`, `WizardModeAgent` | server/state.rs, server/api/chat/services.rs |
| `ModelService` | core/src/model/traits.rs | `OpenAIClient`, `OpenAICompatibleClient`, `ModelRouter` | agent/coordinator.rs, planner/mod.rs |
| `EmbeddingService` | core/src/model/traits.rs | `OpenAIClient`, `OpenAICompatibleClient`, `ModelRouter` | storage/vfs/mod.rs, storage/summary.rs |
| `VirtualFileSystem` | core/src/storage/traits.rs | `VirtualFileSystemImpl` | server/state.rs, agent/coordinator.rs |
| `StorageBackend` | core/src/storage/traits.rs | `LocalStorageBackend` | storage/vfs/mod.rs |
| `VectorStorage` | core/src/storage/traits.rs | `QdrantVectorStore` | storage/vfs/mod.rs, context/retrieval/ |
| `MemoryExtractionTrait` | core/src/storage/extractor.rs | `MemoryExtractionService` | agent/coordinator.rs (依赖注入) |
| `PlannerTrait` | core/src/planner/mod.rs | `Planner` | agent/coordinator.rs |
| `ExecutorTrait` | core/src/executor/traits.rs | `Executor` | agent/coordinator.rs, planner/mod.rs |
| `SessionManager` | core/src/session/manager.rs | `PersistentSessionManager`, `PlaceholderSessionManager` | server/state.rs, server/api/sessions/services.rs |
| `SkillExecutor` | core/src/skills/executor.rs | `SkillExecutor` | agent/skill_subsystem.rs, server/api/skills/services.rs |

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

以下为架构文档描述与实际代码实现的偏差：

| 偏差项 | 描述 | 实际状态 | 影响 |
|--------|------|---------|------|
| SessionManager | VFS 持久化会话管理 | `PlaceholderSessionManager`（空实现） | 会话不持久化 |
| knowledge 模块 | 文档导入到知识库 | 后端有 API，但 core 模块未被 server 集成 | 知识管理功能不完整 |
| 前端 Knowledge UI | 知识管理面板 | 无前端 UI 对应 | 后端 4 个端点闲置 |
| 前端 Runtime Config UI | 运行时配置面板 | 无前端 UI 对应 | 后端 3 个端点闲置 |

### 已修复的偏差

| 修复时间 | 修复项 | 修复前 | 修复后 |
|---------|--------|-------|--------|
| 2026-05 | `model/types.rs` 单文件 783 行 | 单文件包含 9 种类型域 | ✅ 拆分为 `model/types/` 9 个子模块 |
| 2026-05 | `model/openai.rs` 单文件 860 行 | 单一文件处理所有 OpenAI 功能 | ✅ 拆分为 `model/openai/` 7 文件（config/client/chat/embedding/vision/stream） |
| 2026-05 | `model/router.rs` 单文件 820 行 | 单一文件包含路由、trait impl、builder、测试 | ✅ 拆分为 `model/router/`（mod + builder + trait_impls） |
| 2026-05 | `skills/executor.rs` 单文件 964 行 | 技能执行与内置处理器混合 | ✅ 拆分为 `skills/handlers/`（6 处理器）+ `skills/registry.rs`（注册表工厂），executor 留存核心 |
| 2026-05 | `skills/learning.rs` 单文件 733 行 | GEPA 引擎类型与逻辑混合 | ✅ 拆分为 `skills/learning/`（mod + types + generator） |
| 2026-05 | `storage/vfs.rs` 单文件 1417 行 | VFS 实现与 Builder 混合 | ✅ 拆分为 `storage/vfs/`（mod + builder） |
| 2026-05 | `storage/local.rs` 单文件 564 行 | 实现与测试混合 | ✅ 拆分为 `storage/local/`（mod + tests） |
| 2026-05 | `knowledge/chunker.rs` 单文件 579 行 | 分块器类型与逻辑混合 | ✅ 拆分为 `knowledge/chunker/`（mod + types） |
| 2026-05 | `knowledge/ingestor.rs` 单文件 683 行 | 导入器与 Builder 混合 | ✅ 拆分为 `knowledge/ingestor/`（mod + builder） |
| 2026-05 | `executor/error.rs` 独立错误文件 | `ExecutorError` 定义在独立文件 | ✅ 合并到 `executor/types.rs` |
| 2026-05 | `planner/error.rs` 独立错误文件 | `PlannerError` 定义在独立文件 | ✅ 合并到 `planner/types.rs` |
| 2026-04 | `ChatStreamEvent.chunk_type` | 后端缺失 | ✅ 后端透传 |
| 2026-04 | regenerate/edit 闭环 | 仅前端本地处理 | ✅ 前后端协同 |
| 2026-04 | `/sessions/{id}/title` | 端点缺失 | ✅ 已添加 |
| 2026-04 | Error 格式不一致 | 前后端格式不同 | ✅ 统一适配 |
| 2026-04 | 类型字段缺失 | 前端缺 total/metadata 等 | ✅ 已补齐 |
| 2026-05 | 记忆模块独立存在 | `memory/` 顶层模块 | ✅ 重构为 `storage::MemoryExtractionTrait`，Agent 依赖注入 |
| 2026-05 | skills in Agent | Agent 持有 executor 但未使用 | ✅ AgentSkills 子系统封装，GEPA 进化引擎集成 |
| 2026-05 | 双层检索未接入 | `retrieve_context` 未调用 | ✅ ContextPipeline 完整集成 |
| 2026-05 | 可观测性缺失 | 无可观测性模块 | ✅ observability 模块 + AgentMetrics |
| 2026-05 | Planner 不支持流式 | 仅 run() | ✅ PlannerTrait::run_with_stream() |
| 2026-05 | 验证门控缺失 | 无执行结果验证 | ✅ VerificationGate + LlmJudge |
| 2026-05 | 失败驱动学习缺失 | 失败无反馈 | ✅ AgentHarness（RuleRecorder + RuleSuggester） |
| 2026-05 | common/types.rs 巨型文件 | 单文件 ~1350 行 | ✅ 拆分为 9 个领域子模块（uri/namespace/content/metadata/embedding/search/message/token/memory） |
| 2026-05 | Planner 直接依赖 SessionState | `run(&mut self, input, &mut SessionState)` | ✅ PlannerContext + PlannerMutation 解耦，PlannerTrait 不依赖 SessionState |
| 2026-05 | learned_rules_* 硬编码常量 | `LEARNED_RULES_TOP_K` / `LEARNED_RULES_MAX_TOKENS` 模块级常量 | ✅ 迁移为 AgentConfig 可配置字段 |
| 2026-05 | Executor 无 trait 抽象 | Agent 直接依赖 `Arc<Executor>` | ✅ 提取 ExecutorTrait，Planner 依赖 `Arc<dyn ExecutorTrait>` |
| 2026-05 | Planner-Executor 依赖方向错误 | `Action/Step` 定义于 planner | ✅ 类型下移至 executor/types.rs，依赖反转 |
| 2026-05 | 测试基础设施薄弱 | 缺少工厂函数和集成测试 | ✅ factory.rs + 10 个 planner_decoupling 集成测试 |

---

## 7. 已知待办事项

| 位置 | 内容 | 优先级 |
|------|------|:---:|
| sessions/services.rs | TODO: 与核心存储集成 | 🔴 |
| knowledge/services.rs | TODO: 与核心摄入管道/检索引擎集成 (4处) | 🔴 |
| config/services.rs | TODO: 与核心配置存储集成 (2处) | 🔴 |
| gui 知识管理面板 | 前端无 Knowledge UI，后端端点闲置 | 🟡 |
| gui 运行时配置 | 前端无 Runtime Config UI | 🟡 |
| 记忆持久化 | MemoryExtractionTrait 已就绪，server 层接入待实现 | 🟡 |

---

**文档版本**: 2026-05-10
**最后更新**: 2026-05-10（新增模块目录拆分历史记录：model/types/openai/router、skills/executor/learning、storage/vfs/local、knowledge/chunker/ingestor 目录化拆分 + error 合并修复清单）
