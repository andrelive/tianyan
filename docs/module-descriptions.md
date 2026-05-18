# 天演模块功能说明

> 本文档详细描述天演项目各模块的功能职责、公开 API 和关键实现。
>
> **相关文档**：[模块关系图](./module-relationships.md) | [系统架构文档](./system-architecture.md)

---

## 目录

- [1. Core 模块 (tianyan-core)](#1-core-模块-tianyan-core)
- [2. Server 模块 (tianyan-server)](#2-server-模块-tianyan-server)
- [3. GUI 模块 (tianyan-gui)](#3-gui-模块-tianyan-gui)
- [4. Tauri 模块 (tianyan-tauri)](#4-tauri-模块-tianyan-tauri)

---

## 1. Core 模块 (tianyan-core)

### 1.1 模块总览

Core 是天演的核心库，提供 AI Agent 的全部基础能力。无外部项目依赖，被 server 和 tauri 两个 crate 依赖。

| 子模块 | 职责 | 关键文件 | 集成状态 |
|--------|------|---------|---------|
| `agent` | 智能体协调器 + Harness/Skills 子系统 + 会话状态 | coordinator.rs, session_state.rs, prompt.rs, tools.rs, harness.rs, skill_subsystem.rs, types.rs | ✅ 已集成 |
| `common` | 通用类型（按领域拆分为子模块）、错误处理、日志 | error.rs, types/ (9个子模块), logging.rs | ✅ 已集成 |
| `config` | 配置管理（TOML + 环境变量 + 向导） | mod.rs, wizard.rs, validation.rs, agent.rs, model.rs, storage.rs | ✅ 已集成 |
| `context` | 上下文工程（检索 + 压缩 + 规则记录/建议 + 上下文管线） | pipeline.rs, retrieval/, compression/, rule_recorder.rs, rule_suggester.rs, assembly.rs | ✅ 已集成 |
| `executor` | 步骤执行器 + 审批工作流 + 验证门控 + LLM-as-Judge | executor.rs, approval.rs, traits.rs, types.rs, judge.rs, verification.rs | ✅ 已集成 |
| `knowledge` | 知识库管理（解析、分块、导入） | parser.rs, chunker/ (mod.rs, types.rs), ingestor/ (mod.rs, builder.rs), image.rs, types.rs | ❌ 未集成 |
| `model` | 模型服务（OpenAI、兼容 API、路由器） | traits.rs, types/ (chat, embedding, vision, model_info, config, service, streaming, api_error, anthropic), openai/ (mod.rs, config.rs, client.rs, chat.rs, embedding.rs, vision.rs, stream.rs), router/ (mod.rs, builder.rs, trait_impls.rs) | ✅ 已集成 |
| `observability` | 可观测性存储（AgentMetrics，Agent 自省） | mod.rs | ✅ 已集成 |
| `planner` | 任务规划器（LLM 驱动的计划生成，支持流式） | mod.rs, config.rs, context.rs, types.rs | ✅ 已集成 |
| `scheduler` | 定时任务调度器 | task_scheduler.rs | ✅ 已集成 |
| `session` | 会话管理（创建、持久化、消息记录） | manager.rs, types.rs | ⚠️ 占位实现 |
| `skills` | 技能定义、执行和学习（GEPA 进化引擎） | definition.rs, executor.rs, manager.rs, types.rs, handlers/ (6 handler 文件), registry.rs, learning/ (mod.rs, types.rs, generator.rs) | ✅ 已集成 |
| `storage` | 存储后端、VFS 和记忆提取 trait | traits.rs, types.rs, vfs/ (mod.rs, builder.rs), local/ (mod.rs, tests.rs), qdrant.rs, summary.rs, summary_service.rs, uri_mapper.rs, extractor.rs | ✅ 已集成 |
| `tasks` | 后台任务（摘要生成、记忆提取） | summary_task.rs, memory_task.rs | ✅ 已集成 |

### 1.2 agent 子模块

**职责**：Agent Loop 架构的对外接口层，管理会话状态、执行 AgentLoop 迭代循环、处理追问，支持流式与非流式两种对话模式。内部封装 ContextPipeline、AgentHarness、AgentSkills 和 AgentLoop 四个子系统。

**核心类型**：

| 类型 | 说明 |
|------|------|
| `Agent` | `AgentCoordinator` 的默认实现，持有 ModelService、VFS、ContextPipeline、AgentHarness、AgentSkills、AgentLoop、ToolRegistry 等组件 |
| `AgentCoordinator` (trait) | 智能体协调器接口，定义 `process_message`、`process_message_stream`、`handle_clarification`、`initialize`、`shutdown` |
| `AgentBuilder` | 构建器模式创建 Agent（构造 AgentLoop + ToolRegistry） |
| `AgentHarness` | Harness 工程子系统，封装 RuleRecorder + RuleSuggester + AgentMetrics |
| `AgentSkills` | 技能子系统，封装 SkillExecutor + SkillRegistry + SkillLearningEngine |
| `AgentLoop` | 智能体迭代循环（LLM 工具调用循环） |
| `ToolRegistry` | 工具注册表，维护 ToolDefinition[] 并并行执行 tool_calls |
| `SessionState` | 会话状态容器（对话历史为唯一真相源，上下文窗口、待持久化记忆） |
| `SessionStateManager` | 多会话状态管理器（线程安全，Arc<RwLock<HashMap>>） |
| `AgentResponse` | 智能体响应（内容、追问、Token 使用量、技能调用信息、处理时间） |
| `AgentStreamChunk` | 流式响应块（含 chunk_type 区分 6 种类型） |
| `StreamChunkType` | 流式块类型：Thought / ToolCall / Observation / Answer / Error / Clarification |
| `StreamEventSender` | 便捷构造各类 AgentStreamChunk 的辅助发送器 |
| `AgentTool` | 工具定义枚举（VFS CRUD + 技能调用） |
| `SkillCallInfo` | 技能调用结果信息（skill_id、success、execution_time_ms、error） |
| `AgentState` | Agent 执行状态（initialized、conversations_processed、total_tokens、retrievals_performed、skills_executed） |

**关键流程**：
- `process_message` → 获取/创建 SessionState → ContextPipeline.run → AgentLoop::run → 处理结果 → 后台异步提取记忆/技能学习/规则提炼
- `process_message_stream` → 流式执行 AgentLoop（run 带 StreamEventSender）→ 通过 mpsc 通道逐块输出 AgentStreamChunk
- `handle_clarification` → 检查 pending_clarification → 重新运行 AgentLoop

**Agent 结构**：
```rust
pub struct Agent {
    config: AgentConfig,
    model_service: Arc<dyn ModelService>,
    vfs: Arc<dyn VirtualFileSystem>,
    context_pipeline: ContextPipeline,
    harness: AgentHarness,
    skills: AgentSkills,
    state: Arc<RwLock<AgentState>>,
    agent_loop: AgentLoop,
    memory_extractor: Option<Arc<dyn MemoryExtractionTrait + Send + Sync>>,
    background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    verification_gate: VerificationGate,
    llm_judge: Option<LlmJudge>,
}
```

### 1.3 model 子模块

**职责**：提供统一的模型服务接口，支持多模型路由和故障转移。

**模块组织**：
- `model/traits.rs` — 核心服务 trait（ModelService, EmbeddingService, VlmService, VisionEncoder）
- `model/types/` — 请求、响应和配置的类型定义，拆分为 9 个子模块（chat, embedding, vision, model_info, config, service, streaming, api_error, anthropic）
- `model/openai/` — OpenAI 兼容 API 客户端，拆分为 6 个文件（mod.rs, config.rs, client.rs, chat.rs, embedding.rs, vision.rs, stream.rs）
- `model/router/` — 智能模型路由器，拆分为 3 个文件（mod.rs, builder.rs, trait_impls.rs）

**核心类型**：

| 类型 | 说明 |
|------|------|
| `ModelService` (trait) | 聊天补全服务接口 |
| `EmbeddingService` (trait) | 文本嵌入服务接口 |
| `VlmService` (trait) | 视觉语言模型服务接口 |
| `VisionEncoder` (trait) | 视觉编码器接口 |
| `ModelRouter` | 智能路由器，同时实现 ModelService + EmbeddingService + VlmService |
| `ModelRouterBuilder` | ModelRouter 的构建器模式实现 |
| `OpenAIClient` | OpenAI API 客户端（默认使用阿里 DashScope qwen3.5-plus） |
| `OpenAICompatibleClient` | OpenAI 兼容 API 客户端 |
| `ChatCompletionRequest/Response` | 聊天补全请求和响应类型 |
| `EmbeddingRequest/Response` | 嵌入请求和响应类型 |

### 1.4 storage 子模块

**职责**：提供统一的虚拟文件系统（VFS），支持分层内容存储（Abstract/Overview/Detail）和向量检索。

**模块组织**：
- `storage/traits.rs` — 核心 trait 定义（VirtualFileSystem, StorageBackend, VectorStorage, ContentLoader）
- `storage/types.rs` — 存储相关类型定义
- `storage/vfs/` — 虚拟文件系统实现，拆分为 mod.rs（VirtualFileSystemImpl）和 builder.rs（VirtualFileSystemBuilder）
- `storage/local/` — 本地文件系统存储后端，拆分为 mod.rs 和 tests.rs
- `storage/qdrant.rs` — Qdrant 向量数据库实现

**核心类型**：

| 类型 | 说明 |
|------|------|
| `VirtualFileSystem` (trait) | VFS 统一接口（CRUD、搜索、摘要） |
| `VirtualFileSystemImpl` | VFS 默认实现 |
| `VirtualFileSystemBuilder` | VFS 构建器 |
| `StorageBackend` (trait) | 存储后端接口 | 
| `LocalStorageBackend` | 本地文件系统存储实现 |
| `VectorStorage` (trait) | 向量存储接口 |
| `QdrantVectorStore` | Qdrant 向量数据库实现（支持多点向量、RRF 融合搜索） |
| `SummaryEngine` | 分层摘要生成引擎（L0: ~100 tokens, L1: ~500 tokens） |
| `SummaryService` | 摘要生成服务（定时任务使用） |
| `UriMapper` | URI 到文件系统路径映射 |

### 1.5 planner 子模块（已废弃）

**职责**：原基于 LLM 的批量任务规划器（Planner-Executor 架构），已废弃。现为 `ClarificationQuestion` 类型的导出壳，保留该模块以维持向后兼容。

**核心类型**：

| 类型 | 说明 |
|------|------|
| `ClarificationQuestion` | 追问问题（question、reason、optional）— 唯一保留的类型 |

**已移除类型**：`Planner`、`PlannerTrait`、`PlannerContext`、`PlannerMutation`、`Plan`、`Step`、`Turn`、`PlannerConfig`、`ContextManager`

### 1.6 executor 子模块（已重构）

**职责**：原纯机械执行 Planner 生成步骤的执行器（ExecutorTrait 模式），已重构。现在提供独立的工具执行函数（`execute_read_file`、`execute_write_file`、`execute_search_code` 等），供 `ToolRegistry` 调用。仍保留 `Action` 和 `ExecutorError` 类型供 `ApprovalWorkflow` 使用。

**模块组织**：
- `executor/executor.rs` — 独立执行函数（`execute_read_file`、`execute_write_file` 等）+ 标记 `#[deprecated]` 的 `Executor` 壳结构体
- `executor/types.rs` — `Action` 和 `ExecutorError`（`Step`、`StepResult`、`FailureHandling` 已移除）
- `executor/approval.rs` — 审批工作流
- `executor/verification.rs` — 验证门控
- `executor/judge.rs` — LLM-as-Judge 语义验证

**核心类型**：

| 类型 | 说明 |
|------|------|
| `Action` | 动作定义（ReadFile / WriteFile / ExecuteCommand / SearchCode），保留供 `ApprovalWorkflow` 使用 |
| `ExecutorError` | 执行器错误类型 |
| `SecurityPolicy` | 安全策略（命令白名单/黑名单、目录限制、command_timeout） |
| `ApprovalWorkflow` | 审批工作流（Safe/Low/Medium/High/Critical 五级风险） |
| `VerificationGate` | 验证门控（执行后自动运行 cargo check/测试验证产出） |
| `LlmJudge` | LLM-as-Judge（语义判断，解析 `-- JUDGMENT: PASS/FAIL/NEEDS_CHANGES`） |

**已移除类型**：`Executor`、`ExecutorTrait`、`Step`、`StepResult`、`FailureHandling`

### 1.7 context 子模块

**职责**：上下文工程系统，包括双层检索、对话压缩、规则记录/建议和统一上下文管线。

**子模块组织**：
- `context/retrieval/` — 意图分析、双层向量检索（RRF 融合）、内容加载（Token 预算）、检索追踪
- `context/compression/` — 对话摘要压缩（分层策略）、Token 估算
- `context/pipeline.rs` — `ContextPipeline`，统一上下文管线（规则注入 → 检索 → 压缩）
- `context/rule_recorder.rs` — `RuleRecorder`，失败驱动规则记录（FailureKind: System / Logic / Safety）
- `context/rule_suggester.rs` — `RuleSuggester`，扫描记忆聚类自动提炼规则
- `context/assembly.rs` — `assemble_prompt`，组装最终 Prompt
- `context/types.rs` — `ContextWindow`（system_prompt + messages + token_usage）

**核心类型**：

| 类型 | 说明 |
|------|------|
| `ContextPipeline` | 统一上下文管线，封装检索→规则注入→压缩的完整流程 |
| `ContextCompressor` | 对话压缩器，保留最近消息 + 分层压缩策略 |
| `CompressionConfig` | 压缩配置（preserve_recent_messages 默认 6） |
| `ContextWindow` | 上下文窗口（system_prompt、messages、token_usage） |
| `RuleRecorder` | 规则记录器，将失败转化为 learned rules 写入 VFS |
| `RuleSuggester` | 规则建议器，扫描记忆聚类并 promote_to_rule |
| `FailureKind` | 失败类型：System / Logic / Safety |
| `DualLayerRetriever` | L0+L1 双层向量检索器 |
| `TokenBudget` | Token 预算管理（按比例分配 L0/L1/L2） |

**集成状态**：`ContextPipeline` 在 Agent 的 `process_message` 和 `process_message_stream` 中完整集成。每次处理消息时自动执行规则注入→检索→压缩流程。

### 1.8 skills 子模块

**职责**：管理 Agent 可调用的技能，包括技能定义、执行、注册、发现和学习（GEPA 进化引擎）。

**模块组织**：
- `skills/definition.rs` + `skills/types.rs` — 技能定义、参数模式和注册表
- `skills/executor.rs` — 技能执行器（参数验证 + 安全检查 + 执行监控）
- `skills/manager.rs` — 技能管理器
- `skills/handlers/` — 6 个内置技能处理文件（file_read, file_write, file_delete, file_list, system_command, http_request），从原 `executor.rs` 分离
- `skills/registry.rs` — 技能注册表工厂（`create_builtin_skills`, `register_builtin_skills`），从原 `executor.rs` 分离
- `skills/learning/` — GEPA 进化引擎，拆分为 mod.rs（核心引擎）、types.rs（类型定义）、generator.rs（技能生成逻辑）

**核心类型**：

| 类型 | 说明 |
|------|------|
| `SkillDefinition` | 技能定义（id、name、description、parameters、executor） |
| `SkillExecutor` | 技能执行接口 |
| `SkillRegistry` | 技能注册表，支持运行时动态注册/发现/执行 |
| `SkillType` | 技能类型：BuiltIn / Custom / Generated |
| `SkillLearningEngine` | GEPA 进化引擎，从执行历史中自动提取可复用技能 |
| `SkillExecutionResult` | 技能执行结果（success、output、error、duration） |
| `ExecutionHistory` | 执行历史记录，用于 GEPA 引擎 |
| `GeneratedSkill` | GEPA 引擎生成的技能 |

**GEPA 进化引擎**：
- **G**enerate：分析执行历史中的成功模式
- **E**volve：生成 GeneratedSkill { id, name, description }
- **P**erfect：通过多次使用优化参数模板
- **A**dapt：根据上下文自动调整技能执行策略

### 1.9 observability 子模块

**职责**：Agent 的可观测性存储，记录执行指标并支持 Agent 自省查询。

**核心类型**：

| 类型 | 说明 |
|------|------|
| `AgentMetrics` | 可观测性存储，记录 Token 消耗、成功率、规则有效性 |
| `ExecutionRecord` | 单次执行记录（timestamp、step_name、success、error、tokens、skills_called） |
| `TokenSummary` | Token 消耗汇总（total_input、total_output、avg_per_conversation） |
| `SuccessRateSummary` | 成功率汇总（total_executions、successes、rate_pct） |
| `RuleEffectivenessSummary` | 规则有效性汇总（total_injections、total_hits、relevance_pct） |
| `HarnessHealth` | Harness 健康摘要（executions、success_rate、rule_hits、pipeline_failures） |

**Agent 自省接口**：
- `query_token_summary()` — Token 消耗历史和平均值
- `query_success_rate()` — 执行总数、成功数、成功率百分比
- `query_rule_effectiveness()` — 规则注入总数、命中数、相关性百分比
- `query_common_failures()` — Top-10 常见失败步骤及错误信息
- `query_harness_health()` — Harness 健康摘要
- `record_execution()` — 记录单次执行
- `record_rule_hit()` — 记录规则命中
- `record_harness_failure()` — 记录 Pipeline 失败

### 1.10 其他子模块

| 子模块 | 核心类型 | 说明 |
|--------|---------|------|
| `config` | `TianyanConfig`, `AgentConfig`, `StorageConfig`, `ModelsConfig`, `ConfigStatus`, `WizardConfig` | 全局配置管理，支持 TOML + env，含配置向导。`AgentConfig` 含 `learned_rules_top_k`(默认5) 和 `learned_rules_max_tokens`(默认800) 字段 |
| `common` | `TianyanError`, `ErrorCategory`, `Message`, `TianyanUri`, `Embedding`, `TokenUsage`, `MemoryEntry`, `AgentPath` | 通用错误（含 `ErrorCategory` 分类：is_retryable/is_user_facing/error_code）、URI、向量、消息、记忆类型。`types/` 拆分为 9 个领域子模块（uri, namespace, content, metadata, embedding, search, message, token, memory） |
| `session` | `Session`, `SessionManager` (trait), `PersistentSessionManager` | 会话管理，支持 VFS 持久化 |
| `storage` | `VirtualFileSystem`, `VirtualFileSystemBuilder`, `LocalStorageBackend`, `QdrantStorage`, `MemoryExtractionTrait`, `StorageBackend` (trait), `VectorStorage` (trait) | 存储后端、VFS（vfs/ 拆分为 mod + builder）和记忆提取 trait。local/ 拆分为 mod + tests |
| `knowledge` | `KnowledgeIngestor`, `KnowledgeIngestorBuilder`, `CompositeParser`, `DocumentChunker`, `ChunkingConfig`, `ImageProcessor` | 知识库导入（完全未集成，API 端点使用独立逻辑）。chunker/ 拆分为 mod + types，ingestor/ 拆分为 mod + builder |
| `scheduler` | `TaskScheduler`, `TaskHandler` (trait), `TaskContext` | 定时任务调度框架 |
| `tasks` | `SummaryTask`, `MemoryTask` | 后台任务实现 |

## 2. 集成状态汇总

| 模块 | 状态 | 说明 |
|------|------|------|
| agent | ✅ 完整集成 | Agent、Harness、Skills、AgentLoop、ToolRegistry、SessionState、ContextPipeline 全部集成 |
| model | ✅ 完整集成 | ModelRouter + OpenAI 已集成 |
| planner | ⚠️ 已废弃 | 仅保留 `ClarificationQuestion` 类型导出壳 |
| executor | ⚠️ 已重构 | 独立执行函数 + `Action`/`ExecutorError`，`ExecutorTrait`/`Step`/`StepResult` 已移除 |
| context | ✅ 完整集成 | ContextPipeline 在 Agent 中完整集成 |
| skills | ✅ 完整集成 | 含 GEPA 进化引擎 |
| observability | ✅ 完整集成 | AgentMetrics 作为 AgentHarness 的一部分 |
| storage | ✅ 完整集成 | VFS + Local + Qdrant 多后端 |
| tasks | ✅ 完整集成 | SummaryTask + MemoryTask 定时执行 |
| scheduler | ✅ 完整集成 | TaskScheduler 定时调度 |
| config | ✅ 完整集成 | 配置加载器和验证器 |
| common | ✅ 完整集成 | 错误类型和通用工具 |
| session | ⚠️ 占位实现 | 会话持久化尚未完全实现 |
| knowledge | ❌ 未集成 | 代码编写完成，但未在 Agent 中使用 |

**文档版本**: 2026-05-10
**最后更新**: 2026-05-10（全模块目录拆分：model/types → 9 子模块, model/openai → 6 文件, model/router → 3 文件, skills/executor → handlers + registry, skills/learning → 3 文件, storage/vfs → mod + builder, storage/local → mod + tests, knowledge/chunker → mod + types, knowledge/ingestor → mod + builder, executor/error → 合并到 types, planner/error → 合并到 types）

---

## 2. Server 模块 (tianyan-server)

### 2.1 模块总览

Server 是天演的 HTTP API 层，基于 Axum 框架，提供 REST API、SSE 流式传输和静态文件服务。采用领域驱动设计（DDD），每个领域包含 routes/handlers/services/types 四层。

| 子模块 | 职责 | 关键文件 |
|--------|------|---------|
| `api/chat` | 对话领域（流式 + 非流式 + regenerate + edit） | handlers.rs, routes.rs, services.rs, types.rs |
| `api/sessions` | 会话管理（CRUD + 标题更新） | handlers.rs, routes.rs, services.rs, types.rs |
| `api/knowledge` | 知识管理（摄入 + 检索） | handlers.rs, routes.rs, services.rs, types.rs |
| `api/skills` | 技能执行（列表 + 执行 + 状态查询） | handlers.rs, routes.rs, services.rs, types.rs |
| `api/config` | 运行时配置（GET + PUT） | handlers.rs, routes.rs, services.rs, types.rs |
| `api/config/wizard` | 配置向导（状态 + 保存 + 连接测试） | wizard_handlers.rs, wizard_routes.rs, wizard_services.rs, wizard_types.rs |
| `api/shared` | 共享类型、错误处理、响应格式 | error.rs, response.rs, types.rs |
| `state` | 应用状态管理（Agent、VFS、SessionManager 等生命周期） | state.rs |
| `agent_builder` | Agent 构建工厂（构建 + 验证 + 降级） | agent_builder.rs |
| `core_bridge` | Core 类型转换桥接 | core_bridge.rs |

### 2.2 API 端点清单

#### 配置向导（/api，无版本前缀）

| 端点 | 方法 | 功能 | 前端状态 |
|------|------|------|:---:|
| `/config/status` | GET | 配置状态检查 | ✅ |
| `/config/wizard` | POST | 保存配置向导 | ✅ |
| `/config/test-connection` | POST | 测试 API 连接 | ⚠️ 已定义未使用 |

#### 对话领域（/api/v1/chat）

| 端点 | 方法 | 功能 | 前端状态 |
|------|------|------|:---:|
| `/chat` | POST | 非流式聊天 | ⚠️ 已定义未使用 |
| `/chat/stream` | POST | 流式聊天 (SSE，含 chunk_type) | ✅ |
| `/chat/regenerate` | POST | 重新生成回复 | ✅ |
| `/chat/edit` | POST | 编辑消息后重新生成 | ✅ |

#### 会话管理（/api/v1/sessions）

| 端点 | 方法 | 功能 | 前端状态 |
|------|------|------|:---:|
| `/sessions` | GET | 列出所有会话 | ✅ |
| `/sessions` | POST | 创建会话 | ⚠️ API 已定义，前端未调用 |
| `/sessions/{id}` | GET | 获取会话详情（含消息） | ✅ |
| `/sessions/{id}` | DELETE | 删除会话 | ✅ |
| `/sessions/{id}/messages` | GET | 获取会话消息（轻量） | ⚠️ 可优化使用 |
| `/sessions/{id}/title` | POST | 更新会话标题 | ⚠️ API 已就绪，前端待接入 UI |

#### 知识管理（/api/v1）

| 端点 | 方法 | 功能 | 前端状态 |
|------|------|------|:---:|
| `/ingest` | POST | 上传文档（Multipart） | ❌ |
| `/ingest/{job_id}/status` | GET | 导入任务状态 | ❌ |
| `/search` | GET | 知识库检索 | ❌ |
| `/search/suggestions` | GET | 检索建议 | ❌ |

#### 技能执行（/api/v1/skills）

| 端点 | 方法 | 功能 | 前端状态 |
|------|------|------|:---:|
| `/skills` | GET | 列出可用技能 | ✅ |
| `/skills/{id}/execute` | POST | 执行技能 | ✅ |
| `/skills/{id}/jobs/{job_id}/status` | GET | 查询执行状态 | ✅ |

#### 运行时配置（/api/v1/config）

| 端点 | 方法 | 功能 | 前端状态 |
|------|------|------|:---:|
| `/config` | GET | 获取完整运行时配置 | ❌ |
| `/config` | PUT | 更新运行时配置 | ❌ |
| `/config/{section}` | GET | 获取配置片段 | ❌ |

### 2.3 关键组件

**AppState** (`state.rs`)：
- 持有 `Agent`、`Config`、`VFS`、`SessionManager`、`SkillRegistry`、`SkillExecutor`、`SummaryService`
- 使用 `PlaceholderMemoryCoordinator` 和 `PlaceholderSessionManager`（标记 TODO 待与核心存储集成）
- 支持配置热重载（`update_config` → `reload_agent`）

**AgentBuilderFactory** (`agent_builder.rs`)：
- 静态工厂类，负责 Agent 实例的创建和配置验证
- `build_agent_or_wizard`：构建失败时降级为 `WizardModeAgent`

**core_bridge** (`core_bridge.rs`)：
- `convert_message`：API 消息类型 → Core 消息类型
- `convert_token_usage`：Core Token 使用量 → API Token 使用量

**错误处理** (`shared/error.rs`)：
- `ApiError` 统一错误类型（NotFound / BadRequest / Internal / Config / Agent）
- `ErrorResponse { error: String }` 统一错误响应格式
- 前端已适配此格式进行错误解析

---

## 3. GUI 模块 (tianyan-gui)

### 3.1 模块总览

GUI 是天演的 Yew (Rust WASM) 前端，编译为 WebAssembly 在浏览器中运行，采用 Yew Reducible 模式管理全局状态。

| 子模块 | 职责 | 关键文件 |
|--------|------|---------|
| `api/chat` | 聊天 API 客户端（流式 SSE + 非流式 + regenerate/edit） | chat.rs |
| `api/sessions` | 会话 API 客户端（CRUD + 标题更新） | sessions.rs |
| `api/skills` | 技能 API 客户端（列表 + 执行 + 状态轮询） | skills.rs |
| `components/chat` | 聊天面板组件（消息列表、输入框、编辑、重新生成、复制） | mod.rs |
| `components/sidebar` | 侧边栏组件（会话列表、新建会话、视图切换） | mod.rs |
| `components/skills` | 技能中心面板（技能列表、参数输入、执行、结果展示） | mod.rs |
| `components/settings` | 设置面板组件（主题、字号、API 地址） | mod.rs |
| `components/config_wizard` | 配置向导组件（六步向导） | mod.rs, api.rs, types.rs |
| `state` | 全局状态管理（Yew Reducible） | mod.rs |

### 3.2 前端 API 覆盖情况

| 后端功能域 | 前端 API 模块 | 状态 |
|-----------|-------------|------|
| 聊天对话 | `api/chat` | ✅ 完整实现（流式 + regenerate + edit） |
| 会话管理 | `api/sessions` | ✅ 完整实现（CRUD + title 更新） |
| 技能执行 | `api/skills` | ✅ 完整实现（列表 + 执行 + 状态轮询） |
| 知识管理 | - | ❌ 未实现 |
| 运行时配置 | - | ❌ 未实现 |
| 配置向导 | `config_wizard/api` | ✅ 完整实现 |

### 3.3 状态管理

**AppState** 使用 Yew 的 `use_reducer` 模式，包含 14 种 Action：

| Action | 说明 | 使用状态 |
|--------|------|:---:|
| `SetView` | 切换视图（Chat/Skills/Settings） | ✅ |
| `SetCurrentSession` | 设置当前会话 ID | ✅ |
| `SetSessions` | 设置会话列表 | ✅ |
| `AddSession` | 添加会话到列表 | ⚠️ |
| `RemoveSession` | 从列表移除会话 | ✅ |
| `SetMessages` | 设置消息列表 | ✅ |
| `AddMessage` | 追加消息 | ✅ |
| `UpdateLastMessage` | 追增最后一条消息内容 | ✅ |
| `AppendStreamMessage` | 追加流式过程消息（Thought/ToolCall/Observation） | ✅ |
| `AppendSkillCalls` | 附加技能调用信息 | ✅ |
| `SetStreamStatus` | 设置流状态（Idle/Streaming/Error） | ✅ |
| `ToggleSidebar` | 切换侧边栏 | ✅ |
| `UpdateSettings` | 更新本地设置 | ✅ |
| `ClearMessages` | 清空消息 | ✅ |
| `SetSkills` | 设置技能列表 | ✅ |
| `RegenerateFrom` | 从指定索引截断消息 | ✅ |
| `EditMessage` | 编辑指定消息内容 | ✅ |
| `DeleteMessagesFrom` | 删除指定索引之后的消息 | ✅ |

### 3.4 流式响应处理

前端 `StreamHandler` 通过 `ReadableStream` API 处理 SSE 流式响应，支持：

- **6 种 chunk_type 差异化渲染**：
  - `Thought` → 独立消息气泡，💭 图标，"思考中"标签
  - `ToolCall` → 独立消息气泡，🔧 图标，"工具调用"标签
  - `Observation` → 独立消息气泡，👁️ 图标，"观察结果"标签
  - `Answer` / `Clarification` → 追加到最后一条助手消息
  - `Error` → 错误内容追加
- **可中断**：通过 `AbortController` 实现"停止生成"
- **技能调用**：skill_calls 附加为调用卡片

---

## 4. Tauri 模块 (tianyan-tauri)

### 4.1 模块总览

Tauri 是天演的桌面应用壳，负责窗口管理和服务器生命周期。

| 文件 | 职责 |
|------|------|
| `lib.rs` | 应用入口：初始化日志、加载配置、启动服务器、健康检查、创建窗口 |
| `server.rs` | 服务器启动封装：`start_axum_server` 和 `start_axum_server_blocking` |
| `main.rs` | 二进制入口：调用 `tianyan_tauri_lib::run()` |

### 4.2 启动参数

| 参数 | 值 | 说明 |
|------|---|------|
| 服务器地址 | `127.0.0.1:3000` | 固定值 |
| 健康检查超时 | 30 秒 | `HEALTH_CHECK_TIMEOUT` |
| 健康检查间隔 | 500 毫秒 | `HEALTH_CHECK_INTERVAL` |
| 日志目录 | `%APPDATA%/com.tianyan.app/logs/` | Windows 默认路径 |

### 4.3 与其他模块的交互

```
Tauri lib.rs::run()
    │
    ├─→ tianyan::config::TianyanConfig  (读取配置)
    ├─→ tianyan_server::start_server()  (启动 HTTP 服务器)
    │     └─ initialize_vfs_for_app → AppState::new → axum::serve
    ├─→ reqwest GET /health             (健康检查)
    └─→ Tauri WebView → gui/dist/       (加载前端)
```

---

**文档版本**: 2026-04-27
**最后更新**: 2026-04-27（前后端接口对齐：chunk_type、regenerate/edit、错误格式、类型字段）
