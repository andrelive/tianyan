# 天演 (Tianyan) 系统架构文档

本文档描述天演本地智能代理系统的整体架构、核心决策和组件间交互。

> **相关文档导航**：
> - [模块关系图](./module-relationships.md) — 模块间依赖关系、调用流程和数据交互机制
> - [模块功能说明](./module-descriptions.md) — 各模块功能职责、公开 API 和集成状态

## 目录

- [1. 系统概述](#1-系统概述)
- [2. 应用架构](#2-应用架构)
- [3. 核心架构决策](#3-核心架构决策)
- [4. Agent Loop 架构](#4-agent-loop-架构)
- [5. 完整数据流图](#5-完整数据流图)
- [6. 配置结构](#6-配置结构)

---

## 1. 系统概述

天演 (Tianyan) 是一个基于 Rust 开发的本地 AI 助手桌面应用，采用 **Tauri v2 + Yew + Axum** 技术栈。

**核心能力**：

| 能力 | 说明 |
|------|------|
| **Agent Loop 架构** | LLM 在循环中自主调用工具或直接回答，支持并行工具调用和追问中断 |
| **VFS 双层摘要索引** | 三层内容（L0 Abstract / L1 Overview / L2 Detail）+ 双向量 RRF 融合检索 |
| **StructuredMessage** | 单一真相源：持久化、会话组装、压缩跟踪、Token 统计 |
| **组件工具化** | 13 个 OpenAI function calling 兼容工具，`call_skill` 桥接到技能系统 |
| **前缀匹配缓存** | soul+rules+memories 固定前缀 → history 可变后缀，利用 LLM Provider 缓存 |
| **技能系统** | 6 个内置技能 + GEPA 进化引擎自动学习 |
| **流式响应** | SSE 流式输出，6 种 chunk_type 差异化渲染 |

---

## 2. 应用架构

### 2.1 四层架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Tauri Desktop App                        │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Yew Frontend (gui/)                      │  │
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
│  │  Agent | ContextPipeline | VFS | Skills | Scheduler   │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 项目结构

```
tianyan/
├── core/           # 核心库 (lib: tianyan)
│   ├── agent/      # Agent 协调器 + AgentLoop + ToolRegistry + 会话状态
│   ├── common/     # 通用类型、错误处理、日志
│   ├── config/     # 配置管理
│   ├── context/    # 上下文工程（assembler、pipeline、retrieval/、compression/）
│   ├── executor/   # [已废弃] 独立执行函数 + 审批工作流 + 验证门控
│   ├── knowledge/  # 知识库导入和解析（parser、image、ingestor/）
│   ├── memory/     # 记忆提取（MemoryExtractor）
│   ├── model/      # 模型服务容器 + provider/
│   │   ├── traits.rs    # ChatService/EmbeddingService/VlmService trait
│   │   ├── services.rs  # ModelServices（替代 ModelRouter 的容器）
│   │   └── provider/    # AsyncOpenAIClient 等统一客户端
│   ├── observability/ # AgentMetrics 可观测性存储和自省接口
│   ├── scheduler/  # 定时任务调度器 + tasks/（RuleTask、MemoryTask 等）
│   ├── session/    # 会话管理（PersistentSessionManager）
│   ├── skills/     # 技能系统（definition、executor、handlers/、learning/）
│   └── vfs/        # 虚拟文件系统（traits、vfs_impl、backend/、vector/、summary/）
├── server/         # Axum HTTP 服务
│   └── src/
│       ├── api/            # 领域 API（chat/sessions/knowledge/skills/config）
│       ├── state.rs        # AppState（持有 Agent、VFS、SessionManager）
│       ├── agent_builder.rs # Agent 构建工厂
│       └── lib.rs          # bootstrap_app_vfs() + 服务器启动
├── gui/            # Yew WASM 前端
│   └── src/
│       ├── api/            # API 客户端
│       ├── components/     # UI 组件（chat/sidebar/skills/settings/config_wizard）
│       └── state/          # Yew Reducible 全局状态
├── tauri/          # Tauri 桌面包装
│   └── src/
│       ├── lib.rs          # 入口：日志→配置→启动服务器→健康检查→Tauri 窗口
│       └── server.rs       # start_axum_server()
└── docs/           # 项目文档
```

### 2.3 API 端点

服务器提供以下 REST API 端点（前端调用状态：✅ 已接入 / ⚠️ 可优化 / ❌ 未接入）：

| 领域 | 端点 | 方法 | 前端 |
|------|------|------|:---:|
| 对话 | `/api/v1/chat/stream` | POST (SSE) | ✅ |
| 对话 | `/api/v1/chat` | POST | ⚠️ |
| 对话 | `/api/v1/chat/regenerate` | POST | ✅ |
| 对话 | `/api/v1/chat/edit` | POST | ✅ |
| 会话 | `/api/v1/sessions` | GET/POST | ✅/⚠️ |
| 会话 | `/api/v1/sessions/{id}` | GET/DELETE | ✅ |
| 会话 | `/api/v1/sessions/{id}/title` | POST | ⚠️ |
| 技能 | `/api/v1/skills` | GET | ✅ |
| 技能 | `/api/v1/skills/{id}/execute` | POST | ✅ |
| 知识 | `/api/v1/ingest` | POST | ❌ |
| 知识 | `/api/v1/search` | GET | ❌ |
| 配置 | `/api/config/status` | GET | ✅ |
| 配置 | `/api/config/wizard` | POST | ✅ |
| 健康 | `/health` | GET | 内部 |

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

**检索流程**：查询文本 → embed → Qdrant RRF 融合搜索 `abstract_vector` + `overview_vector` → `ContentLoadStrategy::from_score()` 按分数分层加载（>0.85→L2, >0.6→L1, 其他→L0）。

**此机制是项目底层基础**，其他设计必须妥协于它：

- **知识库不做 chunk**：完整文档直接写入 VFS L2 Detail，`SummaryEngine` 生成 L0/L1，双层检索替代传统 chunk-based RAG。`Chunker` 已移除。（`core/src/knowledge/ingestor/`）
- **技能渐进式披露**：技能存于 `tianyan://skill/` 命名空间。L0 Abstract 快速发现，L2 Detail（`skill.md`）按需加载。`description_embedding` 向量支持语义检索。
- **图像双通道**：VLM 生成文本描述 → 文本 embedding 用于 L0/L1，同时 `embed_image()` 生成 `visual_vector` 用于视觉相似度搜索。
- 所有命名空间（User、Session、Memory、Knowledge、Agent、Skill）共享同一套机制。

关键文件：`core/src/vfs/traits.rs`（`VfsSearch` trait）、`core/src/vfs/vfs_impl.rs`（`search()`）、`core/src/vfs/vector/qdrant.rs`（RRF 融合）、`core/src/context/retrieval/retriever.rs`（`DualLayerRetriever`）。

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

1. **持久化**：JSONL 格式写入 VFS。`AgentLoop` 每产生一条消息，实时调 `SessionManager::add_structured_message()` 落盘。
2. **会话组装**：存储与传输分离 — `StructuredMessage`（存储层）↔ `Message`（传输层）。`ContextAssembler::assemble()` 转换。
3. **会话跟踪**：`compression_marker` 标记压缩产生的摘要消息。加载会话时反向扫描到最近 marker，只加载 marker 及之后的的消息（旧消息保留在磁盘）。
4. **Token 统计**：LLM 响应 `TokenUsage` → `AgentLoop` 捕获 → `DetailedTokenUsage` → 持久化 → 聚合到 `AgentState.total_tokens`。

### 3.3 决策 3: 组件工具化（待实现）

知识库查询、技能调用等能力封装为 OpenAI function calling 兼容的工具，由 LLM 通过 `tool_call` 自主调用。

当前 `ToolRegistry` 注册了 9 个工具：`read_file`、`write_file`、`execute_command`、`search_code`、`call_skill`、`run_tests`、`verify_build`、`ask_user`、`delegate_to_agent`。其中 `call_skill` 桥接到 `SkillExecutor`（参数验证 + 安全检查 + 超时控制）。

### 3.4 决策 4: 上下文组装前缀匹配原则

`ContextAssembler::assemble()` 严格遵循**固定前缀 + 可变后缀**顺序：

```
soul → rules+memories → history(from compression_marker) → current input
```

- **固定前缀（soul + rules + memories）**：会话期间不变，LLM Provider 可利用前缀缓存只计算一次。
- **可变后缀（history）**：从最近 `compression_marker` 开始拼接，marker 前的旧消息不纳入本次输入。
- **绝对禁止**将 soul/rules/memories 放在 history 之后——会破坏前缀缓存。

实现位置：`core/src/context/assembler.rs` 的 `assemble()` 方法，顺序不可变更。

---

## 4. Agent Loop 架构

### 4.1 Agent 组件结构

```
Agent :: process_message(session_id, msg)
  │
  ├─ SessionManager::get_session(session_id) → SessionState
  │     └─ load_session_from_vfs()：解析 JSONL → compression_marker 截断
  │
  ├─ ContextPipeline::prepare_context()
  │     ├─ ContextAssembler::assemble()：soul → rules+memories → history → input
  │     ├─ DualLayerRetriever::retrieve()：L0+L1 RRF 融合检索
  │     └─ compress_if_needed()：compression_marker 后 > 6 条 → LLM 摘要压缩
  │
  ├─ AgentLoop::run(messages, session_id, parent_id)
  │     ├─ ChatCompletionRequest::new(model, messages).with_tools(ToolRegistry.defs())
  │     ├─ LLM 返回 tool_calls → ToolRegistry::execute_parallel()
  │     │     └─ 每条消息实时 persist（含工具调用结果，不丢弃）
  │     ├─ 调用 ask_user → NeedsClarification，中断循环
  │     └─ 返回 content → Answer，循环结束
  │
  └─ 后台异步（不阻塞响应）：
        ├─ MemoryExtractor::extract_and_store()
        ├─ SkillLearningEngine::learn_from_history()
        └─ Scheduler: RuleTask → RuleSuggester → RuleRecorder
```

### 4.2 ToolRegistry 实现

`ToolRegistry` 维护所有可用工具的 JSON Schema 定义，`execute_parallel()` 通过 tokio JoinSet 并行执行：

- **9 个内置工具**：基于 `#[derive(JsonSchema)]` 参数结构体自动生成 Schema
- **安全策略**：`SecurityPolicy` 控制命令白名单/黑名单，文件操作前检查
- **审批工作流**：`ApprovalWorkflow` 五级风险（Safe/Low/Medium/High/Critical）
- **验证门控**：`VerificationGate` 控制执行后自动验证（cargo check/test）
- **LLM-as-Judge**：`LlmJudge` 对执行结果进行语义验证

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
用户输入 (Yew Frontend)
         ↓ HTTP POST /api/v1/chat/stream (SSE)
Axum Server → AppState.agent().process_message()
         ↓
SessionState: structured_messages（单一真相源）
         ↓
ContextAssembler::assemble()：soul → rules+memories → history → input
         ↓
AgentLoop 迭代循环
  ├─ LLM 决策：tool_calls 或 content
  ├─ ToolRegistry.execute_parallel() → 实时持久化 StructuredMessage
  └─ 重复直到 Answer 或 NeedsClarification
         ↓
SSE stream: 6 种 chunk_type 差异化渲染
→ Yew Frontend 展示
```

### 5.2 记忆持久化流程

```
Scheduler 定时触发
  ├─ MemoryTask: MemoryExtractor.extract() → LLM 提取 → VFS (tianyan://memory/)
  ├─ SummaryTask: SummaryEngine.generate() → VFS 三级摘要 (L0/L1)
  └─ RuleTask: RuleSuggester.scan() → RuleRecorder.record() → VFS (agent/learned/)
```

### 5.3 VFS 数据流

```
文档/内容 → KnowledgeIngestor (解析) → SummaryEngine (L0+L1 摘要)
  → VFS write (L0/L1/L2) → EmbeddingService (向量化)
  → Qdrant upsert (abstract_vector + overview_vector + visual_vector)

查询 → EmbeddingService (embed) → VfsSearch::search()
  → Qdrant RRF fusion (abstract + overview)
  → ContentLoadStrategy::from_score() → L0/L1/L2 分层加载
```

---

## 6. 配置结构

配置采用 TOML 文件加载，支持环境变量覆盖：

```
TianyanConfig
├── models      # 模型配置（服务列表）
├── storage     # 存储配置（数据目录、向量数据库）
├── agent       # 智能体配置（learned_rules_top_k: 5, learned_rules_max_tokens: 800）
├── memory      # 记忆配置
├── retrieval   # 检索配置
├── security    # 安全配置
├── logging     # 日志配置
└── features    # 功能开关
```

配置查找顺序：`./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`。

---

**文档版本**: 2026-06-07
**最后更新**: 2026-06-07（同步近期演进：KnowledgeIngestor 泛型消除、知识 API 集成、Server anyhow 迁移完成）
**维护者**: 天演团队
