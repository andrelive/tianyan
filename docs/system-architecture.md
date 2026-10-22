# 天演 (Tianyan) 系统架构文档

本文档描述天演本地智能代理系统的整体架构、核心决策和组件间交互——**README 级总览**。
实现细节与最新事实以代码为准，权威文档：

> **相关文档导航**：
> - [模块索引](./architecture/module-map.md) — 模块清单与位置
> - [模块关系图](./module-relationships.md) — 模块间依赖关系、调用流程和数据交互机制
> - [模块功能说明](./module-descriptions.md) — 各模块功能职责、公开 API 和集成状态
> - [架构决策记录](./architecture/decisions/) — ADR-001~030（含修订关系）
> - [工具目录](./architecture/tool-catalog.md) — 自动生成，与代码一致

> **最后更新**: 2026-09-06（同步 ADR-028/029/030：统一事件推送、事件订阅与快照恢复、统一 Agent 循环框架）

## 目录

- [1. 系统概述](#1-系统概述)
- [2. 应用架构](#2-应用架构)
- [3. 核心架构决策](#3-核心架构决策)
- [4. Agent Loop 架构](#4-agent-loop-架构)
- [5. 完整数据流图](#5-完整数据流图)
- [6. 配置结构](#6-配置结构)

---

## 1. 系统概述

天演 (Tianyan) 是一个基于 Rust 开发的本地 AI 助手桌面应用，采用 **Tauri v2 + React/TypeScript + Axum** 技术栈。

**核心能力**：

| 能力 | 说明 |
|------|------|
| **Agent Loop 架构** | LLM 在循环中自主调用工具或直接回答，支持并行工具调用和追问中断 |
| **VFS 双层摘要索引** | 三层内容（L0 Abstract / L1 Overview / L2 Detail）+ 双向量 RRF 融合检索 |
| **StructuredMessage** | 单一真相源：持久化、会话组装、压缩跟踪、Token 统计 |
| **组件工具化** | 30 个 OpenAI function calling 兼容工具，`call_skill` 读 VFS 技能文档 |
| **前缀匹配缓存** | soul+rules+memories 固定前缀 → history 可变后缀，利用 LLM Provider 缓存 |
| **技能系统** | 技能 = VFS 方法论文档（planning 预置 + GEPA 进化引擎自动学习） |
| **流式响应** | SSE 流式输出，7 种 chunk_type（含 message 边界事件）；历史/流式共用服务端权威时间线（segments，ADR-019） |

---

## 2. 应用架构

### 2.1 四层架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Tauri Desktop App                        │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              React Frontend (gui-vite/)                      │  │
│  │  chat / sidebar / skills / settings / config_wizard   │  │
│  └───────────────────────┬───────────────────────────────┘  │
│                          ↓ HTTP + SSE                       │
│  ┌───────────────────────┴───────────────────────────────┐  │
│  │              Axum Server (server/)                    │  │
│  │  REST API + SSE streaming + Core Bridge               │  │
│  └───────────────────────┬───────────────────────────────┘  │
│                          ↓ Rust Calls                       │
│  ┌───────────────────────┴───────────────────────────────┐  │
│  │              Core Library (core/)                     │  │
│  │  Agent | ContextPort | VFS | Skills | Scheduler       │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 项目结构

```
tianyan/
├── core/           # 核心库 (lib: tianyan)
│   ├── agent/      # Agent 协调器 + AgentLoop + ToolRegistry + 会话状态
│   ├── common/     # 通用类型、错误处理、日志
│   ├── config/     # 配置结构 + 全局槽位（TianyanConfig/子 Config/预设表/get_config·set_config）
│   │               # 加载/持久化/迁移/校验/向导 → service/config（ADR-049 期 3-T4）
│   ├── context/    # 上下文工程契约 + 组装机制（assembler、port.rs、assembly.rs）；策略实现 → plugins/policy-context
│   ├── executor/   # [已废弃] 独立执行函数 + 审批工作流 + 验证门控
│   ├── knowledge/  # 知识库导入和解析（parser、image、ingestor/）
│   ├── model/      # 模型服务契约 + 数据（实现已出 core，ADR-049 期 2 序 6）
│   │   ├── traits.rs    # ChatService/EmbeddingService/VlmService/ServiceDiscovery（S 级）
│   │   ├── types/       # 请求/响应/工具载荷
│   │   ├── spec.rs      # 模型规格 + 上下文窗口（内核动态 max_tokens / 压缩窗口联动）
│   │   ├── retry.rs     # 重试策略与可重试谓词（两个 provider 插件共用）
│   │   ├── assembly.rs  # 装配原语：ProviderLlmServices / ProviderEmbedServices / ConnectionProbe
│   │   ├── unconfigured.rs # 「能力未配置」降级服务
│   │   └── usage.rs     # EmbeddingUsageSink 契约（实现方在 server）
│   │   （客户端 → plugins/provider-client；聊天/视觉 → plugins/provider-llm；
│   │     嵌入 → plugins/provider-embed；容器 ModelServices → server/src/model_services.rs）
│   ├── observability/ # AgentMetrics 可观测性存储和自省接口
│   ├── scheduler/  # 定时任务调度机制（TaskHandler/TaskContext/TaskScheduler + 装配原语 + EvolutionReviewExecutor 契约）；core 无具体任务实现（→ plugins/task-builtin、plugins/task-evolution）
│   ├── session/    # 会话管理（SessionStore SQLite 权威存储 + PersistentSessionManager + SessionRecall）
│   ├── skills/     # 技能系统（definition、executor、handlers/、learning/）
│   └── vfs/        # 虚拟文件系统（traits、vfs_impl、backend/、vector/、summary/）
├── server/         # Axum HTTP 服务
│   └── src/
│       ├── api/            # 领域 API（chat/sessions/knowledge/skills/config）
│       ├── state.rs        # AppState（持有 Agent、VFS、SessionManager）
│       ├── agent_builder.rs # Agent 构建工厂
│       └── lib.rs          # bootstrap_app_vfs() + 服务器启动
├── service/
│   └── config/     # 服务层配置 crate（tianyan-service-config）
│       └── src/loader.rs     # 加载 / 持久化（段保全）/ 路径解析 / 段位注入
│           src/migration.rs  # 数据目录搬迁（ADR-023）
│           src/validation.rs # 校验编排（向导 / 配置状态）
│           src/wizard.rs     # 连接测试 DTO
├── gui-vite/            # React TypeScript 前端
│   └── src/
│       ├── api/            # API 客户端
│       ├── components/     # UI 组件（chat/sidebar/skills/settings/config_wizard）
│       └── state/          # Zustand 全局状态
├── tauri/          # Tauri 桌面包装
│   └── src/
│       ├── lib.rs          # 入口：日志→配置→启动服务器→健康检查→Tauri 窗口
│       └── server.rs       # start_axum_server()
└── docs/           # 项目文档
```

### 2.3 API 端点

服务器提供以下 REST API 端点（已接入前端的主要端点；完整路由以 `server/src/api/*/routes.rs` 为准）：

| 领域 | 端点 | 方法 | 说明 |
|------|------|------|------|
| 对话 | `/api/v1/chat/stream` | POST | 对话启动（收敛为开关：校验 + 启动 AgentLoop 立即返回，ADR-028；流式输出经 `GET /events`） |
| 对话 | `/api/v1/chat/answer` | POST | ask_user 追问回答提交 |
| 事件 | `/api/v1/events` | GET (SSE) | 统一事件流（message / task_status / command_output / chat_stream / snapshot，ADR-028/029） |
| 事件 | `/api/v1/events/subscribe` | POST | 事件订阅（快照恢复触发点，ADR-029） |
| 会话 | `/api/v1/sessions` | GET/POST | 会话列表 / 创建 |
| 会话 | `/api/v1/sessions/{id}/messages` | GET | 会话消息（历史/对齐快照） |
| 任务 | `/api/v1/tasks` | GET | 后台任务列表（委托/终端统一） |
| 任务 | `/api/v1/tasks/{id}/cancel` | POST | 取消任务 |
| 任务 | `/api/v1/tasks/{id}/log` | GET | 命令输出日志分页（ADR-028） |
| 知识 | `/api/v1/knowledge/ingest` | POST | 知识摄入（VFS L2 + 摘要） |
| 技能 | `/api/v1/skills` | GET | 技能列表（L0 渐进披露） |
| 配置 | `/api/v1/config` | GET/PUT | 配置读写（ADR-023：`~/.tianyan/tianyan.toml` 固定） |
| 配置 | `/api/v1/config/status` | GET | 配置状态（向导门控） |
| 工作区 | `/api/v1/workspace/{tree,read,diff,apply-patch,apply-edit}` | GET/POST | 工作区浏览/编辑 |
| 审批 | `/api/v1/approval/{status,respond}` | GET/POST | 应用层审批（wait_for_approval） |
| 健康 | `/health` | GET | 健康检查 |

---

## 3. 核心架构决策

天演有 4 个核心架构决策，贯穿整个系统设计。

### 3.1 决策 1: VFS 双层摘要索引 —— 基础机制

VFS 是所有上下文（知识库、记忆、技能、规则）的统一存储与检索层，采用**三层内容 + 双向量 RRF 融合**：

| 层级 | 名称 | Token | 向量 | 用途 |
|------|------|-------|------|------|
| L0 | Abstract | ~100 | `abstract_vector` | 向量搜索、快速过滤 |
| L1 | Overview | ~2K | `overview_vector` | 内容导航、重排序 |
| L2 | Detail | 无限制 | — | 完整内容，按需加载 |

**检索流程**：查询文本 → embed → LanceDB RRF 融合搜索 `abstract_vector` + `overview_vector` → `ContentLoadStrategy::from_score()` 按分数分层加载（>0.85→L2, >0.6→L1, 其他→L0）。

**此机制是项目底层基础**，其他设计必须妥协于它：

- **知识库不做 chunk**：完整文档直接写入 VFS L2 Detail，`SummaryEngine` 生成 L0/L1，双层检索替代传统 chunk-based RAG。`Chunker` 已移除。（`core/src/knowledge/ingestor/`）
- **技能渐进式披露**：技能存于 `tianyan://skill/` 命名空间。L0 Abstract 快速发现，L2 Detail（`skill.md`）按需加载。`description_embedding` 向量支持语义检索。
- **图像双通道**：VLM 生成文本描述 → 文本 embedding 用于 L0/L1，同时 `embed_image()` 生成 `visual_vector` 用于视觉相似度搜索。
- 所有命名空间（User、Session、Memory、Knowledge、Agent、Skill）共享同一套机制。

关键文件：`core/src/vfs/traits.rs`（`VfsSearch` trait）、`core/src/vfs/vfs_impl.rs`（`search()`）、`plugins/storage-vector`（LanceDB + RRF 融合检索原语）、`plugins/policy-context/src/context/retrieval/retriever.rs`（`DualLayerRetriever`）。

### 3.2 决策 2: StructuredMessage —— 核心数据结构

`StructuredMessage` 是贯穿持久化、会话组装、跟踪、统计的单一真相源：

```rust
pub struct StructuredMessage {
    pub id: String, pub parent_id: Option<String>, pub role: MessageRole,
    pub parts: Vec<Part>,                  // Text | Reasoning | ToolCall | ToolResult
    pub tokens: DetailedTokenUsage,        // input/output/reasoning/cache
    pub cost: f64, pub model_id: Option<String>, pub time: MessageTime,
    pub session_id: String, pub finish: Option<String>,
    pub compression_marker: bool,          // ★ 会话压缩锚点
}
```

**四个核心职责**：

1. **持久化**：完整消息写入 SQLite 权威存储（ADR-018，`SessionStore`，单事务原子取号 + FTS 同步）。`AgentLoop` 每产生一条消息，实时调 `SessionManager::add_structured_message()` 落盘。
2. **会话组装**：存储与传输分离 — `StructuredMessage`（存储层）↔ `Message`（传输层）。`ContextAssembler::assemble()` 转换。API 层序列化为 `ChatMessage` 时按 parts 顺序生成 `segments` 时间线（思考/正文/工具调用真实到达顺序），随历史加载与流式边界事件送达前端——渲染层不再本地重排（ADR-019）。
3. **会话跟踪**：`compression_marker` 标记压缩产生的摘要消息。加载会话时反向扫描到最近 marker，只加载 marker 及之后的的消息（旧消息仍完整保留在 SQLite 中）。
4. **Token 统计**：LLM 响应 `TokenUsage` → `AgentLoop` 捕获 → `DetailedTokenUsage` → 持久化 → 聚合到 `AgentState.total_tokens`。

### 3.3 决策 3: 组件工具化（已实施，ADR-003）

知识库查询、技能调用等能力封装为 OpenAI function calling 兼容的工具，由 LLM 通过 `tool_call` 自主调用。

当前 `ToolRegistry` 注册 34 个内置工具（完整清单见自动生成的 [`tool-catalog.md`](architecture/tool-catalog.md)）：`read_file`、`write_file`、`apply_edit`、`apply_patch`、`execute_command`、`search_vfs`、`call_skill`、`run_tests`、`discover_tests`、`ask_user`、`self_check`、`knowledge_ingest`、`web_search`、`web_fetch`、`delegate_to_agent`、`glob`、`list_dir`、`symbol_outline`、`repo_map`、`task_status`、`task_cancel` 等。其中 `call_skill` 读 VFS 技能文档（方法论文档，无执行语义）；`delegate_to_agent` 走统一循环框架（ADR-030：子代理 = AgentLoop 实例 + `TurnPolicy` 委托策略，流式路径 + 消息落库即广播）；`web_search`/`web_fetch` 提供网页感知（后端 + SSRF 防护 + 缓存）。

### 3.4 决策 4: 上下文组装前缀匹配原则

`ContextAssembler::assemble()` 严格遵循**固定前缀 + 可变后缀**顺序：

```
soul → rules+memories → history(from compression_marker，含当前用户输入)
```

- **固定前缀（soul + rules + memories）**：会话期间不变，LLM Provider 可利用前缀缓存只计算一次。
- **可变后缀（history）**：从最近 `compression_marker` 开始拼接，marker 前的旧消息不纳入本次输入。
- **绝对禁止**将 soul/rules/memories 放在 history 之后——会破坏前缀缓存。

实现位置：`core/src/context/assembler.rs` 的 `assemble()` 方法，顺序不可变更。

---

## 4. Agent Loop 架构

### 4.1 Agent 组件结构

```
Agent :: process_message(session_id, msg) / process_message_stream（多模态 + 流式）
  │
  ├─ SessionManager::get_session(session_id) → SessionState
  │     └─ SessionStore::load()：SQLite 按 seq 查询 → compression_marker 截断
  │
  ├─ ContextPort::load_injectable() + compress_for_session()（实现：plugins/policy-context）
  │     ├─ ContextAssembler::assemble()：soul → rules+memories → history（含当前用户输入）
  │     ├─ DualLayerRetriever::retrieve()：L0+L1 RRF 融合检索
  │     └─ compress_if_needed()：compression_marker 后超阈值 → LLM 摘要压缩
  │
  ├─ AgentLoop::run / run_stream（统一循环框架，ADR-030）
  │     ├─ TurnPolicy 配置：工具执行器 / 持久化 FTS 开关 / max_turns 策略 / 请求工具集
  │     ├─ 主 agent = 默认策略（全局工具 + 索引 FTS）；子代理 = 委托策略（角色过滤 + 不索引 + 流式）
  │     ├─ LLM 返回 tool_calls → ToolRegistry::execute_parallel()（可插拔管线）
  │     ├─ 收尾统一：无工具调用 = 完成（子代理 submit_result 降级为可选结果落盘工具）
  │     └─ 每条消息实时落库（子代理走 add_structured_message_no_fts + 广播）
  │
  └─ 后台异步（不阻塞响应）：
        ├─ EvolutionTask（ADR-017，plugins/task-evolution）：每日演化综述（记忆/技能/规则/组织形态统一演化）
        └─ Scheduler 其他定时任务：SummaryTask / GcTask / SnapshotGcTask / ReminderTask
```

### 4.2 ToolRegistry 实现

`ToolRegistry` 维护所有可用工具的 JSON Schema 定义，`execute_parallel()` 通过 tokio JoinSet 并行执行（同轮多个 tool_call 并发）。
**工具执行走可插拔管线**（`tool_registry/pipeline.rs`，DSH 瀑布吸收）：pre-execute 监听器（fail-closed）→ 单调守卫（只允许拒绝）→ 执行 → post-execute 监听器（观察/改写结果）。
**管线出口的两个内置监听器已出 core**（ADR-049 期 3-T5，`plugins/tool-pipeline`）：

- `observability.rs` 承担 usage stats / Trace / GEPA 执行历史 / 失败规则学习；
- `spill.rs` 承担超预算工具输出落盘（ADR-047），**必须注册在观测监听器之后**——数据层要看到原始结果，落盘会替换 result。未装配时降级为「大输出不落盘 + warn 一次」（best effort，与 ADR-047 一致）。

装配经 `AssemblyServices::tool_pipeline_factory` 注入（server 库不认识该插件）。

- **25 个内置工具**：基于 `#[derive(JsonSchema)]` 参数结构体自动生成 Schema；完整清单见自动生成的 [`docs/architecture/tool-catalog.md`](architecture/tool-catalog.md)（`scripts/gen-tool-catalog.ps1` 生成，freshness 挂入 `scripts/test.ps1` lint 门禁）
- **展示契约（A2）**：每个工具注册 `ToolPresentation` 展示意图（read/terminal/diff/search/web/...），随 SSE `tool_call` 事件透传，前端 `ToolCallCard` 数据驱动渲染——工具与 UI 解耦
- **安全策略**：`SecurityPolicy` 控制命令白名单/黑名单，文件操作前检查
- **审批工作流**：`ApprovalWorkflow` 五级风险（Safe/Low/Medium/High/Critical）
- **验证门控**：`VerificationGate` 控制执行后自动验证（cargo check/test）
- **LLM-as-Judge**：`LlmJudge` 对执行结果进行语义验证
- **动态工具**：外部 MCP 服务器工具经 `DynamicToolExecutor` 注册进注册表（对 LLM 可见可执行）

### 4.3 已移除/废弃组件

以下组件不再存在或已标记废弃：

- **`planner/` 模块**：已删除（原 Planner-Executor 架构）
- **`Executor` 结构体**：`#[deprecated]`，功能迁移至 `ToolRegistry`
- **`ModelRouter`**：已删除，被 `ModelServices` 替代
- **`AgentHarness` / `AgentSkills`** 包装器结构体：已删除，功能由 `Agent` 直接持有
- **`TokenBudget`**：已删除，检索仅用 `ContentLoadStrategy::from_score()`
- **`Chunker`**：已删除，VFS 双层检索替代传统 chunk-based RAG

---

## 5. 完整数据流图

### 5.1 用户对话数据流

```
用户输入 (React Frontend)
         ↓ HTTP POST /api/v1/chat/stream（开关：立即返回；流式增量经 GET /events）
Axum Server → AppState.agent().process_message()
         ↓
SessionState: structured_messages（单一真相源）
         ↓
ContextAssembler::assemble()：soul → rules+memories → history（含当前用户输入）
         ↓
AgentLoop 迭代循环
  ├─ LLM 决策：tool_calls 或 content
  ├─ ToolRegistry.execute_parallel() → 实时持久化 StructuredMessage
  └─ 重复直到 Answer 或 NeedsClarification
         ↓
SSE stream: 7 种 chunk_type 差异化渲染（增量）
  └─ 流结束 message 边界事件：完整 ChatMessage + segments（parts 顺序时间线）
→ React Frontend：历史加载与流式共用 segments 时间线统一渲染（ADR-019）
```

### 5.2 记忆持久化流程

```
Scheduler 定时触发
  ├─ EvolutionTask: EvolutionReviewExecutor（server 实现：主 agent）产出 diff → 记账提交 VFS
  ├─ SummaryTask: SummaryEngine.generate() → VFS 三级摘要 (L0/L1)
  └─ GcTask / SnapshotGcTask / ReminderTask / UsageStatsFlushTask
```

> **任务实现的归属**（ADR-049 期 2 序 5a / 5b）：上面 5 个任务（Summary / GC / SnapshotGC /
> Reminder / UsageStatsFlush）已迁至 `plugins/task-builtin`，`EvolutionTask` 迁至
> `plugins/task-evolution`；两份清单由装配层拼接（`splice(1..1, …)`：演化任务紧跟
> `summary_generation`）后经 `TaskFactory` 插槽注入。core 只留调度机制（`TaskHandler` /
> `TaskContext` / `TaskScheduler`）、清单类型 `TaskRegistration`、能力集合 `TaskServices`
> 与综述执行器契约 `EvolutionReviewExecutor`（实现 `AgentEvolutionExecutor` 在 server，
> 经 `TaskServices.evolution_executor` 注入插件）——**core 已无任何具体任务实现**。

### 5.3 VFS 数据流

```
文档/内容 → KnowledgeIngestor (解析) → SummaryEngine (L0+L1 摘要)
  → VFS write (L0/L1/L2) → EmbeddingService (向量化)
  → LanceDB upsert (abstract_vector + overview_vector + visual_vector)

查询 → EmbeddingService (embed) → VfsSearch::search()
  → LanceDB RRF fusion (abstract + overview)
  → ContentLoadStrategy::from_score() → L0/L1/L2 分层加载
```

---

## 6. 配置结构

配置采用 TOML 文件加载，支持环境变量覆盖：

```
TianyanConfig
├── models      # 模型配置（服务列表）
├── storage     # 存储配置（数据目录、向量数据库）
├── agent       # 智能体配置（learned_rules_top_k: 5）
├── memory      # 记忆配置
├── retrieval   # 检索配置
├── security    # 安全配置
├── logging     # 日志配置
└── features    # 功能开关
```

配置查找顺序（ADR-023）：`~/.tianyan/tianyan.toml` 固定（`TIANYAN_CONFIG` 环境变量可覆盖）；数据目录由配置 `storage.data_dir` 决定（默认 `%LOCALAPPDATA%/tianyan`），配置目录与数据目录分离。

---

**文档版本**: 2026-06-07
**最后更新**: 2026-06-07（同步近期演进：KnowledgeIngestor 泛型消除、知识 API 集成、Server anyhow 迁移完成）
**维护者**: 天演团队
