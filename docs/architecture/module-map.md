# 天演模块索引

> 快速导航：每个模块的职责、位置、关键文件。详细功能见 [module-descriptions.md](../module-descriptions.md)，模块间关系和调用流程见 [module-relationships.md](../module-relationships.md)。

---

## Crate 结构

| Crate | Package | 类型 | 位置 |
|-------|---------|------|------|
| `core/` | `tianyan-core` (lib: `tianyan`) | 纯库 | `core/src/` |
| `server/` | `tianyan-server` | Axum HTTP 服务 | `server/src/` |
| `gui-vite/` | - | React TS 前端 | `gui-vite/src/` |
| `tauri/` | `tianyan-tauri` | Tauri 桌面包装 | `tauri/src/` |
| `mcp/` | `tianyan-mcp` | MCP 协议客户端 | `mcp/src/` |

共享依赖统一在 workspace `[workspace.dependencies]` 中定义。

---

## Core 子模块

| 子模块 | 位置 | 职责 | 关键文件 |
|--------|------|------|---------|
| `agent` | `core/src/agent/` | Agent 协调器 + AgentLoop + ToolRegistry + 会话状态 + 后台任务；ToolRegistry 工具执行走可插拔管线（`tool_registry/pipeline.rs`：pre-execute 监听器/单调守卫/post-execute 监听器，DSH A1/A4 吸收）+ 内置可观测性监听器（`tool_registry/observability.rs`：统计/Trace/GEPA 历史/规则学习）；内置工具元数据单一事实源（`tool_registry/builtin_tools.rs`：schema/展示意图） | `coordinator.rs`, `loop.rs`, `loop_tests.rs`, `tool_registry/`（含 `pipeline.rs`、`observability.rs`、`builtin_tools.rs`）, `session_state.rs`, `builder.rs`, `background.rs` |
| `common` | `core/src/common/` | 通用类型、错误处理、日志配置、token 估算、`StructuredMessage`、多模态片段（`ContentPart`/`ImageUrl`）；横切单点：UTF-8 截断（`truncate.rs`）、HTTP 客户端工厂（`http.rs`） | `error.rs`, `logging.rs`, `token_estimator.rs`, `truncate.rs`, `http.rs`, `types/`（含 `retrieval_trace.rs`、`content_part.rs`） |
| `config` | `core/src/config/` | TOML 配置管理 + 环境变量 + 向导 | `mod.rs`, `wizard.rs`, `validation.rs` |
| `context` | `core/src/context/` | 上下文工程（检索 + 压缩 + 管线 + 组装） | `pipeline.rs`, `assembler.rs`, `retrieval/`, `compression/` |
| `executor` | `core/src/executor/` | 工具执行支撑（Action、审批、LLM-as-Judge、验证门控）+ 编程助手执行原语（内容匹配编辑、patch、文件浏览、搜索、符号、测试发现）+ Web 工具（搜索/抓取）；统一截断层含单行截断（`truncate.rs`：`truncate_line`/`MAX_LINE_CHARS`） | `actions.rs`, `security.rs`, `command.rs`, `output_parse.rs`, `approval/`, `verification.rs`, `judge.rs`, `truncate.rs`, `edit.rs`, `patch.rs`, `fs.rs`, `search.rs`, `symbols.rs`, `project.rs`, `test_discovery.rs`, `web.rs` |
| `lsp` | `core/src/lsp/` | LSP 客户端（服务器注册表 + 自研 JSON-RPC 传输 + 诊断存储） | `registry.rs`, `client.rs`, `diagnostics.rs` |
| `knowledge` | `core/src/knowledge/` | 知识库导入（解析、图像、注入管道） | `ingestor/`, `parser.rs`, `image/` |
| `memory` | `core/src/memory/` | 长期记忆提取 | `extractor.rs` |
| `model` | `core/src/model/` | 模型服务容器（`ModelServices`）+ provider 实现 | `traits.rs`, `services.rs`, `provider/` |
| `observability` | `core/src/observability/` | 可观测性 + 使用统计（`AgentMetrics`、`UsageStats`；SQLite 连接经 `vfs::backend::sqlite_db` 复用） | `mod.rs`, `usage_stats.rs` |
| `scheduler` | `core/src/scheduler/` | 定时任务调度器 + 任务实现 | `task_scheduler.rs`, `tasks/` |
| `session` | `core/src/session/` | 会话管理（⚠️ ADR-018 VFS 例外：`SessionStore` SQLite 权威存储，原子取号 + 失败上抛；`PersistentSessionManager` 业务语义；`SessionRecall` FTS 回忆；`session_meta` 存 SessionHeader/injectable 快照）；截断常量单点（`MAX_SESSION_MESSAGES`/`KEEP_RECENT_MESSAGES`） | `store.rs`, `manager.rs`, `search.rs`, `types.rs` |
| `skills` | `core/src/skills/` | 技能定义、执行、学习（GEPA 进化引擎） | `definition.rs`, `executor.rs`, `manager.rs`, `handlers/`, `learning/` |
| `snapshot` | `core/src/snapshot/` | 工作区快照（回退/撤销回退，⚠️ ADR-006 VFS 例外）；重做子系统独立（`redo.rs`，与 capture/restore/diff/gc 正交） | `mod.rs`, `redo.rs` |
| `vfs` | `core/src/vfs/` | 统一存储与检索层（**项目基础机制**） | `traits.rs`, `vfs_impl.rs`, `backend/local.rs`, `backend/sqlite.rs`, `backend/sqlite_db.rs`, `vector/lancedb/`, `summary/engine.rs` |

---

## Server 子模块

| 子模块 | 位置 | 职责 |
|--------|------|------|
| `api/chat` | `server/src/api/chat/` | 对话 API（流式 SSE + 非流式 + regenerate + edit） |
| `api/sessions` | `server/src/api/sessions/` | 会话管理 API |
| `api/knowledge` | `server/src/api/knowledge/` | 知识管理 API（摄入 + 检索） |
| `api/skills` | `server/src/api/skills/` | 技能执行 API |
| `api/tasks` | `server/src/api/tasks/` | 后台任务列表 API（delegate background 任务视图） |
| `api/config` | `server/src/api/config/` | 配置管理 API + 向导 |
| `state` | `server/src/state.rs` | AppState 生命周期管理 |
| `agent_builder` | `server/src/agent_builder.rs` | Agent 构建工厂 |
| `mcp_bridge` | `server/src/mcp_bridge.rs` | MCP 工具桥接（配置服务器 → ToolRegistry 动态工具；截图等图片结果落盘 `{data_dir}/mcp_images/`） |

---

## GUI 子模块

| 子模块 | 位置 | 职责 |
|--------|------|------|
| `lib/` | `gui-vite/src/lib/` | 类型定义、API 客户端、Zustand 状态管理、配置转换 |
| `components/chat/` | `gui-vite/src/components/chat/` | 聊天面板（SSE 流式）；流式协议归约单点 `lib/chat-stream.ts`、思考指示器归属单点 `streaming-indicator.ts` |
| `components/sidebar/` | `gui-vite/src/components/sidebar/` | 侧边栏（会话列表） |
| `components/skills/` | `gui-vite/src/components/skills/` | 技能中心面板 |
| `components/knowledge/` | `gui-vite/src/components/knowledge/` | 知识管理面板 |
| `components/settings/` | `gui-vite/src/components/settings/` | 设置面板（14 Tab） |
| `components/wizard/` | `gui-vite/src/components/wizard/` | 初次配置向导 |
| `components/layout/` | `gui-vite/src/components/layout/` | 布局、错误边界、Toast |
| `components/ui/` | `gui-vite/src/components/ui/` | 布局/输入原语（`ListDetailPanel` 列表-详情、`FieldRow`、`Spinner`、`ErrorBanner`、`EmptyState`、`ConfirmDialog`） |
| `hooks/` | `gui-vite/src/hooks/` | 自定义 Hooks（SSE 流、键盘、主题） |

---

## 已删除/废弃组件

| 组件 | 状态 | 替代 |
|------|------|------|
| `planner/` | 已删除 | Agent Loop + ToolRegistry |
| `ModelRouter` | 已删除 | `ModelServices` |
| `TokenBudget` | 已删除 | `ContentLoadStrategy::from_score()` |
| `Chunker` | 已删除 | VFS 双层检索替代 chunk-based RAG |
| `SqliteSessionStore` | 已删除 | `PersistentSessionManager`（基于 VFS）；ADR-018 后其思想以 `SessionStore` 形式回归（会话迁出 VFS） |
| `server/src/api/vfs/` | 已删除 | VFS 管理 API 未完成，已移除 |
| `AgentHarness` wrapper | 已删除 | 功能由 `Agent` 直接持有 |
| `AgentSkills` wrapper | 已删除 | 功能由 `Agent` 直接持有 |
| `hooks/use-debounced-value.ts` | 已删除 | 浅 hook 内联（唯一调用点 KnowledgeSearchTab 直接实现） |

> **注**：存储后端已 trait 化（`StorageBackend` seam，ADR-005）。`LocalFileBackend` 为默认生产后端；`SqliteBackend` 已接入,通过配置 `[storage] backend = "sqlite"` 启用。

---

## 核心架构决策

所有架构决策记录在 [decisions/](decisions/) 目录：

- [ADR-001: VFS 双层摘要索引](decisions/001-vfs-dual-layer-index.md)
- [ADR-002: StructuredMessage](decisions/002-structured-message.md)
- [ADR-003: 组件工具化](decisions/003-component-toolization.md)
- [ADR-004: 前缀匹配上下文组装](decisions/004-prefix-match-context-assembly.md)
- [ADR-005: SQLite 作为主存储后端](decisions/005-sqlite-backend.md)
- [ADR-006: 工作区快照独立存储](decisions/006-snapshot-storage-exception.md) — snapshot 的 VFS 例外
- [ADR-007: Core 依赖环消除与共享基础设施归属](decisions/007-core-dependency-cycle-removal.md) — SqliteDb/RetrievalTrace/LoggingConfig/TokenEstimator 下沉决策
- [ADR-009: 语义化编辑双原语](decisions/009-semantic-editing.md) — 内容匹配 + unified diff 信封（apply_edit / apply_patch）
- [ADR-008: 快照升级](decisions/008-snapshot-upgrade.md) — gzip 压缩 + GC + similar diff（扩展 ADR-006）
- [ADR-010: 对话多模态链路](decisions/010-multimodal-message-chain.md) — 图片输入（Message.content_parts + Part::Image）+ MCP 截图落盘
- [ADR-011: 子任务授权边界](decisions/011-subagent-approval-boundary.md) — 子 agent 无交互审批
- [ADR-012: 注入上下文快照持久化](decisions/012-injectable-snapshot-persistence.md) — 前缀零漂移（SessionHeader；ADR-018 后存 `session_meta` 表）
- [ADR-018: 会话权威存储迁至 SQLite](decisions/018-session-authoritative-sqlite.md) — 会话迁出 VFS；`SessionStore` 原子取号 + 失败上抛
- [ADR-013: 统一消息通知与唤醒原语](decisions/013-unified-message-notification-wake.md) — 消息入库 + 唤醒语义
- [ADR-014: 错误分类语义谓词](decisions/014-error-classification.md) — not_found/conflict/invalid_input 语义谓词
- [ADR-015: 会话工作区绑定](decisions/015-session-workspace-binding.md) — 工作区是会话的父级分组
