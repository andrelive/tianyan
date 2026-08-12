# 天演模块功能说明

> 本文档描述天演项目各模块的功能职责、公开 API 和关键实现。
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

Core 是天演的核心库，提供 AI Agent 的全部基础能力。4 crate workspace（resolver = "2"），无外部项目依赖。

| 子模块 | 职责 | 关键文件 | 集成状态 |
|--------|------|---------|---------|
| `agent` | Agent 协调器 + AgentLoop 迭代循环 + 会话状态 | coordinator.rs, builder.rs, session_state.rs, loop.rs, tool_registry/, tools.rs, types.rs | ✅ 已集成 |
| `common` | 通用类型（按领域拆分）、错误处理、日志配置、token 估算 | error.rs, logging.rs, token_estimator.rs, types/ | ✅ 已集成 |
| `config` | 配置管理（TOML + 环境变量 + 向导） | mod.rs, wizard.rs, validation.rs, agent.rs, model.rs | ✅ 已集成 |
| `context` | 上下文工程（检索 + 压缩 + 管线 + 组装） | pipeline.rs, retrieval/, compression/, assembler.rs | ✅ 已集成 |
| `eval` | 回答质量评测（LLM-as-Judge 评分式：四维度 1-10 分 + 黄金用例批处理；离线基准用，不接入在线链路） | judge.rs, runner.rs, golden.rs | ✅ 已实现（离线工具） |
| `executor` | 工具执行支撑（Action、审批工作流、LLM-as-Judge、验证门控）+ 编程助手执行原语（hashline 编辑、patch、文件浏览、搜索、符号、测试发现） | actions.rs, security.rs, command.rs, output_parse.rs, approval/, types.rs, judge.rs, verification.rs, hashline.rs, truncate.rs, edit.rs, patch.rs, fs.rs, search.rs, symbols.rs, project.rs, test_discovery.rs | ✅ 正常使用 |
| `lsp` | LSP 客户端（服务器注册表 + 自研 JSON-RPC 传输 + 诊断存储） | registry.rs, client.rs, diagnostics.rs | ✅ 已集成 |
| `snapshot` | 工作区快照（回退/撤销回退；gzip 压缩 + GC + similar diff） | mod.rs | ✅ 正常使用 |
| `knowledge` | 知识库管理（解析、图像、导入） | parser.rs, image/, types.rs, ingestor/ | ✅ 已集成（Server 层通过 KnowledgeIngestor 真实处理导入与检索） |
| `memory` | 长期记忆提取 | extractor.rs | ✅ 已集成 |
| `model` | 模型服务（provider 实现 + 服务容器） | traits.rs, types/, provider/, services.rs | ✅ 已集成 |
| `observability` | 可观测性存储（AgentMetrics，Agent 自省）+ 使用统计 | mod.rs, usage_stats.rs（SQLite 连接复用 `vfs/backend/sqlite_db.rs`） | ✅ 已集成 |
| `scheduler` | 定时任务调度器 + 任务实现 | task_scheduler.rs, tasks/ | ✅ 已集成 |
| `session` | 会话管理（创建、持久化、消息记录）；截断常量单点 | manager.rs, types.rs（常量在 mod.rs） | ✅ 已集成 |
| `skills` | 技能定义、执行和学习（GEPA 进化引擎） | definition.rs, executor.rs, manager.rs, handlers/, registry.rs, learning/ | ✅ 已集成 |
| `vfs` | 虚拟文件系统、存储后端、向量存储、摘要引擎 | traits.rs, types.rs, vfs_impl.rs, vfs_builder.rs, backend/（含 sqlite_db.rs）, vector/, summary/, uri_mapper.rs | ✅ 已集成 |

### 1.2 agent 子模块

**职责**：Agent Loop 架构的对外接口层，管理会话状态、执行 AgentLoop 迭代循环、处理追问，支持流式与非流式两种对话模式。内部封装 ContextPipeline、AgentLoop 和 ToolRegistry 三个子系统。

**核心类型**：

| 类型 | 说明 |
|------|------|
| `Agent` | `AgentCoordinator` 的默认实现，持有 default_model、ContextPipeline、AgentMetrics、AgentLoop、SessionManager、SnapshotManager（可选）等组件 |
| `AgentCoordinator` (trait) | Agent 协调器接口，定义 `process_message`、`process_message_stream`、`handle_clarification`、`initialize`、`shutdown`；消息参数为 `&Message`（支持多模态图片片段，ADR-010） |
| `AgentBuilder` | 构建器模式创建 Agent（构造 AgentLoop + ToolRegistry；`Agent::new` 7 参数） |
| `AgentLoop` | Agent 迭代循环（LLM 工具调用循环） |
| `AgentLoopConfig` | AgentLoop 配置（loop_limit 默认 50） |
| `ToolRegistry` | 工具注册表，维护 ToolDefinition[] 并并行执行 tool_calls（JoinSet，同轮多调用并发）；注册 25 个工具：read_file、write_file、execute_command、search_code、search_knowledge、vfs_read、vfs_list、call_skill、run_tests、verify_build、ask_user、self_check、knowledge_ingest、delegate_to_agent、web_search、web_fetch、task_status、task_cancel、apply_edit、apply_patch、glob、list_dir、discover_tests、symbol_outline、lsp；`delegate_to_agent` 支持嵌套委托（深度上限 3，RAII guard 计数）、`max_turns`/`timeout_secs` 参数与**后台执行**（`background: true` → 任务注册表 + 完成通知注入父会话） |
| `SessionState` | 会话状态容器（对话历史为唯一真相源，上下文窗口、待持久化记忆） |
| `SessionStateManager` | 多会话状态管理器（线程安全，Arc<RwLock<HashMap>>） |
| `AgentResponse` | Agent 响应（内容、追问、Token 使用量、技能调用信息、处理时间） |
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

**核心架构决策集成**：

1. **StructuredMessage 持久化**：AgentLoop 每产生一条消息，实时调 `SessionManager::add_structured_message()` 落盘，工具调用消息全部持久化。
2. **压缩锚点**：`StructuredMessage.compression_marker` 标记压缩产生的摘要消息，加载会话时反向扫描到最近 marker。
3. **组件工具化**：`ToolRegistry` 注册 21 个 OpenAI function calling 兼容工具，`call_skill` 桥接到 `SkillExecutor`。

### 1.3 model 子模块

**职责**：提供统一的模型服务接口，`ModelServices` 是 `Arc<dyn ChatService/EmbeddingService/VlmService>` 的容器，不路由、不重试。

**模块组织**：
- `model/traits.rs` — 核心服务 trait（ChatService, EmbeddingService, VlmService）
- `model/types/` — 请求、响应和配置的类型定义
- `model/provider/` — OpenAI 兼容 API 客户端实现，`pub(crate)` 访问，外部通过 `ModelServices` 使用
- `model/services.rs` — `ModelServices`，从配置构建一组已包装（日志）服务的容器，替代旧 `ModelRouter`

**核心类型**：

| 类型 | 说明 |
|------|------|
| `ChatService` (trait) | 聊天补全服务接口（1 生产 + 5 测试 mock → 真实 seam） |
| `EmbeddingService` (trait) | 文本嵌入服务接口（1 生产 + 1 测试 mock → 真实 seam） |
| `VlmService` (trait) | 视觉语言模型服务接口（1 生产 + 0 测试 mock → 假设性 seam） |
| `ModelServices` | 服务容器，打包 chat/embedding/vision 三种服务 |
| `AsyncOpenAIClient` | OpenAI 兼容 API 客户端 |
| `ChatCompletionRequest/Response` | 聊天补全请求和响应类型 |
| `EmbeddingRequest/Response` | 嵌入请求和响应类型 |
| `SharedChatService` | `Arc<dyn ChatService>` 类型别名 |

**多模态序列化**（ADR-010）：`model/provider/chat.rs::user_content` 在用户消息携带 `content_parts`（图片）时输出 OpenAI 数组格式（text + image_url content parts，detail 映射 low/high/auto）；无图片保持纯文本（行为零变化）。`ContentPart`/`ImageUrl` 类型已上移 `common/types/content_part.rs`（`model::types` re-export 兼容）。

### 1.4 vfs 子模块

**职责**：提供统一的虚拟文件系统（VFS），所有上下文（知识库、记忆、技能、规则）的统一存储与检索层。**这是天演项目的基础机制，其他设计必须妥协于它。**

**模块组织**：
- `vfs/traits.rs` — 核心 trait 定义（VfsCore, ContentStore, VfsSearch）
- `vfs/types.rs` — 存储相关类型定义
- `vfs/vfs_impl.rs` — `VirtualFileSystemImpl` 实现 + `initialize()` 基础设施初始化
- `vfs/vfs_builder.rs` — `VirtualFileSystemBuilder` 构建器
- `vfs/backend/sqlite.rs` — SQLite 存储后端（替代已删除的 `LocalFileBackend`）
- `vfs/backend/sqlite_db.rs` — 共享 SQLite 连接 `SqliteDb`（与 `observability/usage_stats.rs` 共用，ADR-005 单连接语义；自 `observability/` 下沉，ADR-007）
- `vfs/vector/lancedb/` — LanceDB 嵌入式向量数据库实现（`batch.rs` / `mod.rs` / `tests.rs`，RRF 融合搜索）
- `vfs/summary/engine.rs` — `SummaryEngine` 分层摘要生成（`new(model_service, model_name)` 2 参数，依赖 `model::ChatService`）
- `vfs/uri_mapper.rs` — URI 到文件系统路径映射

**核心类型**：

| 类型 | 说明 |
|------|------|
| `VfsCore` (trait) | VFS 核心操作（CRUD、元数据） |
| `ContentStore` (trait) | 分层内容读写（L0 Abstract / L1 Overview / L2 Detail） |
| `VfsSearch` (trait) | 向量检索：查询 embed → LanceDB RRF 融合 abstract_vector + overview_vector |
| `VirtualFileSystemImpl` | VFS 默认实现 |
| `VirtualFileSystemBuilder` | VFS 构建器 |
| `SqliteBackend` | SQLite 存储后端（具体类型，替代已删除的 `LocalFileBackend`） |
| `LanceDbVectorStore` | LanceDB 嵌入式向量数据库实现（支持多点向量、RRF 融合搜索） |
| `SummaryEngine` | 分层摘要生成引擎（L0: ~100 tokens, L1: ~2K tokens） |
| `UriMapper` | URI 到文件系统路径映射 |

**核心架构决策——VFS 双层摘要索引**：

采用**三层内容 + 双向量 RRF 融合**：

| 层级 | 名称 | Token | 向量 | 用途 |
|------|------|-------|------|------|
| L0 | Abstract | ~100 | `abstract_vector` | 向量搜索、快速过滤 |
| L1 | Overview | ~2K | `overview_vector` | 内容导航、重排序 |
| L2 | Detail | 无限制 | — | 完整内容，按需加载 |

`SummaryEngine` 通过 LLM 为每条 VFS 条目生成 L0/L1 摘要。检索时：查询文本 → embed → LanceDB RRF 融合搜索 `abstract_vector` + `overview_vector` → `ContentLoadStrategy::from_score()` 按分数分层加载（大于0.85→L2, 大于0.6→L1, 其他→L0）。

**对子系统的约束**：
- **知识库不做 chunk**：文档完整解析后直接写入 VFS L2 Detail，SummaryEngine 生成 L0/L1 摘要，双层检索自然替代 chunk-based RAG。`Chunker` 已移除。
- **技能渐进式披露**：技能存于 `tianyan://skill/` 命名空间。L0 Abstract 用于快速发现（`SkillManager::list_available_skills()` 读取所有技能的 abstract），L2 Detail 按需加载完整定义。
- **图像双通道**：VLM 生成文本描述 → 文本 embedding 用于 L0/L1 检索，同时 `embed_image()` 生成 `visual_vector` 用于视觉相似度搜索。

### 1.5 executor 子模块

**职责**：独立的工具执行函数（`execute_read_file`、`execute_write_file`、`execute_search_code`、`execute_command_action`、`execute_run_tests`、`execute_verify_build`）+ 编程助手执行原语（apply_edit / apply_patch / glob / list_dir / discover_tests / symbol_outline / lsp），供 `ToolRegistry` 调用。`Action`、`ExecutorError`、`ApprovalWorkflow`、`LlmJudge`、`VerificationGate` 类型被 `agent/build.rs` 和 `agent/tool_registry.rs` 使用。

> `Executor` trait 壳、`Step`、`StepResult`、`FailureHandling`、`ExecutorTrait` 等旧 Planner-Executor 架构类型已移除。

**模块组织**：
- `executor/actions.rs` — 公开执行函数（`execute_read_file`、`execute_write_file`、`execute_search_code`、`execute_run_tests`、`execute_verify_build`）+ 重导出 `execute_command_action` / `SecurityPolicy`
- `executor/security.rs` — `SecurityPolicy` 安全策略（命令白名单/黑名单、目录限制、command_timeout）+ 路径沙箱统一判定（`check_path_rules` / `normalize_path_for_check`，skills 侧共享）
- `executor/command.rs` — 命令执行（`execute_command_action`、`DEFAULT_COMMAND_TIMEOUT_SECS`）
- `executor/output_parse.rs` — 命令输出解析统一实现（`extract_build_errors` / `count_test_passed` / `extract_test_failures`：大小写不敏感匹配、单行 200 字符截断、50 条上限）
- `executor/types.rs` — `Action` 枚举（ReadFile / WriteFile / ExecuteCommand / SearchCode / ApplyEdit / ApplyPatch）和 `ExecutorError`
- `executor/approval/` — 审批工作流（`types.rs` 领域类型 + `workflow.rs` 状态机）
- `executor/verification.rs` — 验证门控 + 结构化诊断（`StructuredDiagnostic` / `parse_json_diagnostics`，解析 `cargo check --message-format=json-render-diagnostics`）
- `executor/judge.rs` — LLM-as-Judge 语义验证
- `executor/hashline.rs` — hashline 锚点（`line_hash` 空白不敏感 FNV-1a；`format_line` 生成 `N#ID|content`；`parse_anchor` / `hash_line_pair`）
- `executor/truncate.rs` — 统一截断层（`truncate_head` / `truncate_tail` / `truncate_spill`，`MAX_LINES = 2000` / `MAX_BYTES = 50KB`，UTF-8 安全）
- `executor/edit.rs` — 语义化编辑（`EditSpec` / `apply_edits_to_content` 纯函数 bottom-up 原子应用 / `apply_edit_action` / `detect_eol` CRLF 保留）
- `executor/patch.rs` — unified diff（`parse_patch` 解析 `*** Update File:` 信封 / `apply_patch_to_content` similar fuzzy seek / `apply_patch_action` 多文件原子，`FUZZY_RATIO_THRESHOLD = 0.75`）
- `executor/fs.rs` — 文件浏览（`execute_glob` rg --files + 回退 walk、mtime 排序；`execute_list_dir` 目录优先 + 分页；`MAX_GLOB_RESULTS = 200`）
- `executor/search.rs` — ripgrep 封装（`SearchOptions` / `OutputMode`（files_with_matches 默认）/ `execute_search_code`；64KB 记录拒绝、100 submatch 上限、2000 字符行截断、`.git` 排除、offset/head_limit 分页、无效正则报"正则无效"）
- `executor/symbols.rs` — tree-sitter 多语言符号大纲（`symbol_outline` / `SymbolKind` / `language_from_extension`，rust/ts/tsx/js/py/go，`MAX_SYMBOLS = 500`，解析错误置 errors 标志）
- `executor/project.rs` — 项目探测（`probe_project` walk-up 标记检测：Cargo.toml > pyproject.toml > tsconfig.json；`ProjectFormat` / `verification_command`）
- `executor/test_discovery.rs` — 测试发现与结果解析（`discover_tests` 解析 cargo/pytest/vitest 列表，上限 500；`parse_test_output` 失败 ≤20 + 回溯头 30/尾 20 行、按文件分组；`resolve_test_command` / `run_tests_action`）
- `executor/web.rs` — Web 工具执行器（`WebSearchClient`：web_search 结构化结果 + web_fetch 可读正文提取；DuckDuckGo HTML / SearXNG JSON 双后端；SSRF 防护 `validate_public_url`（与 http_request 同策略）+ 响应大小上限 + TTL 缓存）

**核心类型**：

| 类型 | 说明 |
|------|------|
| `Action` | 动作定义（ReadFile / WriteFile / ExecuteCommand / SearchCode / ApplyEdit / ApplyPatch） |
| `ExecutorError` | 执行器错误类型 |
| `SecurityPolicy` | 安全策略（命令白名单/黑名单、目录限制、command_timeout） |
| `ApprovalWorkflow` | 审批工作流（Safe/Low/Medium/High/Critical 五级风险） |
| `VerificationGate` | 验证门控（执行后自动运行 cargo check/测试验证产出） |
| `LlmJudge` | LLM-as-Judge（语义判断，解析 `-- JUDGMENT: PASS/FAIL/NEEDS_CHANGES`） |
| `StructuredDiagnostic` | 结构化编译诊断（file/line/column/level/code/message/suggestion） |
| `EditSpec` | 语义化编辑规格（start_line / anchor / old_lines / new_lines，hashline 防陈旧校验） |
| `Truncated` | 截断结果（text / truncated / total_lines / total_bytes / spill_path） |

### 1.6 lsp 子模块

**职责**：语言服务器协议（LSP）客户端，提供诊断、跳转、符号等代码智能能力，供 `lsp` 工具调用（goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation）。诊断 = 内环快信号，`verify_build` = 最终权威门控（见 gap-analysis D4）。

**模块组织**：
- `lsp/registry.rs` — 服务器注册表：`ServerSpec { language_id, extensions, root_markers, spawn_command, args, auto_install_hint }`；`BUILTIN_SERVERS` 内置 rust-analyzer / typescript-language-server / pyright-langserver / gopls；`spec_for_extension` 按扩展名路由、`probe_project_root` 按 root marker 探测项目根
- `lsp/client.rs` — 自研 JSON-RPC 2.0 客户端：Content-Length 帧、tokio 进程管道、DashMap + oneshot 请求/响应关联、10s 超时；服务器不可用时优雅降级（"不可用" + 安装提示）
- `lsp/diagnostics.rs` — `LspManager`：按绝对路径键控的 push 诊断存储、按项目根键控的服务器池、`diagnostics_for` 排序输出、`ensure_server` 惰性拉起、`query` 统一入口

**核心类型**：

| 类型 | 说明 |
|------|------|
| `ServerSpec` | LSP 服务器规格（语言 ID、扩展名、root 标记、spawn 命令、自动安装提示） |
| `LspClient` | JSON-RPC 2.0 LSP 客户端（initialize / hover / goto_definition / references / document_symbols / workspace_symbols / goto_implementation） |
| `LspManager` | 诊断存储 + 服务器池管理（handle_publish / diagnostics_for / ensure_server / query） |

### 1.7 snapshot 子模块

**职责**：工作区快照（会话回退/撤销回退），ADR-006 定义的 VFS 例外（独立文件存储于 `{data_dir}/snapshots/`，不经 VFS）。编程助手落地（ADR-008）新增三项能力：gzip 压缩、GC、similar diff。

**模块组织**：
- `snapshot/mod.rs` — `SnapshotManager`：内容寻址对象库（sha256-of-raw 键，全局去重）+ 树文件（`trees/{index}.json`）+ redo 增量（`redo/`）
- **gzip 压缩**（`flate2`）：对象以 gzip 流写入，读取时按魔数 `0x1f 0x8b`（`GZIP_MAGIC`）识别，legacy 未压缩对象向后兼容；对象键不变，去重语义不变
- **`gc()` 标记-清除**：可达集 = 所有会话 `trees/*.json` + `redo/tree-*.json`，清理孤儿 redo cache 文件与不可达对象，返回 `GcStats`
- **`diff(session_id, index)`**：经 `similar::TextDiff::from_lines` 生成 `DiffResult` / `FileDiff`（added / removed / modified / binary 分类 + unified 文本），服务 `/api/v1/workspace/diff`

**核心类型**：

| 类型 | 说明 |
|------|------|
| `SnapshotManager` | 快照管理器（capture / restore / save_redo / load_redo / gc / diff） |
| `GcStats` | GC 统计（标记对象数、删除孤儿数、释放字节数） |
| `DiffResult` / `FileDiff` | 快照间差异（文件级分类 + unified 文本） |

### 1.8 context 子模块

**职责**：上下文工程系统，包括双层检索、对话压缩、统一上下文管线和消息组装。

**子模块组织**：
- `context/retrieval/` — 意图分析、双层向量检索（DualLayerRetriever）、内容加载（ContentLoadStrategy）、检索追踪（`RetrievalTraceBuilder` 构建器；`RetrievalTrace` 类型定义于 `common/types/retrieval_trace.rs`，此处 re-export）
- `context/compression/` — 对话压缩（ContextCompressor）；`TokenEstimator` / `estimate_tokens` 定义于 `common/token_estimator.rs`，此处 re-export（全系统唯一估算入口）
- `context/pipeline.rs` — `ContextPipeline`，统一上下文管线（规则注入 → 检索 → 压缩）
- `context/assembler.rs` — `ContextAssembler`，纯函数：存储层 `StructuredMessage` → 传输层 `Message`

**核心类型**：

| 类型 | 说明 |
|------|------|
| `ContextPipeline` | 统一上下文管线，封装检索→规则注入→压缩的完整流程 |
| `ContextCompressor` | 对话压缩器，保留最近消息 + 分层压缩策略 |
| `CompressionConfig` | 压缩配置（preserve_recent_messages 默认 6） |
| `ContextAssembler` | 存储层/传输层消息格式转换（纯函数） |
| `DualLayerRetriever` | 双层向量检索器（Intent 分析 + 向量搜索 + 内容加载） |
| `ContentLoadStrategy` (enum) | 基于分数的内容加载策略：Full(大于0.85→L2) / Overview(0.6~0.85→L1) / Abstract(小于等于0.6→L0) |
| `IntentAnalyzer` | 意图分析器：LLM 分析查询意图 + 目标命名空间 |

**核心架构决策——前缀匹配原则**：

`ContextAssembler::assemble()` 拼装顺序严格固定，不可变更：

```
soul → rules+memories → history(from compression_marker) → current input
```

- **固定前缀（soul + rules + memories）**：会话期间不变化，利用 LLM Provider 前缀匹配缓存，只计算一次
- **可变后缀（history）**：compression_marker 决定拼接起点，最近 marker 之后的消息
- **绝对禁止**：把 soul/rules/memories 放在 history 之后——破坏前缀缓存，每次请求重新计算全部 token

**规则管线**：规则记录/提炼逻辑通过 Scheduler 定时运行：
- `RuleTask` (`scheduler/tasks/rule_task.rs`) — 规则提炼的 cron 壳
- `RuleSuggester` (`scheduler/tasks/rule_suggester.rs`) — 扫描聚类 + LLM 提炼
- `RuleRecorder` (`scheduler/tasks/rule_recorder.rs`) — 去重 + 写入 learned rule

### 1.9 skills 子模块

**职责**：管理 Agent 可调用的技能，包括技能定义、执行、注册、发现和学习（GEPA 进化引擎）。

**模块组织**：
- `skills/definition.rs` + `skills/types.rs` — 技能定义、参数模式和注册表
- `skills/executor.rs` — 技能执行器（参数验证 + 安全检查 + 执行监控）
- `skills/manager.rs` — 技能管理器
- `skills/handlers/` — 6 个内置技能处理文件（file_read, file_write, file_delete, file_list, system_command, http_request）
- `skills/registry.rs` — 技能注册表工厂（`create_builtin_skills`, `register_builtin_skills`）
- `skills/learning/` — GEPA 进化引擎（mod.rs 核心引擎、types.rs 类型定义、generator.rs 技能生成逻辑）

**核心类型**：

| 类型 | 说明 |
|------|------|
| `Skill` | 技能定义（id、name、description、parameters、handler） |
| `SkillRegistry` | 技能注册表，支持运行时动态注册/发现/执行 |
| `SkillExecutor` | 技能执行器（参数验证 + 安全检查 + 执行监控） |
| `SkillManager` | 技能管理器，`list_available_skills()` 读取所有技能的 abstract |
| `SkillHandler` | 技能处理器 trait |
| `SkillLearningEngine` | GEPA 进化引擎，从执行历史中自动提取可复用技能 |
| `ExecutionHistory` | 执行历史记录，用于 GEPA 引擎 |
| `GeneratedSkill` | GEPA 引擎生成的技能 |

**GEPA 进化引擎**：
- **G**enerate：分析执行历史中的成功模式
- **E**volve：生成 GeneratedSkill { id, name, description }
- **P**erfect：通过多次使用优化参数模板
- **A**dapt：根据上下文自动调整技能执行策略

### 1.10 observability 子模块

**职责**：Agent 的可观测性存储，记录执行指标并支持 Agent 自省查询。模块包含 2 个文件（mod.rs、usage_stats.rs）；SQLite 连接复用 `vfs/backend/sqlite_db.rs` 的 `SqliteDb`（ADR-005 单连接，ADR-007 下沉），`usage_stats.rs` 依赖 `common::types::retrieval_trace::RetrievalTrace` 持久化检索轨迹。

**核心类型**：

| 类型 | 说明 |
|------|------|
| `AgentMetrics` | 可观测性存储，记录 Token 消耗、成功率、规则有效性 |
| `TokenRecord` | 单次执行的 Token 消耗记录 |
| `FailureStats` | 步骤失败统计（step_description、failure_count、last_error） |
| `RuleHitRecord` | 规则命中记录 |

**Agent 自省接口**：
- `query_token_summary()` — Token 消耗历史和平均值
- `query_success_rate()` — 执行总数、成功数、成功率百分比
- `query_rule_effectiveness()` — 规则注入总数、命中数、相关性百分比
- `query_harness_health()` — Harness 健康摘要
- `record_token_usage()` / `record_execution()` / `record_rule_hit()` / `record_pipeline_failure()` / `record_skill_call()` — 记录接口

### 1.11 其他子模块

| 子模块 | 核心类型 | 说明 |
|--------|---------|------|
| `config` | `TianyanConfig`, `AgentConfig`, `ModelsConfig`, `ConfigStatus` | 全局配置管理，支持 TOML + env。查找顺序：`./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml` |
| `common` | `TianyanError`, `Message`, `TianyanUri`, `Embedding`, `TokenUsage`, `StructuredMessage`, `ContentPart`, `ImageUrl`, `LoggingConfig`, `TokenEstimator` | 通用错误（禁止引入新错误类型）、URI、向量、消息（含多模态 `content_parts`，ADR-010）、记忆类型、日志配置、token 估算（叶模块，无 core 内部依赖） |
| `eval` | `AnswerJudge`, `AnswerEvaluation`, `DimensionScores`, `EvalCase`, `EvalResult`, `golden_cases` | 回答质量评测（LLM-as-Judge 评分式）：四维度（相关性/正确性/完整性/清晰度）1-10 分 + 加权总分 + 分级判定；解析回退链（JSON → 围栏提取 → 行格式 → 中性 5 分）；`run_eval_suite` 批处理 + `format_report` 报告；依赖 `model::ChatService` 与 `common::types`，与 `executor::judge`（工具执行二值门控）正交 |
| `session` | `Session`, `SessionManager` (trait), `PersistentSessionManager` | 会话管理，支持 VFS 持久化；`load_session_from_vfs()` 用 `compression_marker` 截断；截断常量单点定义于 `session/mod.rs`（`MAX_SESSION_MESSAGES=100` / `KEEP_RECENT_MESSAGES=50`）；JSONL 首行 SessionHeader 承载注入上下文快照（ADR-012）+ 会话元数据（created_at/title/ended_at，重启恢复；`list_sessions` 按目录条目过滤防幽灵会话） |
| `memory` | `MemoryExtractor`, `ExtractionConfig` | 从会话文本中提取结构化记忆的纯功能，与调度/持久化解耦 |
| `knowledge` | `KnowledgeIngestor`, `KnowledgeIngestorBuilder`, `CompositeParser`, `ImageProcessor` | 知识库导入（已通过 `knowledge_ingest` 工具集成到 Agent 流程）。ingestor/ 拆分为 mod + builder；`image/` 拆分为 types/processor/analyzer |
| `scheduler` | `TaskScheduler`, `TaskHandler` (trait), `TaskContext`, `RuleTask`, `GcTask`, `MemoryTask`, `SummaryTask`, `RuleRecorder`, `RuleSuggester`, `UsageStatsFlushTask` | 定时任务调度框架 + 所有任务实现，位于 `scheduler/tasks/`；`TaskResult.error: Option<TianyanError>`（结构化错误）；GcTask 职责为规则归档 + 记忆 TTL 清理（文档漂移检测/质量报告投机代码已删除）；UsageStatsFlushTask 定期把使用统计内存计数器刷入 SQLite（构造器注入，同 SnapshotGcTask 模式） |

---

## 2. 集成状态汇总

| 模块 | 状态 | 说明 |
|------|------|------|
| agent | ✅ 完整集成 | Agent、AgentLoop、ToolRegistry、SessionState、ContextPipeline 全部集成 |
| model | ✅ 完整集成 | ModelServices + AsyncOpenAIClient 已集成；原 ModelRouter 已移除 |
| context | ✅ 完整集成 | ContextPipeline + DualLayerRetriever + ContextAssembler 在 Agent 中完整集成 |
| skills | ✅ 完整集成 | 含 GEPA 进化引擎，通过 call_skill 工具桥接 |
| vfs | ✅ 完整集成 | VirtualFileSystemImpl + SqliteBackend + LanceDbVectorStore；双层摘要索引 |
| scheduler | ✅ 完整集成 | TaskScheduler + RuleTask + MemoryTask + SummaryTask + GcTask + UsageStatsFlushTask |
| config | ✅ 完整集成 | 配置加载器和验证器 |
| common | ✅ 完整集成 | 错误类型和通用工具 |
| session | ✅ 已集成 | `PersistentSessionManager` 持久化到 VFS |
| memory | ✅ 已集成 | `MemoryExtractor` 提取结构化记忆 |
| observability | ✅ 已集成 | AgentMetrics 提供可观测性存储和自省接口 |
| executor | ✅ 正常使用 | 独立执行函数、审批工作流、验证门控均被 agent 模块使用 |
| eval | ✅ 已实现（离线） | AnswerJudge 评分式评测 + 黄金用例；离线基准用，未接入在线对话链路 |
| lsp | ✅ 已集成 | 自研 LSP 客户端（注册表 + JSON-RPC 传输 + 诊断存储），通过 lsp 工具接入 |
| snapshot | ✅ 已集成 | gzip 压缩 + GC + similar diff（ADR-006 例外，ADR-008 升级） |
| knowledge | ✅ 已集成 | KnowledgeIngestor 已通过 knowledge_ingest 工具集成到 Agent 流程，Server 层通过 KnowledgeIngestor 真实处理导入与检索 |

**已删除模块**：`planner/`（Planner-Executor 架构已废弃，仅保留 `ClarificationQuestion` 类型在 agent 中导出）
**已删除类型**：`ModelRouter`、`TokenBudget`、`DocumentChunker`、`ChunkingConfig`、`ConversationSummarizer`、`VisionEncoder`、`AgentHarness`（wrapper struct）、`AgentSkills`（wrapper struct）、`MemoryExtractionTrait`、`ContextRetriever` (trait)
**已拆分/下沉文件**（公开 API 路径不变）：`agent/tool_registry/executors.rs` → `file_ops.rs`/`code_ops.rs`/`knowledge_ops.rs`/`agent_ops.rs`；`observability/sqlite_db.rs` → `vfs/backend/sqlite_db.rs`；`config/logging.rs` → `common/logging.rs`；`context/compression/estimator.rs` → `common/token_estimator.rs`；`context/retrieval/types.rs` 的 RetrievalTrace → `common/types/retrieval_trace.rs`；`executor/actions.rs` 拆分出 `security.rs`/`command.rs`/`output_parse.rs`（详见 ADR-007）

---

## 3. Server 模块 (tianyan-server)

### 3.1 模块总览

Server 是天演的 HTTP API 层，基于 Axum 框架，提供 REST API、SSE 流式传输和静态文件服务。

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

### 3.2 API 端点清单

#### 配置向导（没有版本前缀）

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
| `/knowledge/ingest` | POST | 上传文档（Multipart） | ❌ |
| `/knowledge/ingest/{job_id}/status` | GET | 导入任务状态 | ❌ |
| `/knowledge/search` | GET | 知识库检索 | ❌ |
| `/knowledge/search/suggestions` | GET | 检索建议 | ❌ |

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

### 3.3 关键组件

**AppState** (`state.rs`)：
- 持有 `Agent`、`Config`、`VFS`、`SessionManager`、`SkillRegistry`、`SkillExecutor`、`SummaryService`
- VFS 初始化分离：基础设施 → `VFS::initialize()`；应用内容（soul.md、learned 目录）→ `server/src/lib.rs::bootstrap_app_vfs()`

**AgentBuilderFactory** (`agent_builder.rs`)：
- 静态工厂类，负责 Agent 实例的创建和配置验证
- `build_agent_or_wizard`：构建失败时降级为 `WizardModeAgent`

**core_bridge** (`core_bridge.rs`)：
- `convert_message`：API 消息类型 → Core 消息类型
- `convert_token_usage`：Core Token 使用量 → API Token 使用量

**错误处理** (`shared/error.rs`)：
- `ApiError` 统一错误类型（NotFound / BadRequest / Internal / Config / Agent）
- `ErrorResponse { error: String }` 统一错误响应格式

---

## 4. GUI 模块 (tianyan-gui)

### 4.1 模块总览

GUI 是天演的 React TypeScript 前端，采用 Zustand 模式管理全局状态。

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
| `state` | 全局状态管理（Zustand） | mod.rs |

### 4.2 前端 API 覆盖情况

| 后端功能域 | 前端 API 模块 | 状态 |
|-----------|-------------|------|
| 聊天对话 | `api/chat` | ✅ 完整实现（流式 + regenerate + edit） |
| 会话管理 | `api/sessions` | ✅ 完整实现（CRUD + title 更新） |
| 技能执行 | `api/skills` | ✅ 完整实现（列表 + 执行 + 状态轮询） |
| 知识管理 | - | ❌ 未实现 |
| 运行时配置 | - | ❌ 未实现 |
| 配置向导 | `config_wizard/api` | ✅ 完整实现 |

### 4.3 状态管理

**AppState** 使用 Zustand store，包含多种 Action：

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
| `AppendStreamMessage` | 追加流式过程消息 | ✅ |
| `AppendSkillCalls` | 附加技能调用信息 | ✅ |
| `SetStreamStatus` | 设置流状态（Idle/Streaming/Error） | ✅ |
| `ToggleSidebar` | 切换侧边栏 | ✅ |
| `UpdateSettings` | 更新本地设置 | ✅ |
| `ClearMessages` | 清空消息 | ✅ |
| `SetSkills` | 设置技能列表 | ✅ |
| `RegenerateFrom` | 从指定索引截断消息 | ✅ |
| `EditMessage` | 编辑指定消息内容 | ✅ |
| `DeleteMessagesFrom` | 删除指定索引之后的消息 | ✅ |

### 4.4 流式响应处理

前端通过 `ReadableStream` API 处理 SSE 流式响应，支持：

- **6 种 chunk_type 差异化渲染**：
  - `Thought` → 独立消息气泡，思考中标签
  - `ToolCall` → 独立消息气泡，工具调用标签
  - `Observation` → 独立消息气泡，观察结果标签
  - `Answer` / `Clarification` → 追加到最后一条助手消息
  - `Error` → 错误内容追加
- **可中断**：通过 `AbortController` 实现停止生成
- **技能调用**：skill_calls 附加为调用卡片

---

## 5. Tauri 模块 (tianyan-tauri)

### 5.1 模块总览

Tauri 是天演的桌面应用壳，负责窗口管理和服务器生命周期。

| 文件 | 职责 |
|------|------|
| `lib.rs` | 应用入口：初始化日志、加载配置、启动服务器、健康检查、创建窗口 |
| `server.rs` | 服务器启动封装：`start_axum_server`（监督循环启动路径） |
| `main.rs` | 二进制入口：调用 `tianyan_tauri_lib::run()` |

### 5.2 启动参数

| 参数 | 值 | 说明 |
|------|---|------|
| 服务器地址 | `127.0.0.1:3000` | 固定值 |
| 健康检查超时 | 30 秒 | `HEALTH_CHECK_TIMEOUT` |
| 健康检查间隔 | 500 毫秒 | `HEALTH_CHECK_INTERVAL` |
| 日志目录 | `%APPDATA%/com.tianyan.app/logs/` | Windows 默认路径 |

### 5.3 与其他模块的交互

```
Tauri lib.rs::run()
    │
    ├─→ tianyan::config::TianyanConfig  (读取配置)
    ├─→ tianyan_server::start_server()  (启动 HTTP 服务器)
    │     └─ bootstrap_app_vfs → AppState::new → axum::serve
    ├─→ reqwest GET /health             (健康检查)
    └─→ Tauri WebView → gui/dist/       (加载前端)
```

---

**文档版本**: 2026-08-07
**最后更新**: 2026-08-07（编程助手落地同步：executor 新增 hashline/truncate/edit/patch/fs/search/symbols/project/test_discovery 子模块，新增 lsp 模块，snapshot 升级 gzip+GC+diff（ADR-007/ADR-008），工具 14→21，Agent 章节工具清单更新）
**历史**: 2026-08-06（Wave 6 重构后同步：executor 拆分 security/command/output_parse、tool_registry 4 域文件、approval//image//lancedb/ 目录化、SqliteDb/RetrievalTrace/LoggingConfig/TokenEstimator 下沉（ADR-007）、observability 2 文件、SessionManager 仅存 PersistentSessionManager、GcTask 删除投机代码、会话截断常量单点）
**历史**: 2026-08-04（重构：storage→vfs，移除 planner/chunker/ModelRouter/TokenBudget/ConversationSummarizer/VisionEncoder/AgentHarness/AgentSkills wrapper，修正 ContentLoadStrategy→enum、L1 tokens→~2K、MemoryExtractionTrait→MemoryExtractor，反映 4 项核心架构决策，model/router+openai→provider，tasks→scheduler/tasks，executor 标注废弃，knowledge 确认未集成，session 确认已集成）
**历史**: 2026-08-04（审查改进 16 项：knowledge_ingest 工具确认已接入 Agent 流程；ToolRegistry 拆分 execute_single 为 14 个独立工具方法；审批默认关闭无人值守、拒绝降级 ask_user 追问；TianyanError 新增 not_found/is_not_found 结构化错误分类；AppState 复用 ModelServices；SummaryTask 缓存 FIFO 淘汰；SSE 事件 id 语义对齐）

