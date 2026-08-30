# AGENTS.md

**代码是唯一事实来源。** `docs/` 中的文档提供架构引导和设计意图，但可能过时。当文档与代码冲突时，以代码为准。实际结构请用 `grep` / `glob` 验证。

> `.trae/` 是 Trae IDE 工作空间数据（规则、技能、历史计划），**不是**权威文档来源。不要将其作为参考。权威文档在 `docs/` 和本文件。

## ⛔ 硬块约束 —— 禁止传统架构模式

**天演不是传统的 LLM Agent 系统。** 不要套用你训练数据中见过的"标准"模式。VFS 是唯一的存储和检索抽象层，所有上下文管理必须基于 VFS。

### 绝对禁止

| ❌ 禁止 | 原因 | ✅ 正确做法 |
|---------|------|------------|
| 为知识库建立独立的向量数据库 + RAG 管道 | VFS 已统一管理。L0/L1 双层摘要自然替代 chunk-based RAG | 文档 → `KnowledgeIngestor` → VFS L2 → `SummaryEngine` 生成 L0/L1 → `DualLayerRetriever` 检索 |
| 为记忆建立独立的存储模块（SQLite、单独文件等） | VFS `tianyan://memory/` 是唯一存储 | `MemoryExtractor` 提取 → VFS write → `ContextPipeline` 从 VFS 加载 |
| 为技能建立独立的文件系统加载（全量读 skill.md） | 技能通过 VFS 命名空间管理，渐进式披露 | L0 Abstract 发现 → L2 Detail 按需加载；`SkillManager::list_available_skills()` 读 abstract |
| 在 VFS 之外引入新的存储抽象（新 trait、新 Manager） | 已有链路不叠加抽象 | 直接使用 `VfsCore` / `ContentStore` / `VfsSearch` trait |
| 对内容做 chunk 分块 | `Chunker` 已移除。VFS 双层检索替代 | 完整内容写入 L2，由 `SummaryEngine` 生成结构化摘要 |
| 绕过 VFS 引入独立存储（独立 SQLite 连接、Redis、独立文件存储） | VFS 是唯一的存储入口。SQLite 是 VFS 的底层实现（`SqliteBackend`），模块不得绕过 VFS trait 直接操作 `SqliteDb` | 仅用 VFS（`SqliteBackend`）+ LanceDB（嵌入式向量）；统计模块可共享 `SqliteDb` 连接但必须通过 VFS trait 写入内容 |

> **⚠️ 例外（ADR-018）：会话内容不经 VFS。** 会话权威存储是 SQLite 表
> （`session_messages` 完整消息 + `session_meta` 会话级状态，共享 `SqliteDb` 连接），
> 由 `SessionStore` 统一读写（原子取号、失败上抛）；回忆检索 `SessionRecall`
> 读同一张表。VFS 不再存会话（`tianyan://session/{id}` 仅作逻辑标识，
> `vfs_read` 经兼容层导出 JSONL）。此例外与快照（ADR-006/008）同类：
> 结构性不匹配（流式追加 vs 整块文档）时允许专用存储。**其他内容类型
> （知识库/记忆/技能/角色）仍必须走 VFS，没有第二条路径。**

### VFS 统一模型（每次设计前先看）

```
所有上下文类型 → 统一写入 VFS → SummaryEngine 生成 L0/L1 摘要 → LanceDB 向量化
                                                                    ↓
查询 → embed → RRF 融合检索 (abstract_vector + overview_vector) → 分层加载
```

**关键思维转换**：当你思考"知识库怎么存"、"记忆怎么检索"、"技能怎么加载"时，答案永远是 **"通过 VFS"**。不存在第二条路径。

## 构建与检查命令

```powershell
cargo check --workspace                     # 快速编译检查
cargo fmt --all -- --check                  # 格式检查
cargo fmt --all                             # 自动格式化
cargo clippy --workspace                    # Clippy
cargo test -p tianyan-core --lib            # 单元测试
cargo test -p tianyan-core vfs::backend::local -- --nocapture  # 指定测试模块
.\scripts\test.ps1 lint                     # fmt + clippy
.\scripts\test.ps1 unit                     # 单元测试
.\scripts\test.ps1 all                      # lint + unit + integration + e2e + bench
```

## 工作区结构

5 crate，单 `Cargo.toml` workspace (resolver = "2")：

| Crate | Package | 类型 |
|-------|---------|------|
| `core/` | `tianyan-core` (lib: `tianyan`) | 纯库 |
| `server/` | `tianyan-server` | Axum HTTP 服务 |
| `gui-vite/` | - | React TypeScript 前端 |
| `tauri/` | `tianyan-tauri` | Tauri 桌面包装 |
| `mcp/` | `tianyan-mcp` | MCP 协议客户端 |

## 代码规范

- `unsafe_code = "deny"` — 禁用 unsafe
- Clippy: `unwrap_used`、`expect_used`、`unwrap_in_result` 均为 `warn`
- 公开 API 用 `///` / `//!`，不要用 `//` 行注释
- 所有错误用 `TianyanError`，禁止引入新错误类型。`TianyanError` 仅保留 4 个变体（`Io` / `Json` / `Toml` / `Custom`），模块内部错误通过 `Custom(String)` 传递，调用方在消息中携带"模块前缀：详情"，**严禁新增变体**
- **错误分类用语义谓词**（ADR-014）：构造用 `not_found` / `conflict` / `invalid_input` / `permission` / `timeout`，判定用 `is_not_found` / `is_conflict` / `is_invalid_input` / `is_permission` / `is_timeout`。**禁止在调用方对错误消息做字符串匹配分类**（`starts_with` / `contains` 判定错误类别）——分类契约只在 `core/src/common/error.rs` 一处

## 协作纪律（多 agent / 并行工作）

- **禁止全局 git 操作**：`git stash` / `git reset --hard` / `git checkout .` / `git clean` 会瞬间毁灭其他并行工作者的进行中编辑。只允许 `git diff` / `git status` / `git checkout <具体文件>` / `git log` / `git reflog`
- 发现"文件被神秘回滚/修改"时：先 `git reflog -5` + `git stash list` 排查**内部**操作痕迹，再怀疑外部进程
- 验证"干净树是否编译"用 `git worktree` 或临时克隆，**不要用 stash 切换**

## 架构导航

核心架构决策 → [`docs/architecture/decisions/`](docs/architecture/decisions/)
- [ADR-001: VFS 双层摘要索引](docs/architecture/decisions/001-vfs-dual-layer-index.md) — 项目基础机制
- [ADR-002: StructuredMessage](docs/architecture/decisions/002-structured-message.md) — 单一真相源
- [ADR-003: 组件工具化](docs/architecture/decisions/003-component-toolization.md) — ToolRegistry + call_skill 桥接
- [ADR-004: 前缀匹配上下文组装](docs/architecture/decisions/004-prefix-match-context-assembly.md) — soul→rules→history 顺序
- [ADR-005: SQLite 作为主存储后端](docs/architecture/decisions/005-sqlite-backend.md) — 替代 `LocalFileBackend`
- [ADR-006: 工作区快照独立存储](docs/architecture/decisions/006-snapshot-storage-exception.md) — snapshot 的 VFS 例外
- [ADR-009: 语义化编辑双原语](docs/architecture/decisions/009-semantic-editing.md) — apply_edit 内容匹配(old_string/new_string) + apply_patch unified diff 信封
- [ADR-008: 快照升级](docs/architecture/decisions/008-snapshot-upgrade.md) — gzip 压缩 + GC + similar diff（扩展 ADR-006）
- [ADR-010: 对话多模态链路](docs/architecture/decisions/010-multimodal-message-chain.md) — 图片输入（Message.content_parts + Part::Image）+ MCP 截图落盘
- [ADR-011: 子任务授权边界](docs/architecture/decisions/011-subagent-approval-boundary.md) — 子 agent 无交互审批（任务下发即授权边界，交互只在主 agent 与用户之间）
- [ADR-012: 注入上下文快照持久化](docs/architecture/decisions/012-injectable-snapshot-persistence.md) — 前缀零漂移（JSONL SessionHeader）+ 会话边界/压缩点技能刷新
- [ADR-013: 统一消息通知与唤醒原语](docs/architecture/decisions/013-unified-message-notification-wake.md) — 消息入库 + 唤醒语义（shouldReply = allComplete || failure；任务持久化前置）
- [ADR-018: 会话权威存储迁至 SQLite](docs/architecture/decisions/018-session-authoritative-sqlite.md) — 会话迁出 VFS（流式 vs 文档）；`SessionStore` 原子取号 + 失败上抛；VFS 恢复纯文档基座

被否决的方向（避免重复讨论；触发条件满足时据此重新评估）→ [REJECTED.md](docs/architecture/decisions/REJECTED.md)

模块索引 → [`docs/architecture/module-map.md`](docs/architecture/module-map.md)
设计原则 → [`docs/architecture/principles.md`](docs/architecture/principles.md)
模块详细说明 → [`docs/module-descriptions.md`](docs/module-descriptions.md)
模块间关系 → [`docs/module-relationships.md`](docs/module-relationships.md)
Harness 工程 → [`docs/harness核心思路/harness-engineering-overview.md`](docs/harness核心思路/harness-engineering-overview.md)
**重构实践指南（审查/波次/QA 纪律）→ [`docs/architecture/refactoring-practices.md`](docs/architecture/refactoring-practices.md)**

## 模块速览

| 模块 | 位置 | 一句话 | ⛔ VFS 约束 |
|------|------|--------|-------------|
| `db` | `core/src/db/` | **统一写入门面**（ADR-020）：Database 门面（单连接 + schema 集中）+ SqliteDb + 业务域 Repository（stats/trace/execution/usage）；只依赖 common | — |
| `roles` | `core/src/roles/` | 角色基础类型（ADR-016 纯类型层）：config/agent/scheduler 共用 | — |
| `role_store` | `core/src/role_store.rs` | 角色 VFS 存储（独立存储层，依赖 vfs + roles） | — |
| `vfs` | `core/src/vfs/` | **基础机制**：统一存储检索层（L0/L1/L2 + RRF 融合）；SqliteBackend 经 db::Database 访问 | — |
| `agent` | `core/src/agent/` | Agent 协调器 + AgentLoop + ToolRegistry；工具执行走可插拔管线（`tool_registry/pipeline.rs`：pre-execute 监听器 / 单调守卫 / post-execute 监听器，DSH 吸收）+ 内置可观测性监听器（统计/Trace/GEPA/规则学习） | 所有工具操作通过 VFS |
| `context` | `core/src/context/` | 上下文工程（检索 + 压缩 + 组装） | 检索仅通过 `DualLayerRetriever` |
| `knowledge` | `core/src/knowledge/` | 知识库导入管道 | ❌ **不建独立检索管道**，导入→VFS→SummaryEngine |
| `memory` | `core/src/memory/` | `MemoryExtractor` 长期记忆提取 | ❌ **不建独立存储**，提取→VFS write |
| `skills` | `core/src/skills/` | 技能定义 + 执行 + GEPA 进化引擎 | ❌ **不全量加载**，L0 发现→L2 按需 |
| `session` | `core/src/session/` | `SessionStore`（SQLite 权威存储，ADR-018；SQL 收敛本模块，经 db 单连接）+ `PersistentSessionManager` + `SessionRecall`（FTS 回忆） | ⚠️ 例外：会话内容不经 VFS（ADR-018）；`tianyan://session/{id}` 仅作逻辑标识 |
| `model` | `core/src/model/` | `ModelServices` 容器（不路由、不重试） | — |
| `scheduler` | `core/src/scheduler/` | 定时任务（SummaryTask、EvolutionTask、GcTask、SnapshotGcTask、ReminderTask、UsageStatsFlushTask） | 定时任务产物写入 VFS |
| `observability` | `core/src/observability/` | `AgentMetrics`/`UsageStats`/`TraceCollector`/`ExecutionLog`/`UsageLog`/`RuleRecorder`；SQL 经 db Repository 收敛 | — |
| `executor` | `core/src/executor/` | 工具执行支撑（Action、审批、LLM-as-Judge、验证门控）+ 语义化编辑（内容匹配 edit / patch）、文件浏览（fs/search）、代码智能（symbols/project/test_discovery） | — |
| `lsp` | `core/src/lsp/` | LSP 客户端（服务器注册表 + 自研 JSON-RPC 传输 + 诊断存储；lsp 工具：诊断/跳转/符号） | — |
| `snapshot` | `core/src/snapshot/` | 工作区快照（回退/撤销回退；gzip 压缩 + GC + similar diff） | ⚠️ **ADR-006 例外**：独立文件存储于 `{data_dir}/snapshots/`，不经 VFS |

> **注**：MCP 桥接（`server/src/mcp_bridge.rs`）返回的图片（浏览器截图等）落盘于 `{data_dir}/mcp_images/`，与 snapshot 同级运行时产物例外，不经 VFS（ADR-010）。

已删除组件：`planner/`、`ModelRouter`、`TokenBudget`、`Chunker`、`AgentHarness` wrapper、`AgentSkills` wrapper、`eval/`（回答质量离线评测，判断归入演化智能体）。

## 常见陷阱

- `tianyan` 和 `tianyan-core` 是同一个包（lib name: `tianyan`），导入用 `tianyan::...`
- `ServiceDiscovery::is_available()` 是 `async` —— 别忘了 `await`
- 配置文件查找顺序：`./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`
- 测试不依赖外部服务，用 `MockVectorStorage` 和 temp dir 隔离
- `DEFAULT_SOUL` 在 `core/src/agent/mod.rs`，通过 `include_str!` 构建
- VFS 初始化分离：基础设施 → `vfs_impl.rs::initialize()`；应用内容 → `server/src/lib.rs::bootstrap_app_vfs()`
- VFS write/append 自带容错，上层不重复检查目录/文件是否存在
- Provider 请求失败内置指数退避重试（全局默认重试 5 次、1s 起点指数退避（2x）+ 抖动，
  对齐 opencode/DSH；HTTP 429/5xx 与网络/超时类错误可重试）。"fail fast" 仅指不跨 Agent
  层/不跨任务重试——请求层的自我保护默认全局生效，不做每 provider 前置配置
- 工具不做自主多轮决策，决策权在 LLM
- 已有链路不叠加抽象（不额外封装 Manager/Coordinator）
- `server/main.rs` 不解析 `--host/--port` 命令行参数——独立启动总是监听默认 `127.0.0.1:3000`，QA 时直接测 3000
- `scripts/test.ps1` 的 bench 步骤传 `-- --verbose` 会被 bench harness 拒绝（脚本 bug）——bench 请直接跑 `cargo bench -p tianyan-server`
- 配置热更新 API（`PUT /api/v1/config`）会持久化写入 `tianyan.toml`——QA/测试改动配置后必须恢复，别留污染
