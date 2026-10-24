# 天演模块索引

> 快速导航：每个模块的职责、位置、关键文件。详细功能见 [module-descriptions.md](../module-descriptions.md)，模块间关系和调用流程见 [module-relationships.md](../module-relationships.md)。
> **最后更新**: 2026-09-12（D 轮补全：专题文档导航、ADR 编号补齐、role_store 去向、事件总线有界化）

**专题文档**（机制级说明，先读这些再看模块）：

| 文档 | 覆盖 |
|------|------|
| [context-pipeline.md](context-pipeline.md) | 上下文组装顺序（含 project_instructions）+ 压缩机制（触发口径/阈值/摘要字段/user 锚定）+ 构成量化方法 + 故障模式 |
| [event-protocol.md](event-protocol.md) | 事件通道模型 + 4 类事件字段表 + 订阅与快照恢复 + 可靠性分层 + 防死锁不变量 |
| [model-provider-notes.md](model-provider-notes.md) | Provider/协议矩阵 + reasoning 回传契约 + 思考强度×语言实测 + 前缀缓存 + 上游异常 |
| [task-runtime.md](task-runtime.md) | 三类任务 + 并发排队 + TTL + 唤醒语义 + 取消/回退 + 子会话 FTS 边界 + 面板数据流 |
| [principles.md](principles.md) | 设计原则 |
| [refactoring-practices.md](refactoring-practices.md) | 重构实践指南（审查/波次/QA 纪律） |
| [../operations/troubleshooting.md](../operations/troubleshooting.md) | 日志与三类高频故障排查 + DB 取证 + QA 纪律 |
| [../operations/data-health-check.md](../operations/data-health-check.md) | 表清单 + 巡检 SQL 集 + 解读处置 |
| [../operations/release-msi.md](../operations/release-msi.md) | 版本号落点 + 本地打包流程 + CI 发布链 + 已知坑 + 验收清单 |

---

## Crate 结构

| Crate | Package | 类型 | 位置 |
|-------|---------|------|------|
| `core/` | `tianyan-core` (lib: `tianyan`) | 纯库 | `core/src/` |
| `server/` | `tianyan-server` | Axum HTTP 服务 | `server/src/` |
| `service/config/` | `tianyan-service-config` | 服务层：配置加载 / 持久化 / 迁移 / 校验 / 向导 DTO（ADR-049 期 3-T4） | `service/config/src/` |
| `assembly/` | `tianyan-assembly` | 装配清单：**10 个插件工厂** + 两份清单（`full()` 10 插槽 / `policies()` 8 插槽前缀）+ 跨插件顺序纪律的**单一实现**（ADR-049 期 4a/4b/4c）；全仓唯一 `use` 全部插件 crate 之处 | `assembly/src/` |
| `tools-ops/` | `tianyan-tools-ops` | **运维工具**（ADR-049 期 4d）：服务停止后直接修数据的终端脚本（`skill_import` / `rebuild_overviews`）；**终端消费者——不被任何 crate 依赖**，因此插件 crate 之间零 dev 边 | `tools-ops/src/` |
| `app/` | `tianyan-app` (bin: `tianyan-server`) | 装配器（headless）：配置加载 + 日志 + 启动 + 搬迁重启；清单取自 `tianyan-assembly`（ADR-049 期 1 / 期 4） | `app/src/` |
| `plugins/*` | `tianyan-{tools-fs,tools-code,tools-todo,tools-web,storage-vector,storage-sqlite,task-builtin,task-evolution,policy-approval,policy-context,policy-review,provider-client,provider-llm,provider-embed,tool-pipeline}` | 编译期插件（只依赖 `core`，彼此零互赖） | `plugins/*/src/` |
| `gui-vite/` | - | React TS 前端 | `gui-vite/src/` |
| `tauri/` | `tianyan-tauri` | Tauri 桌面包装（第二个装配器：端口探测 + 窗口生命周期 + 关闭信号；清单同样取自 `tianyan-assembly`） | `tauri/src/` |
| `mcp/` | `tianyan-mcp` | MCP 协议客户端 | `mcp/src/` |

共享依赖统一在 workspace `[workspace.dependencies]` 中定义。

---

## Core 子模块

| 子模块 | 位置 | 职责 | 关键文件 |
|--------|------|------|---------|
| `agent` | `core/src/agent/` | Agent 协调器 + AgentLoop + ToolRegistry + 会话状态 + 后台任务（ADR-026：委托只支持异步、双信号量排队 20+40、SQL 权威注册表 + 3 天 TTL（**持久化仅委托；命令任务有意进程内保留**——§5 D 边界澄清）、子智能体会话消息落库 + 事件通道）；动态工具：`schedule_task`（server 自有能力，经 `AppState::component_dynamic_tools` 注入）；`todo`/`goal` **已迁至 `plugins/tools-todo`**（ADR-049 期 2 序 1），与 `tools-fs`/`tools-code`/`tools-web` 一样经 `PluginServices` + `ToolPluginFactory` 在装配层注入（**首建与每次 reload 共用同一注入点**——否则热重载会丢全部插件工具）；ToolRegistry 工具执行走可插拔管线（`tool_registry/pipeline.rs`：pre-execute 监听器/单调守卫/post-execute 监听器，DSH A1/A4 吸收）+ **管线接线契约**（`ToolPipelineWiring` / `ToolPipelineFactory`：装配层注入、`AgentBuilder` 按工厂返回的**有序**清单注册）；**监听器实现已出 core**（ADR-049 期 3-T5 → `plugins/tool-pipeline`：观测监听器（统计/Trace/GEPA 历史/规则学习）+ 输出落盘（ADR-047）+ 工具目录渲染）——注册表**不再自带任何监听器**（未装配 = 不落盘 + 无观测 + 告警）；内置工具元数据单一事实源（`tool_registry/builtin_tools.rs`：schema/展示意图） | `coordinator.rs`, `loop.rs`, `loop_tests.rs`, `tool_registry/`（含 `pipeline.rs`、`builtin_tools.rs`）, `session_state.rs`, `builder.rs`, `background.rs` |
| `common` | `core/src/common/` | 通用类型、错误处理、日志配置、token 估算、`StructuredMessage`、多模态片段（`ContentPart`/`ImageUrl`）；横切单点：UTF-8 截断（`truncate.rs`）、HTTP 客户端工厂（`http.rs`）、二进制嗅探（`binary.rs`：`sniff_binary` + `sniff_image_mime` 严格图片格式）、SHA-256 摘要（`hash.rs`） | `error.rs`, `logging.rs`, `token_estimator.rs`, `truncate.rs`, `http.rs`, `binary.rs`, `hash.rs`, `types/`（含 `content_part.rs`） |
| `config` | `core/src/config/` | 配置**结构与全局槽位**：`TianyanConfig` + 全部子 Config 及其 `Default`（内核与插件的共同构造参数）、预设数据表 `presets.rs`（server 配置 API 展示预设）、安全字段归一（ADR-033）、`get_config()` / `set_config()` / `try_get_config()`（OnceLock 槽位，装配层注入）。**加载 / 持久化 / 迁移 / 校验 / 向导已出 core**（ADR-049 期 3-T4 → `service/config`：`loader.rs` / `migration.rs` / `validation.rs` / `wizard.rs`） | `mod.rs`, `agent.rs`, `model.rs`, `presets.rs`, `security.rs`, `storage.rs` |
| `db` | `core/src/db/` | **统一写入门面**（ADR-020）：`Database` 门面（单连接 + schema 集中）+ `SqliteDb`（ADR-005 连接）+ 业务域 Repository（stats/trace/execution/usage）；**只依赖 `common`**（纯底层，无领域依赖） | `mod.rs`, `sqlite_db.rs`, `stats.rs`, `trace.rs`, `execution.rs`, `usage.rs` |
| `context` | `core/src/context/` | **契约 + 组装机制**：`ContextAssembler`（K1 组装，纯静态函数，ADR-043 工具对规范化 + 前缀零漂移）、`ContextPort`（3 方法 seam）、`ContextServices`（装配能力集合）、`NullContextPort`（未装配兜底）、`SessionImages`（ADR-050 tool 结果图片的组装期加载：引用 → data URL，越界引用结构上不可能）。**策略实现已出 core**（ADR-049 期 3-T2 → `plugins/policy-context`：管线 / 压缩 / 双层检索 / 意图分类） | `assembler.rs`, `port.rs`, `assembly.rs`, `tool_images.rs`；插件侧 `pipeline.rs`, `compression/`, `retrieval/` |
| `executor` | `core/src/executor/` | 工具执行支撑（Action、审批**门控契约** `ApprovalGate`、任务自审**契约** `review::{TaskReviewer, Judgment, Verdict}`、验证门控）+ 编程助手执行原语（内容匹配编辑、patch、文件浏览、搜索、符号、测试发现）+ Web 工具（搜索/抓取）+ 图片冻结副本（`image.rs`，ADR-050：magic bytes 校验 + sha256 → `{data_dir}/session_images/`，幂等）；统一截断层含单行截断（`truncate.rs`：`truncate_line`/`MAX_LINE_CHARS`）。**审批实现已出 core**（→ `plugins/policy-approval`，ADR-049 期 3-T1），`approval/types.rs` 留 core（A 级 trait `AgentCoordinator` 签名用到）；**自审实现亦已出 core**（`judge.rs` → `plugins/policy-review`，期 3-T3，模块随之改名 `review.rs` 只留 trait 与判定形状）；命令风险定级 `classify_command_risk` / `split_command_segments` / `judged_command_name` 经 `executor` 公开（审批层与执行层共用的单一事实源） | `actions.rs`, `security.rs`, `command.rs`, `output_parse.rs`, `approval/`, `verification.rs`, `review.rs`, `truncate.rs`, `edit.rs`, `patch.rs`, `fs.rs`, `search.rs`, `symbols.rs`, `project.rs`, `test_discovery.rs`, `web.rs`, `image.rs` |
| `knowledge` | `core/src/knowledge/` | 知识库导入（解析、图像、注入管道） | `ingestor/`, `parser.rs`, `image/` |
| `model` | `core/src/model/` | 模型服务**契约 + 数据**（ADR-049 期 2 序 6 之后 core 里没有实现）：`traits.rs`（S 级 `ChatService` / `EmbeddingService` / `VlmService` / `ServiceDiscovery`）、`types/`（载荷）、`spec.rs`（模型规格 + 上下文窗口）、`retry.rs`（重试策略与可重试谓词）、`assembly.rs`（**装配原语**：`ProviderLlmServices` / `ProviderEmbedServices` / `ConnectionProbe`）、`unconfigured.rs`（未配置降级服务）、`usage.rs`（`EmbeddingUsageSink` 契约）。**wire 方言单点仍是 `ProviderDialect`**（[ADR-038](decisions/038-provider-wire-dialect.md)，数据在 `config/model.rs`，解析在 provider 插件构造期一次完成） | `traits.rs`, `types/`, `spec.rs`, `retry.rs`, `assembly.rs` |

**实现去向**（[ADR-049](decisions/049-microkernel-plugin-boundaries.md) 期 2 序 6）：聊天 / 视觉 →
`plugins/provider-llm`，嵌入 → `plugins/provider-embed`，两者共用的 `AsyncOpenAIClient` +
状态码语义分类 → `plugins/provider-client`，容器 `ModelServices` → `server/src/model_services.rs`。
| `observability` | `core/src/observability/` | 可观测性 + 使用统计（`AgentMetrics`、`UsageStats`、`TraceCollector`、`ExecutionLog`、`UsageLog`、`RuleRecorder`；SQL 经 `db` 门面/Repository 收敛，组件保留内存热路径） | `mod.rs`, `usage_stats.rs`, `trace.rs`, `execution_log.rs`, `usage_log.rs`, `rule_recorder.rs`, `execution_history.rs` |
| `roles` | `core/src/roles/` | 角色基础类型（ADR-016 纯类型层：`AgentRole`/`RoleSource`/`RoleStatus`/`RoleUsage`/`DelegationRecord`）——config/agent/scheduler 共用，不依赖领域模块 | `mod.rs` |
| `role_store` | `core/src/role_store.rs` | 角色 VFS 存储（独立存储层，依赖 vfs + roles；scheduler 演化任务与 agent 共用）。**唯一路径**：`crate::role_store`——`agent/role_store.rs` 的历史 re-export 兼容层已删除（2026-09） | `role_store.rs` |
| `scheduler` | `core/src/scheduler/` | 定时任务**机制 + 装配原语**：调度器（间隔 + 宕机补跑）、执行契约（`TaskHandler`/`TaskContext`/`TaskResult`）、注册机制（`TaskDefinition`）、装配原语（`TaskRegistration`/`TaskServices`）、演化综述执行器契约（`EvolutionReviewExecutor`，server 构造实现经 `TaskServices` 注入）。**core 已无任何具体任务实现**：5 个内置任务 → `plugins/task-builtin`（ADR-049 期 2 序 5a），演化任务 → `plugins/task-evolution`（序 5b） | `task_scheduler.rs`, `task_assembly.rs`, `evolution_executor.rs`, `task_state.rs` |
| `session` | `core/src/session/` | 会话管理（⚠️ ADR-018 VFS 例外：`SessionStore` SQLite 权威存储，原子取号 + 失败上抛；**SQL 收敛于本模块**（会话专属存储，经 `db::Database` 单连接直接实现）；`PersistentSessionManager` 业务语义；`SessionRecall` FTS 回忆；`session_meta` 存 SessionHeader/injectable 快照）；存储层返回完整链（ADR-027），压缩点截断发生在组装层 | `store.rs`, `manager.rs`, `search.rs`, `types.rs` |
| `skills` | `core/src/skills/` | 技能 = VFS 方法论文档（发现/读取；**无执行语义**）：`SkillManager`（发现/读取）+ `REVIEWS_PREFIX`（评审记录目录名，技能发现时排除——复审契约已于 **ADR-049 期 4c 退役**，常量保留只为防存量 `_reviews` 目录被当成技能列出；技能质量判断改由演化智能体依据使用统计 + 用户原话完成）；`SkillManager` 因 3a 业务复用（`api/skills/services.rs` 直接持具体类型）留 core | `manager.rs` |
| `snapshot` | `core/src/snapshot/` | 工作区快照（回退/撤销回退，⚠️ ADR-006 VFS 例外）；重做子系统独立（`redo.rs`，与 capture/restore/diff/gc 正交） | `mod.rs`, `redo.rs` |
| `events` | `core/src/events/` | 事件驱动触发（文件监听 + webhook → 事件总线 → 规则动作）。事件总线为**有界通道**（`EVENT_BUS_CAPACITY = 4096` + `try_send`）：订阅者积压时丢弃该事件并计入 `dropped_events()`（绝不阻塞发布方、不无限积压内存） | `mod.rs`, `bus.rs`, `watcher.rs`, `rules.rs` |
| `goals` | `core/src/goals/` | 长期目标 + 进度跟踪（会话绑定；运行期 `goals.json`） | `mod.rs` |
| `notification` | `core/src/notification.rs` | 通知通道抽象（`NotificationSink`；桌面实现由 tauri 注入） | `notification.rs` |
| `todos` | `core/src/todos/` | 待办清单（会话绑定；运行期 `todos.json`） | `mod.rs` |
| `vfs` | `core/src/vfs/` | 统一存储与检索层（**项目基础机制**）；只留 `StorageBackend` 契约（`vfs::StorageBackend` 公开路径不变）与 `UriMapper` 等机制；**后端实现已出 core**（ADR-049 序 4：`backend/sqlite.rs`、`backend/local.rs` → `plugins/storage-sqlite`，装配层经 `StorageBackendFactory` 注入；`backend/sqlite_db.rs` 仅 re-export 兼容，实体在 `db`）；向量实现同样已出 core（序 3：只留 `VectorStorage` 契约，LanceDB 在 `plugins/storage-vector`） | `traits.rs`, `vfs_impl.rs`, `backend/mod.rs`, `uri_mapper.rs`, `vector/traits.rs`, `summary/engine.rs` |

---

## Server 子模块

| 子模块 | 位置 | 职责 |
|--------|------|------|
| `api/chat` | `server/src/api/chat/` | 对话 API（流式 SSE + 非流式 + regenerate + edit） |
| `api/sessions` | `server/src/api/sessions/` | 会话管理 API |
| `api/knowledge` | `server/src/api/knowledge/` | 知识管理 API（摄入 + 检索） |
| `api/skills` | `server/src/api/skills/` | 技能执行 API |
| `api/tasks` | `server/src/api/tasks/` | 后台任务 API：`GET /tasks`（统一列表：委托/终端）、`POST /tasks/{id}/cancel`、`GET /tasks/{id}/log`（日志分页）；`GET /events`（统一事件流，ADR-028：task_status / command_output / chat_stream / snapshot）+ `POST /events/subscribe`（快照恢复，ADR-029/031：消息不再广播，端点仅推快照） |
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
| `agent/tool_registry/observability.rs` | 已迁出 | `plugins/tool-pipeline/src/observability.rs`（观测监听器：统计 / Trace / GEPA / 规则学习；ADR-049 期 3-T5） |
| `agent/tool_registry/spill.rs` | 已迁出 | `plugins/tool-pipeline/src/spill.rs`（ADR-047 超预算输出落盘；期 3-T5） |
| `agent/tool_catalog.rs` | 已迁出 | `plugins/tool-pipeline/src/tool_catalog.rs`（工具目录渲染 + freshness 口径；期 3-T5；`cargo catalog-check` 入口改指插件） |

> **注**：存储后端已 trait 化（`StorageBackend` seam，ADR-005）。**默认后端为 `SqliteBackend`**（`config/storage.rs` 的 `#[default] Sqlite`）；`LocalFileBackend` 保留为可选后端。两者的**实现**在 `plugins/storage-sqlite`（ADR-049 序 4），`config.storage.backend` 的 adapter 选择在装配层。

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
- [ADR-016: 角色专业化与演化](decisions/016-role-specialization-evolution.md) — 角色注册表 + 角色化委托 + 使用统计驱动的演化
- [ADR-017: 统一自演化](decisions/017-unified-self-evolution.md) — 记忆/规则/技能三路自演化的统一模型
- [ADR-019: 服务端权威消息时序](decisions/019-server-authoritative-message-timeline.md) — 服务端为消息时序权威（ADR-027 前身）
- [ADR-020: 统一写入门面 Database](decisions/020-database-facade.md) — 单连接 + schema 集中 + 业务域 Repository；db 只依赖 common
- [ADR-021: 分层重构与循环消除](decisions/021-layered-refactor.md) — 基础类型层/存储层/领域层单向依赖；生产代码零模块环
- [ADR-022: 会话绑定任务面板](decisions/022-session-bound-task-ux.md) — todo/goal 会话绑定；数据目录只读 + 搬迁对话框
- [ADR-023: 配置目录与数据目录分离](decisions/023-config-data-dir-separation.md) — 配置固定永不搬迁；搬迁=复制+校验+先改配置后删源
- [ADR-024: 调度模型 cron → 间隔 + 补跑](decisions/024-interval-scheduler.md) — 单一扫描循环、last_run 持久化宕机补跑
- [ADR-025: 移除检索轨迹](decisions/025-remove-retrieval-traces.md) — 观测回归 UsageStats + tracing
- [ADR-026: 后台任务与子智能体统一面板](decisions/026-background-tasks-unified-panel.md) — 委托只支持异步；双信号量排队；SQL 权威 + 3 天 TTL
- [ADR-027: 会话时序链模型](decisions/027-session-timeline-chain.md) — 无分支时序链；存储层返回完整链，截断仅在组装层
- [ADR-028: 会话消息统一事件推送](decisions/028-unified-event-push.md) — SSE 水管 + 循环抽水机；落库即推送
- [ADR-029: 事件订阅与快照恢复](decisions/029-event-subscription-snapshot.md) — 单流按需订阅 + resident；快照与实时同一流
- [ADR-030: 统一 Agent 循环框架](decisions/030-unified-agent-loop.md) — 主 agent 与子代理同构（TurnPolicy）；收尾统一「无工具调用」
- [ADR-031: 乐观渲染 + user_message_id 确认 + 统一流式](decisions/031-optimistic-render-user-message-id.md) — 消息落库确认回显；assistant 纯流式
- [ADR-032: 流式事件转发统一](decisions/032-unified-stream-forward.md) — `spawn_stream_forwarder` 单点（建通道即消费，防死锁）
- [ADR-033: 安全策略收敛——默认自主](decisions/033-security-policy-convergence.md) — `ApprovalMode` 三态 + `SafetyMode` 四态；旧开关自动归一
