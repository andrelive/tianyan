# 天演 (Tianyan) 系统架构文档

本文档详细描述天演本地智能代理系统的整体架构、运行流程和各组件间的交互。

> **相关文档导航**：
> - [模块关系图](./module-relationships.md) — 模块间依赖关系、调用流程和数据交互机制
> - [模块功能说明](./module-descriptions.md) — 各模块功能职责、公开 API 和集成状态

## 目录

- [1. 系统概述](#1-系统概述)
- [2. 应用架构](#2-应用架构)
- [3. 系统初始化流程](#3-系统初始化流程)
- [4. 模型路由系统](#4-模型路由系统)
- [5. 双层检索流程 (L0 + L1)](#5-双层检索流程-l0--l1)
- [6. Agent Loop 架构](#6-agent-loop-架构)
- [7. 技能执行系统](#7-技能执行系统)
- [8. 存储架构](#8-存储架构)
- [9. 记忆管理系统](#9-记忆管理系统)
- [10. 完整数据流图](#10-完整数据流图)
- [11. 配置结构](#11-配置结构)

---

## 1. 系统概述

天演 (Tianyan) 是一个基于 Rust 开发的本地 AI 助手桌面应用，采用 **Tauri v2 + Yew + Axum** 技术栈，支持：

- **Agent Loop 架构**：基于原生 Tool Call 的 LLM-in-the-loop 迭代执行，支持并行工具调用和追问中断
- **双层向量检索**：基于摘要和概览向量的快速过滤 + 精确重排序
- **多模型路由**：支持 OpenAI 和 OpenAI Compatible API 的智能路由与故障转移
- **技能系统**：可扩展的工具执行能力（文件操作、HTTP 请求、系统命令等）
- **记忆管理**：对话历史的自动提取、重要性评估和持久化
- **虚拟文件系统 (VFS)**：统一的分层知识库管理（Abstract/Overview/Detail）
- **流式响应**：实时的流式输出，支持思考过程、工具调用、观察结果和最终回答的差异化展示

---

## 2. 应用架构

天演采用 **Tauri v2** 桌面应用架构，结合 **Yew** (Rust WASM 前端) 和 **Axum** (Rust 后端服务器)。

### 2.1 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Tauri Desktop App                        │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Yew Frontend (gui/)                      │  │
│  │  • 聊天界面组件 (chat)                                 │  │
│  │  • 配置向导组件 (config_wizard)                        │  │
│  │  • 技能中心组件 (skills)                               │  │
│  │  • 设置界面 (settings)                                 │  │
│  │  • 侧边栏导航 (sidebar)                                │  │
│  └───────────────────────────────────────────────────────┘  │
│                            ↓ HTTP Calls                     │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Axum Server (server/)                    │  │
│  │  • REST API 端点                                       │  │
│  │  • Server-Sent Events (SSE) 流式传输                  │  │
│  │  • 核心桥接层 (Core Bridge)                            │  │
│  │  • 静态文件服务 (gui/dist)                             │  │
│  └───────────────────────────────────────────────────────┘  │
│                            ↓ Rust Calls                     │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Core Library (core/)                     │  │
│  │  • Agent 协调器 (Agent Loop 模式)                       │  │
│  │  • AgentHarness (失败驱动规则增强)                      │  │
│  │  • AgentSkills (技能执行 + GEPA 进化学习)               │  │
│  │  • ContextPipeline (统一上下文管线)                     │  │
│  │  • 双层检索器                                          │  │
│  │  • 模型路由器                                          │  │
│  │  • 技能执行器 (含技能学习系统)                          │  │
│  │  • 虚拟文件系统 (VFS)                                  │  │
│  │  • 可观测性 (AgentMetrics, Agent 自省)                   │  │
│  │  • 知识管理系统                                        │  │
│  │  • 后台任务调度器                                      │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 项目结构

```
tianyan/
├── core/           # 核心库 (AI Agent, 模型，记忆，检索)
│   ├── agent/      # Agent 协调器 + AgentLoop + ToolRegistry + Harness/Skills + 会话状态
│   ├── config/     # 配置管理（包含向导、验证）
│   ├── common/     # 通用类型（types/ 按领域拆分为 9 个子模块）、错误处理、日志
│   ├── context/    # 上下文工程（检索 + 压缩 + 规则记录/建议）
│   ├── executor/   # 独立执行函数 + Action/ExecutorError + 审批工作流 + 验证门控
│   ├── knowledge/  # 知识库导入和解析
│   │   ├── chunker/  # 文档分块 (mod.rs + types.rs)
│   │   ├── image.rs  # 图像处理
│   │   ├── ingestor/ # 知识导入 (mod.rs + builder.rs)
│   │   ├── parser.rs # 文档解析
│   │   └── types.rs  # 类型定义
│   ├── model/      # 模型服务和路由器
│   │   ├── traits.rs    # 核心服务 trait
│   │   ├── types/       # 类型定义 (9 个子模块)
│   │   ├── openai/      # OpenAI 客户端 (mod|config|client|chat|embedding|vision|stream)
│   │   └── router/      # 模型路由器 (mod.rs + builder.rs + trait_impls.rs)
│   ├── observability/ # 可观测性（AgentMetrics，Agent 自省）
│   ├── planner/    # 已废弃：仅 ClarificationQuestion 类型导出壳
│   ├── scheduler/  # 定时任务调度器
│   ├── session/    # 会话管理
│   ├── skills/     # 技能定义、执行和学习（GEPA 进化引擎）
│   │   ├── definition.rs   # 技能定义和注册表
│   │   ├── executor.rs     # 技能执行器
│   │   ├── handlers/       # 内置技能处理器 (6 个)
│   │   ├── learning/       # GEPA 进化引擎 (mod.rs + types.rs + generator.rs)
│   │   ├── manager.rs      # 技能管理器
│   │   ├── registry.rs     # 技能注册表工厂
│   │   └── types.rs        # 类型定义
│   ├── storage/    # 存储后端、VFS 和记忆提取 trait
│   │   ├── traits.rs       # 核心 trait 定义
│   │   ├── types.rs        # 存储类型定义
│   │   ├── vfs/            # VFS 实现 (mod.rs + builder.rs)
│   │   ├── local/          # 本地存储 (mod.rs + tests.rs)
│   │   ├── qdrant.rs       # Qdrant 向量存储
│   │   ├── summary.rs      # 摘要生成引擎
│   │   ├── summary_service.rs # 摘要生成服务
│   │   ├── uri_mapper.rs   # URI 映射
│   │   └── extractor.rs    # 记忆提取 trait
│   └── tasks/      # 后台任务（摘要、记忆提取）
├── server/         # HTTP API 服务器 (Axum)
│   └── src/
│       ├── api/            # API 路由和处理
│       │   ├── chat/       # 对话领域
│       │   ├── sessions/   # 会话管理领域
│       │   ├── knowledge/  # 知识管理领域
│       │   ├── skills/     # 技能执行领域
│       │   ├── config/     # 配置管理 + 向导
│       │   └── shared/     # 共享类型和错误处理
│       ├── state.rs        # 应用状态管理
│       ├── agent_builder.rs # Agent 构建工厂
│       └── core_bridge.rs  # Core 类型转换桥接
├── gui/            # Yew 前端 (WASM)
│   ├── src/
│   │   ├── api/    # API 客户端（chat, sessions, skills）
│   │   ├── components/  # UI 组件
│   │   │   ├── chat/        # 聊天面板
│   │   │   ├── sidebar/     # 侧边栏
│   │   │   ├── skills/      # 技能中心
│   │   │   ├── settings/    # 设置面板
│   │   │   └── config_wizard/ # 配置向导
│   │   └── state/  # 全局状态管理 (Yew Reducible)
│   └── dist/       # 编译后的前端资源
├── tauri/          # Tauri 桌面应用
│   ├── src/
│   │   ├── lib.rs  # Tauri 应用入口
│   │   └── server.rs  # 服务器启动
│   └── tauri.conf.json  # Tauri 配置
└── docs/           # 项目文档
```

### 2.3 启动流程

```rust
// tauri/src/lib.rs
pub fn run() {
    // 1. 初始化日志系统
    init_logging();
    
    // 2. 加载配置
    let tianyan_config = TianyanConfig::load().unwrap_or_default();
    
    // 3. 启动 Axum 服务器在后台线程
    rt.spawn(async move {
        start_axum_server(config_clone).await;
    });
    
    // 4. 等待服务器就绪（健康检查）
    wait_for_server_ready().await?;
    
    // 5. 启动 Tauri 应用
    tauri::Builder::default()
        .setup(|app| {
            app.get_webview_window("main").unwrap().open_devtools();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Error while running Tauri application");
}
```

### 2.4 API 端点

服务器提供以下完整的 REST API 端点：

#### 配置向导（/api，无版本前缀）

| 端点 | 方法 | 功能 | 前端调用 |
|------|------|------|:---:|
| `/api/config/status` | GET | 配置状态检查 | ✅ |
| `/api/config/wizard` | POST | 保存配置向导数据 | ✅ |
| `/api/config/test-connection` | POST | 测试模型 API 连接 | ⚠️ |

#### 对话领域（/api/v1/chat）

| 端点 | 方法 | 功能 | 前端调用 |
|------|------|------|:---:|
| `/chat` | POST | 非流式聊天 | ⚠️ |
| `/chat/stream` | POST | 流式聊天（SSE） | ✅ |
| `/chat/regenerate` | POST | 重新生成回复 | ✅ |
| `/chat/edit` | POST | 编辑消息后重新生成 | ✅ |

#### 会话管理（/api/v1/sessions）

| 端点 | 方法 | 功能 | 前端调用 |
|------|------|------|:---:|
| `/sessions` | GET | 列出所有会话 | ✅ |
| `/sessions` | POST | 创建新会话 | ⚠️ |
| `/sessions/{id}` | GET | 获取会话详情 | ✅ |
| `/sessions/{id}` | DELETE | 删除会话 | ✅ |
| `/sessions/{id}/messages` | GET | 获取会话消息 | ⚠️ |
| `/sessions/{id}/title` | POST | 更新会话标题 | ⚠️ |

#### 知识管理（/api/v1）

| 端点 | 方法 | 功能 | 前端调用 |
|------|------|------|:---:|
| `/ingest` | POST | 上传文档导入知识库 | ❌ |
| `/ingest/{job_id}/status` | GET | 查询导入任务状态 | ❌ |
| `/search` | GET | 知识库检索 | ❌ |
| `/search/suggestions` | GET | 检索建议 | ❌ |

#### 技能执行（/api/v1/skills）

| 端点 | 方法 | 功能 | 前端调用 |
|------|------|------|:---:|
| `/skills` | GET | 列出可用技能 | ✅ |
| `/skills/{id}/execute` | POST | 执行指定技能 | ✅ |
| `/skills/{id}/jobs/{job_id}/status` | GET | 查询技能执行状态 | ✅ |

#### 运行时配置（/api/v1/config）

| 端点 | 方法 | 功能 | 前端调用 |
|------|------|------|:---:|
| `/config` | GET | 获取完整运行时配置 | ❌ |
| `/config` | PUT | 更新运行时配置 | ❌ |
| `/config/{section}` | GET | 获取配置片段 | ❌ |

#### 健康检查

| 端点 | 方法 | 功能 |
|------|------|------|
| `/health` | GET | 健康检查（Tauri 启动时使用） |

---

## 3. 系统初始化流程

### 3.1 服务器启动流程

当 Tauri 应用启动时，按以下步骤初始化：

```
┌─────────────────────────────────────────────────────────────┐
│  1. 初始化日志系统                                            │
│     • 配置日志级别和格式                                      │
│     • 设置文件日志输出                                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  2. 加载配置 (TianyanConfig)                                  │
│     • 从配置文件加载                                          │
│     • 支持环境变量覆盖                                        │
│     • 提供默认配置作为回退                                    │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  3. 启动 Axum 服务器 (后台线程)                               │
│     • 创建 VFS 单一实例                                       │
│     • 构建 AppState（Agent、SessionManager、TaskScheduler）    │
│     • 创建 API 路由器                                         │
│     • 配置 CORS                                               │
│     • 设置静态文件服务 (gui/dist)                             │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  4. 健康检查                                                  │
│     • 轮询检查 /health 端点                                    │
│     • 超时时间：30 秒                                          │
│     • 检查间隔：500 毫秒                                       │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  5. 启动 Tauri 应用                                           │
│     • 创建主窗口                                              │
│     • 加载 Yew 前端资源                                        │
│     • 启用开发者工具（可选）                                  │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 核心组件初始化

系统采用**应用层统一初始化 VFS**的架构设计，确保整个应用生命周期内只有一个 VFS 实例（单一事实来源）。

#### 3.2.1 架构设计原则

```
┌─────────────────────────────────────────────────────────┐
│  应用启动层 (server/src/lib.rs)                         │
│  - create_app()                                         │
│    - 1. initialize_vfs_for_app() ← 单一实例             │
│    - 2. AppState::new(config, vfs)                      │
│    - 3. 创建 Router                                     │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  应用状态层 (server/src/state.rs - AppState)            │
│  - 持有：Agent, Config, VFS, SessionManager,            │
│    SkillRegistry, SkillExecutor, SummaryService         │
│  - 所有组件共享同一个 VFS 实例                            │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  组件构建层 (agent_builder.rs, core_bridge.rs)          │
│  - AgentBuilderFactory::build_agent_or_wizard()         │
│  - 不再创建 VFS，只接收已创建的 VFS                      │
└─────────────────────────────────────────────────────────┘
```

**核心原则**：
- **单一事实来源**：整个应用只有一个 VFS 实例
- **依赖注入**：VFS 从应用层注入到各组件
- **职责分离**：应用启动层负责创建 VFS，组件构建层只接收 VFS

---

## 4. 模型路由系统

模型路由系统支持多模型服务间的智能路由和故障转移。核心组件：

- **ModelRouter**（位于 `model/router/`）：实现 `ModelService` + `EmbeddingService` + `VlmService` 三个 trait，拆分为 `mod.rs`（核心路由逻辑）、`builder.rs`（构建器）、`trait_impls.rs`（trait 实现）
- **OpenAIClient**（位于 `model/openai/`）：OpenAI 标准 API 客户端，拆分为 `config.rs`、`client.rs`、`chat.rs`、`embedding.rs`、`vision.rs`、`stream.rs`
- **OpenAICompatibleClient**：OpenAI 兼容 API 客户端（支持阿里 DashScope 等）

路由决策流程：
1. 检查显式路由规则（按 TaskType）
2. 尝试模型提示匹配
3. 使用默认服务
4. 找到任何可用服务（故障转移）

---

## 5. 双层检索流程 (L0 + L1)

基于 VFS 三层内容结构的双层向量检索：

- **L0 过滤层**：对 Abstract（~100 token 摘要）向量检索，快速过滤候选
- **L1 重排序层**：对 Overview（~500 token 概览）向量检索，精确重排序
- 使用 **RRF (Reciprocal Rank Fusion)** 算法融合两层结果
- 通过 **ContentLoader** 按三级预算（L0/L1/L2）加载完整内容

---

## 6. Agent Loop 架构

天演采用 **Agent Loop** 模式实现智能体循环。LLM 在循环中自主决定调用工具或直接回答，工具由 `ToolRegistry` 并行执行，执行结果作为 `Message::tool` 回传对话历史，进入下一轮迭代。

### 6.1 核心组件

```
┌─────────────────────────────────────────────────────────────┐
│  Agent（AgentCoordinator 实现）                              │
│    ├── ContextPipeline（统一上下文管线）                      │
│    │     ├── DualLayerRetriever（L0+L1 双层检索）            │
│    │     ├── ContextCompressor（对话压缩 + Token 预算）       │
│    │     └── VFS 集成                                       │
│    ├── AgentHarness（Harness 工程子系统）                    │
│    │     └── AgentMetrics（可观测性指标）                     │
│    ├── AgentSkills（技能子系统）                             │
│    │     ├── SkillExecutor（参数验证 + 安全检查）             │
│    │     ├── SkillRegistry（技能注册表）                     │
│    │     └── SkillLearningEngine（GEPA 进化学习）             │
│    ├── AgentLoop（智能体循环）                               │
│    │     ├── ToolRegistry（工具注册表 + 并行执行）            │
│    │     ├── max_turns 循环控制                              │
│    │     └── ask_user 追问中断                               │
│    ├── VerificationGate（验证门控）                          │
│    └── LlmJudge（LLM-as-Judge 语义验证）                     │
│                                                             │
│         ↓                                                   │
│  ToolRegistry (工具层)                                      │
│    - 维护 ToolDefinition[]（JSON Schema）                    │
│    - 并行执行 tool_calls                                     │
│    - 安全策略 + 审批工作流                                   │
│    - 内置工具：read_file / write_file / execute_command      │
│              / search_code / call_skill / run_tests          │
│              / verify_build / ask_user / delegate_to_agent   │
│                                                             │
│         ↓                                                   │
│  VFS / Skills (底层能力)                                    │
│    - 文件操作、代码搜索、命令执行等                          │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 6.2 Agent Loop 循环流程

```
用户输入
  ↓
AgentCoordinator.process_message(session_id, message)
  ↓
获取或创建 SessionState
  ├── conversation: Vec<Message> (对话历史，唯一真相源)
  ├── context_window: Option<ContextWindow> (上下文窗口)
  └── pending_memories: Vec<MemoryEntry> (待持久化记忆)
  ↓
ContextPipeline.run(message, &mut conversation)
  ├── 检索：DualLayerRetriever → 相关上下文
  ├── 规则注入：扫描 learned rules 并注入 system_prompt
  ├── 压缩：ContextCompressor → Token 预算管理
  └── 输出：ContextWindow { system_prompt, messages, token_usage }
  ↓
AgentLoop.run(messages, stream_sender)
  ├── ChatCompletionRequest::new(model, messages)
  │     .with_tools(ToolRegistry.definitions())
  ├── LLM 决策（看到完整上下文 + 可用工具 Schema）
  ├── 若返回 tool_calls → ToolRegistry.execute_parallel(tool_calls)
  │     ├── 执行工具（文件读写、命令执行、代码搜索等）
  │     └── 将结果作为 Message::tool 追加到 messages
  ├── 若调用 ask_user → 返回 NeedsClarification，中断循环
  └── 若返回 content → 返回 Answer，循环结束
  ↓
后台任务（异步，不阻塞响应）：
  ├── MemoryExtractionService::extract_from_session()
  ├── SkillLearningEngine::learn_from_history()（GEPA 进化）
  └── RuleTask (scheduler): RuleSuggester.scan() → LLM 聚类 → 写 learned rule
  ↓
state.cleanup() (防止无限增长)
```

### 6.3 会话状态管理（SessionState）

**SessionState** 是统一管理会话所有状态的核心容器。重构后 `conversation` 成为唯一真相源，不再维护独立的执行历史。

```rust
pub struct SessionState {
    pub session_id: String,
    pub conversation: Vec<Message>,   // 对话历史（含 user/assistant/tool 消息）
    pub current_goal: Option<String>,
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,
    pub last_activity: Instant,
    pub context_window: Option<ContextWindow>,
    pub pending_memories: Vec<MemoryEntry>,
    pub total_tokens: usize,
    pub start_time: Instant,
}
```

### 6.4 Agent 内部子系统

#### 6.4.1 ContextPipeline（统一上下文管线）

`ContextPipeline` 是 Agent 处理每条消息时的标准化上下文处理流程：

```
ContextPipeline.run(message, conversation)
    │
    ├── Step 1: 规则注入
    │     └── 从 VFS（tianyan://agent/learned/）加载 learned rules
    │         过滤与当前 query 相关的规则，注入到 system_prompt
    │
    ├── Step 2: 检索（DualLayerRetriever）
    │     ├── L0 检索：Abstract 向量 → 快速过滤候选
    │     ├── L1 检索：Overview 向量 → 精确重排序（RRF 融合）
    │     └── ContentLoader：按 TokenBudget 加载内容
    │
    ├── Step 3: 压缩（ContextCompressor）
    │     ├── 评估对话总 token 数
    │     ├── 超限时执行分层压缩策略
    │     └── 保留最近 6 条消息（preserve_recent_messages）
    │
    └── 输出: ContextWindow { system_prompt, messages, token_usage }
```

#### 6.4.2 AgentHarness（Harness 工程子系统）

基于 Harness Engineering 原则实现的失败驱动增强系统：

| 组件 | 职责 | 触发时机 |
|------|------|---------|
| `RuleRecorder` | 去重 + 写入 learned rule（含版本元数据） | Scheduler RuleTask → RuleSuggester → RuleRecorder |
| `RuleSuggester` | 扫描记忆聚类 + LLM 提炼为规则 | RuleTask 定时触发 |
| `AgentMetrics` | 记录 Token 消耗、成功率、规则命中率 | 每次执行 |

**失败驱动增强闭环**：
```
1. 会话 → MemoryTask(定时) → MemoryExtractor(LLM) → 分类记忆写入 VFS (patterns/ + failed_tasks/)
2. RuleTask(定时) → RuleSuggester.scan() → LLM 聚类分析 → RuleRecorder.record_with_kind()
3. RuleRecorder 去重检查 → 写入 VFS agent/learned/
4. 下次对话时 ContextPipeline 加载 learned rules 注入 prompt
5. AgentMetrics 追踪规则命中和注入次数
```

#### 6.4.3 AgentSkills（技能子系统）

| 组件 | 职责 |
|------|------|
| `SkillExecutor` | 参数验证 + 安全检查 + 超时控制 + 执行监控 |
| `SkillRegistry` | 技能注册表，支持运行时动态注册 |
| `SkillLearningEngine` | GEPA 进化引擎：从执行历史中自动提取可复用技能 |

**GEPA 进化引擎流程**：
```
1. 会话结束 → learn_skills_from_session()
2. 提取执行历史 → ExecutionHistory[]
3. 分析成功模式 → 生成 GeneratedSkill{ id, name, description }
4. 自动注册 → SkillRegistry.register()
5. 下次会话 → 新技能立即可用
```

#### 6.4.4 可观测性（AgentMetrics）

`AgentMetrics` 是 Agent 可查询的可观测性存储，支持 Agent 自省：

| 查询接口 | 返回内容 |
|---------|---------|
| `query_token_summary()` | Token 消耗历史和平均值 |
| `query_success_rate()` | 执行总数、成功数、成功率百分比 |
| `query_rule_effectiveness()` | 规则注入总数、命中数、相关性百分比 |
| `query_common_failures()` | Top-10 常见失败步骤及错误信息 |
| `query_harness_health()` | Harness 健康摘要（执行数、成功率、规则命中、Pipeline 失败） |

#### 6.4.5 SessionStateManager

`SessionStateManager` 是一个线程安全的多会话状态管理器，使用 `Arc<RwLock<HashMap<String, SessionState>>>` 实现：

| 方法 | 功能 |
|------|------|
| `with_state(id, f)` | 获取或创建会话状态，执行闭包 |
| `with_state_read(id, f)` | 只读访问会话状态 |
| `remove(id)` | 删除会话状态 |
| `cleanup_expired(secs)` | 清理过期会话（超过指定秒数不活动） |

### 6.5 ToolRegistry 实现

`ToolRegistry` 维护所有可用工具的 JSON Schema 定义，并提供并行执行能力。

- **工具定义**：每个工具通过 `#[derive(JsonSchema)]` 参数结构体自动生成 JSON Schema，注册为 `ToolDefinition`
- **并行执行**：`execute_parallel(tool_calls)` 使用 tokio JoinSet 同时执行多个 tool_call
- **安全策略**：`SecurityPolicy` 控制命令白名单/黑名单，WriteFile 前检查 `check_file_write()`
- **审批工作流**：`ApprovalWorkflow` 五级风险（Safe/Low/Medium/High/Critical），支持自动/手动审批
- **验证门控**：`VerificationGate` 控制是否对执行结果进行自动验证
- **LLM-as-Judge**：`LlmJudge` 对执行结果进行语义验证
- **追问工具**：`ask_user` 工具被特殊处理——AgentLoop 检测到该工具调用时立即返回 `NeedsClarification`，中断循环
- **子 Agent 委托**：`delegate_to_agent` 工具用于创建完全隔离的子 Agent（Agent-as-Tool 模式，待实现）

### 6.6 已废弃组件（保留用于向后兼容）

以下组件已被 Agent Loop 架构取代，保留空壳或 deprecated 标记以维持向后兼容：

- **`planner/` 模块**：原 Planner-Executor 批量规划架构。现为 `ClarificationQuestion` 类型的导出壳
- **`Executor` 结构体**：标记 `#[deprecated]`，功能迁移至 `ToolRegistry`
- **`Action::SubPlanner`**：标记 `#[serde(skip)]`，子任务分解改为 `delegate_to_agent` 工具

---

## 7. 技能执行系统

技能系统提供可扩展的工具执行能力：

- **内置技能**（`skills/handlers/`）：文件操作（file_read、file_write、file_delete、file_list）、HTTP 请求（http_request）、系统命令（system_command），从原 `executor.rs` 分离为独立处理器文件
- **技能注册表**（`skills/registry.rs`）：`create_builtin_skills` 和 `register_builtin_skills` 工厂函数，从原 `executor.rs` 分离
- **技能执行器**（`skills/executor.rs`）：参数验证 + 安全检查 + 执行监控
- **技能学习**（`skills/learning/`）：GEPA 进化引擎，拆分为核心引擎（mod.rs）、类型定义（types.rs）、技能生成（generator.rs）

前端通过技能中心面板展示技能列表，支持参数输入和执行结果展示。

---

## 8. 存储架构

虚拟文件系统（VFS）提供统一的分层内容存储：

```
┌──────────────────────────────────────────────────────┐
│              VirtualFileSystem (trait)                │
│  统一接口：CRUD + 搜索 + 摘要                         │
├──────────────────────────────────────────────────────┤
│                                                      │
│  StorageBackend (trait)     VectorStorage (trait)    │
│  └─ LocalStorageBackend     └─ QdrantVectorStore     │
│                                                      │
│  UriMapper                    EmbeddingService        │
│  └─ URI ↔ 文件路径映射        └─ (ModelRouter 实现)   │
│                                                      │
│  SummaryEngine / SummaryService                       │
│  └─ 分层摘要生成和管理                                │
│                                                      │
└──────────────────────────────────────────────────────┘
```

**内容层级**：
- **L0 - Abstract**：摘要（~100 tokens），用于快速过滤
- **L1 - Overview**：概览（~500 tokens），用于精确重排序  
- **L2 - Detail**：完整内容，用于最终展示

---

## 9. 记忆管理系统

记忆管理系统负责从对话中提取、组织和持久化长期记忆。核心能力通过 `MemoryExtractionTrait`（定义在 `core/src/storage/`）暴露。

- **MemoryExtractionTrait**：记忆提取服务 trait，定义 `extract_and_store` 方法
- **MemoryEntry**：记忆条目（id、content、category、importance、access_count）
- **MemoryCategory**：记忆类别 — Preference / Decision / SuccessfulCase / FailedCase / Pattern / Entity / Fact
- **Agent 集成**：Agent 持有 `Option<Arc<dyn MemoryExtractionTrait>>`，会话结束后异步提取记忆
- **RuleTask / RuleSuggester**：定时扫描记忆聚类，用 LLM 提炼为 learned rules
- **MemoryTask**：定时后台任务，周期性扫描新会话并提取结构化记忆

> 注意：当前 server 层使用 `PlaceholderMemoryCoordinator` 空实现，记忆处理功能尚未完全接入。Agent 中的 `memory_extractor` 参数支持依赖注入。

---

## 10. 完整数据流图

### 10.1 用户对话数据流

```
用户输入 (Yew Frontend)
         ↓ HTTP POST /api/v1/chat/stream (SSE)
Axum Server (chat_stream_handler)
         ↓
AppState.agent().process_message_stream()
         ↓
SessionState (内存中)
         └─ conversation: Vec<Message> (唯一真相源)
         ↓
AgentLoop 迭代循环 (LLM + ToolRegistry)
         ├─ LLM 看到 tools 定义 → 决定 tool_call 或直接回答
         ├─ ToolRegistry.execute_parallel(tool_calls) → 结果回传
         └─ 重复直到 Answer 或 NeedsClarification
         ↓
SSE 流式响应 (含 chunk_type 区分: Thought/ToolCall/Observation/Answer/Error)
         ↓ HTTP Response
Yew Frontend (差异化渲染)
```

### 10.2 消息编辑/重新生成数据流

```
用户点击 "编辑" 或 "重新生成"
         ↓
前端本地更新消息 + 删除后续消息
         ↓ HTTP POST /api/v1/chat/regenerate 或 /chat/edit
后端更新服务端会话状态
         ↓ HTTP POST /api/v1/chat/stream
SSE 流式响应（前端获取新回复）
```

### 10.3 记忆持久化数据流

```
SummaryTask / MemoryTask 定时触发
    │
    ├─ SummaryService::process_all()
    │     └─ VFS 三级摘要生成 (Abstract/Overview)
    │
    └─ MemoryExtractionService::extract_from_session()
          └─ LLM 提取 → VFS 持久化到 tianyan://memory/
```

---

## 11. 配置结构

配置采用分层结构，通过 TOML 文件加载，支持环境变量覆盖：

```
TianyanConfig
├── models      # 模型配置（服务列表、路由规则）
├── storage     # 存储配置（数据目录、向量数据库）
├── agent       # 智能体配置（含 learned_rules_top_k: 5, learned_rules_max_tokens: 800）
├── memory      # 记忆配置
├── retrieval   # 检索配置
├── security    # 安全配置
├── logging     # 日志配置
└── features    # 功能开关
```

配置向导（/api/config/wizard）用于首次安装时的图形化配置。

---

## 关键文件索引

| 文件路径 | 功能描述 |
|---------|---------|
| `core/src/agent/coordinator.rs` | AgentCoordinator 实现，含 ContextPipeline/AgentHarness/AgentSkills 集成 |
| `core/src/agent/harness.rs` | AgentHarness，封装 AgentMetrics |
| `core/src/agent/skill_subsystem.rs` | AgentSkills，封装 SkillExecutor + SkillRegistry + LearningEngine |
| `core/src/agent/types.rs` | AgentResponse、AgentStreamChunk、StreamChunkType、StreamEventSender |
| `core/src/agent/session_state.rs` | SessionState + SessionStateManager，统一会话状态管理 |
| `core/src/agent/loop.rs` | AgentLoop，LLM 工具调用迭代循环 |
| `core/src/agent/tool_registry.rs` | ToolRegistry，工具注册表 + 并行执行 |
| `core/src/agent/tool_params.rs` | 工具参数结构体（#[derive(JsonSchema)]） |
| `core/src/context/pipeline.rs` | ContextPipeline，统一上下文管线（检索 + 规则注入 + 压缩） |
| `core/src/scheduler/tasks/rule_task.rs` | RuleTask，规则提炼的 cron 壳 |
| `core/src/scheduler/tasks/rule_suggester.rs` | RuleSuggester，扫描聚类 + LLM 提炼 |
| `core/src/scheduler/tasks/rule_recorder.rs` | RuleRecorder，去重 + 写入 learned rule |
| `core/src/observability/mod.rs` | AgentMetrics，可观测性存储和 Agent 自省接口 |
| `core/src/planner/mod.rs` | Planner 模块（已废弃，仅导出 ClarificationQuestion） |
| `core/src/executor/executor.rs` | 独立执行函数（execute_read_file 等）+ 废弃的 Executor 壳 |
| `core/src/executor/types.rs` | Action / ExecutorError（Step/StepResult/FailureHandling 已移除） |
| `core/src/model/traits.rs` | ModelService/EmbeddingService/VlmService 核心 trait |
| `core/src/model/router/mod.rs` | ModelRouter 核心路由逻辑 |
| `core/src/model/router/builder.rs` | ModelRouter 构建器 |
| `core/src/model/openai/client.rs` | OpenAIClient 实现 |
| `core/src/storage/vfs/mod.rs` | 虚拟文件系统实现（VirtualFileSystemImpl） |
| `core/src/storage/vfs/builder.rs` | VFS 构建器（VirtualFileSystemBuilder） |
| `core/src/storage/qdrant.rs` | Qdrant 向量存储实现 |
| `core/src/storage/local/mod.rs` | 本地文件系统存储后端 |
| `core/src/skills/executor.rs` | SkillExecutor 技能执行器 |
| `core/src/skills/learning/mod.rs` | SkillLearningEngine，GEPA 进化引擎 |
| `core/src/skills/handlers/mod.rs` | 内置技能处理器入口 |
| `core/src/skills/registry.rs` | 技能注册表工厂 |
| `core/src/knowledge/chunker/mod.rs` | DocumentChunker 文档分块器 |
| `core/src/knowledge/ingestor/mod.rs` | KnowledgeIngestor 知识导入器 |
| `server/src/state.rs` | AppState，应用状态管理 |
| `server/src/api/chat/handlers.rs` | 对话 HTTP 处理器（非流式 + 流式 SSE） |
| `server/src/api/chat/services.rs` | 对话业务逻辑（含 chunk_type 传递） |
| `server/src/agent_builder.rs` | Agent 构建工厂 |
| `server/src/core_bridge.rs` | Core-Server 类型转换 |
| `gui/src/components/chat/mod.rs` | 聊天面板组件（含 regenerate/edit 流程） |
| `gui/src/state/mod.rs` | 前端全局状态管理（Yew Reducible） |
| `tauri/src/lib.rs` | Tauri 桌面应用入口 |

---

## 架构演进历史

### 2026-05：全模块目录拆分重构

- **model 模块目录化**：`types.rs`（783 行）→ `model/types/`（9 子模块：chat, embedding, vision, model_info, config, service, streaming, api_error, anthropic）；`openai.rs`（860 行）→ `model/openai/`（7 文件：mod, config, client, chat, embedding, vision, stream）；`router.rs`（820 行）→ `model/router/`（3 文件：mod, builder, trait_impls）
- **skills 模块目录化**：`executor.rs`（964 行）→ `skills/handlers/`（6 处理器）+ `skills/registry.rs`（注册表工厂）；`learning.rs`（733 行）→ `skills/learning/`（3 文件：mod, types, generator）
- **storage 模块目录化**：`vfs.rs`（1417 行）→ `storage/vfs/`（mod + builder）；`local.rs`（564 行）→ `storage/local/`（mod + tests）
- **knowledge 模块目录化**：`chunker.rs`（579 行）→ `knowledge/chunker/`（mod + types）；`ingestor.rs`（683 行）→ `knowledge/ingestor/`（mod + builder）
- **错误类型合并**：`executor/error.rs` 合并到 `executor/types.rs`，`planner/error.rs` 合并到 `planner/types.rs`，消除分散的错误类型定义
- **保持向后兼容**：所有模块通过 `mod.rs` 重新导出公共 API，对外接口（trait 定义、公开类型、函数签名）完全不变
- **测试无影响**：全部 317 个单元测试、5 个集成测试、13 个文档测试均通过

### 2026-05：Harness 工程子系统与可观测性

- **AgentHarness 子系统**：封装 AgentMetrics（可观测性指标）。规则记录通过 Scheduler RuleTask 定时运行，不再内嵌于 Agent。
- **AgentSkills 子系统**：封装 SkillExecutor、SkillRegistry、SkillLearningEngine（GEPA 进化引擎），统一技能生命周期管理
- **ContextPipeline**：引入统一上下文管线，标准化"规则注入 → 检索 → 压缩"的处理流程
- **可观测性（observability 模块）**：新增 AgentMetrics 存储，支持 Agent 自省（Token 消耗、成功率、规则有效性、Harness 健康摘要）
- **验证门控与 LLM-as-Judge**：新增 VerificationGate 和 LlmJudge，支持执行结果的自动验证和语义判断
- **会话状态扩展**：SessionState 新增 context_window、pending_memories、total_tokens、start_time 字段
- **记忆提取 trait**：MemoryExtractionTrait 迁移至 storage 模块，Agent 通过依赖注入接收实现

### 2026-04：前后端接口对齐与功能闭环

- **chunk_type 透传**：后端 `ChatStreamEvent` 新增 `chunk_type` 字段，从 `AgentStreamChunk` 传递到前端，实现思考/工具调用/观察/回答的差异化渲染
- **regenerate/edit 闭环**：前端消息编辑和重新生成改为先调用后端 `/chat/regenerate` 和 `/chat/edit` 端点，再流式获取新回复，解决服务端数据不一致问题
- **会话标题更新**：新增 `POST /sessions/{id}/title` 端点，前后端均已就绪
- **错误格式统一**：前端 `ApiError` 适配后端 `ErrorResponse { error: String }` 格式
- **类型字段补齐**：前端补齐 `Session.metadata`、`ListSessionsResponse.total`、`CreateSessionRequest.initial_message` 等缺失字段

### 2026-04：Planner-ModelRouter 集成重构

- **移除冗余抽象**：移除 `LlmService` trait 和 `DefaultLlmService`
- **统一 LLM 调用**：Planner 改用 `ModelService` trait（已有完整实现）
- **享受路由能力**：Planner 现在可以享受 ModelRouter 的故障转移、多模型路由
- **Prompt 模块迁移**：从 `planner/llm.rs` 移动到 `agent/prompt.rs`

### 2026-05：Agent Loop 架构重构

- **移除 Planner-Executor 批量规划架构**：删除 `Plan`、`Step`、`Turn`、`PlannerContext`、`PlannerMutation`、`PlannerError`、`PlannerOutput`、`PlannerTrait`、`ExecutorTrait`
- **引入 AgentLoop**：LLM 在循环中自主决定调用工具或直接回答，工具结果回传为 `Message::tool`，进入下一轮迭代
- **ToolRegistry 替代 Executor**：维护 `ToolDefinition[]`（JSON Schema），并行执行 `tool_calls`，内置 9 个工具
- **ToolParams 强类型参数**：每个工具定义 `#[derive(JsonSchema)]` 参数结构体，自动生成 JSON Schema
- **ask_user 工具替代 Clarification Plan**：LLM 调用 `ask_user` 工具时 AgentLoop 返回 `NeedsClarification`，中断循环
- **SessionState 简化**：移除 `execution_context: ContextManager`，`conversation` 成为唯一真相源
- **ClarificationQuestion 迁移**：从 `planner/types.rs` 迁移至 `agent/types.rs`
- **SubPlanner → delegate_to_agent**：子任务分解改为 Agent-as-Tool 模式（`delegate_to_agent` 工具，待实现）
- **向后兼容**：`planner/` 模块保留为 `ClarificationQuestion` 导出壳，`Executor` 标记 `#[deprecated]`

### 2026-03：Planner-Executor 架构

- **移除 ReAct 架构**，采用更纯净的 Planner-Executor 模式
- **引入 SessionState**，统一管理对话历史和执行历史
- **Planner 无状态化**，状态由 SessionState 管理
- **Executor 纯机械执行**，并行执行步骤，支持失败处理策略

### 2025-12：VFS 统一初始化

- **应用层统一初始化 VFS**，确保单一实例
- **依赖注入**，VFS 从应用层传递到各组件

### 2025-10：双层检索系统

- **L0 过滤层**：基于摘要向量快速过滤
- **L1 重排序层**：基于完整内容精确重排序

---

**文档版本**: 2026-05-30
**最后更新**: 2026-05-30（规则管线重构：RuleRecorder/RuleSuggester 移至 scheduler/tasks/，新增 RuleTask，AgentHarness 精简）
**维护者**: 天演团队
