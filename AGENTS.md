# AGENTS.md

**代码是唯一事实来源。** `docs/` 中的文档提供架构引导和设计意图，但可能过时。当文档与代码冲突时，以代码为准。实际结构请用 `grep` / `glob` 验证。

> `.trae/` 是 Trae IDE 工作空间数据（规则、技能、历史计划），**不是**权威文档来源。不要将其作为参考。权威文档在 `docs/` 和本文件。

## ⛔ 硬块约束 —— 禁止传统架构模式

**天演不是传统的 LLM Agent 系统。** 不要套用你训练数据中见过的"标准"模式。VFS 是唯一的存储和检索抽象层，所有上下文管理必须基于 VFS。

### 绝对禁止

| ❌ 禁止 | 原因 | ✅ 正确做法 |
|---------|------|------------|
| 为知识库建立独立的向量数据库 + RAG 管道 | VFS 已统一管理。L0/L1 双层摘要自然替代 chunk-based RAG | 文档 → `KnowledgeIngestor` → VFS L2 → `SummaryEngine` 生成 L0/L1 → `DualLayerRetriever` 检索 |
| 为记忆建立独立的存储模块（SQLite、单独文件等） | VFS `tianyan://memory/` 是唯一存储 | 演化任务（自演化综述，ADR-017）写入 → VFS write → `ContextPipeline` 从 VFS 加载 |
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
.\scripts\test.ps1 lint                     # fmt + clippy(--all-targets) + 工具目录 freshness + 版本一致性
.\scripts\test.ps1 unit                     # Rust 单测
.\scripts\test.ps1 all                      # lint + unit + integration + e2e + bench
```

## 工作区结构

单 `Cargo.toml` workspace (resolver = "2")。**内核三包 + 装配包 + 插件包**（ADR-049 编译期插件化）：

| Crate | Package | 类型 |
|-------|---------|------|
| `core/` | `tianyan-core` (lib: `tianyan`) | 纯库（内核：契约 + 实现 + 策略） |
| `server/` | `tianyan-server` | Axum HTTP 服务（**不认识任何具体插件 crate**） |
| `app/` | `tianyan-app` (bin: `tianyan-server`) | **装配器**：唯一 `use` 插件类型之处，装配清单住这里 |
| `gui-vite/` | - | **已退役（2026-11，冻结中）**：React TypeScript 前端（非 crate）。它不是产品前端——MSI 只装 exe、release 链从不构建它；Rust/CI 耦合已摘除（server 静态资源服务 + SPA 回退删除、CI 前端门禁移除、test.ps1 前端层级移除），目录暂留作参考实现（`npm run dev` 手工仍可用） |
| `mcp/` | `tianyan-mcp` | MCP 协议客户端 |
| `service/config/` | `tianyan-service-config` | **服务层**：配置加载 / 持久化（段保全）/ 数据目录迁移 / 校验编排 / 向导 DTO（ADR-049 期 3-T4，753 生产行从 core 搬出）；**不含配置结构**（`TianyanConfig` 留 core——内核与插件的共同构造参数） |
| `assembly/` | `tianyan-assembly` | **装配清单**：11 个插件工厂 + 两份清单（`full()` = 生产 11 插槽 / `policies()` = 集成测试的 9 插槽前缀）+ 跨插件顺序断言的**单一实现**（ADR-049 期 4）；全仓唯一 `use` 全部插件 crate 之处（改造前同一份清单在 `app` / `gui-native` / `server/tests` / `app/tests` / `app/examples` 共 5 处副本） |
| `tools-ops/` | `tianyan-tools-ops` | **运维工具**（ADR-049 期 4d）：服务停止后直接修数据的终端脚本（`skill_import` / `rebuild_overviews`），**不被任何 crate 依赖**；插件 crate 因此零 dev 边 |
| `desktop/` | `tianyan-desktop` | **桌面启动编排（单 server 发现与复用）**：`ensure_server` / `discover_existing`（扫 `3000..=3100` + 校验 `/health` 的 `service` **与** `data_dir`）+ `find_available_port` / `start_axum_server`；`gui-native/` 共用这一份（`tauri/` **已于 2026-10 删除**）。**端口探测只避免端口冲突，不能避免数据目录双写**——实测撞上过「桌面版占 3000、gui-native 顺延 3001，两个 server 同写 `D:\tianyan`」（定时任务各跑一遍 + SQLite 并发写）。只经 `tianyan-assembly::full()` 取插件清单，**不直接认识插件 crate**（不是第二份装配清单） |
| `plugins/tools-fs/` | `tianyan-tools-fs` | 插件：glob / list_dir / read_file / write_file / apply_edit / apply_patch / **view_image**（定义/门控/审批；实现暂留 core） |
| `plugins/tools-code/` | `tianyan-tools-code` | 插件：symbol_outline / repo_map / grep（**含实现**，tree-sitter ×6 在此） |
| `plugins/tools-todo/` | `tianyan-tools-todo` | 插件：todo / goal（含实现） |
| `plugins/tools-web/` | `tianyan-tools-web` | 插件：web_search / web_fetch（含实现，`scraper` 在此） |
| `plugins/storage-vector/` | `tianyan-storage-vector` | 插件：`LanceDbVectorStore`（`lancedb`/`arrow`/`datafusion` 链只在此） |
| `plugins/storage-sqlite/` | `tianyan-storage-sqlite` | 插件：`SqliteBackend` + `LocalFileBackend`（VFS `StorageBackend` 的两个 adapter；`rusqlite` 仍在 core——本插件收窄的是重编面积，不是依赖面） |
| `plugins/task-builtin/` | `tianyan-task-builtin` | 插件：5 个内置定时任务实现（摘要生成 / 垃圾回收 / 统计刷盘 / 快照 GC / 主动提醒，含各自内联测试） |
| `plugins/task-evolution/` | `tianyan-task-evolution` | 插件：自演化综述任务（ADR-017 三阶段编排，含 29 个内联测试）；契约 `EvolutionReviewExecutor` 留 core，实现 `AgentEvolutionExecutor` 留 server（ADR-049 序 5b） |
| `plugins/policy-approval/` | `tianyan-policy-approval` | 插件：**审批策略**（`ApprovalWorkflow`：风险定级 / 自动规则引擎（Deny 优先）/ `ApprovalMode` 三态短路 / 「总是询问」命令 / 人工审批挂起通道 / 审计，含 23 个迁移测试）；契约 `ApprovalGate` 与全部领域类型留 core（期 3-T1） |
| `plugins/policy-context/` | `tianyan-policy-context` | 插件：**上下文策略**（注入管线 / 双层检索 RRF / 意图分类 / 分层压缩，1,729 生产行 + 65 个迁移测试）；`assembler.rs` 留 core（K1 组装机制）——ADR-049 期 3-T2，**期 3 的主体收益** |
| `plugins/policy-review/` | `tianyan-policy-review` | 插件：**评审策略**（`LlmTaskReviewer` 后台任务结果自审，约 170 生产行 + 9 个迁移测试 + 1 个装配门禁）；契约 `TaskReviewer` / `Judgment` / `Verdict` 留 core（期 3-T3）。技能使用复审（`SkillReviewer`）已于**期 4c 退役**——技能质量判断改由演化智能体依据「技能使用统计 + 用户原话材料 + 技能清单」完成（ADR-049） |
| `plugins/provider-client/` | `tianyan-provider-client` | **provider 基底 crate**（不是能力插件、无插槽）：`AsyncOpenAIClient`（UA 单点 / 方言解析 / 双预算超时 / 动态会话头 / 重试策略一次构造）+ `RetryableFailure` / `classify_http_status`；被下面两个 provider 插件共用（孤儿规则不允许把 `impl EmbeddingService for 外部类型` 写在第三方 crate，故共用的客户端必须独立成 crate） |
| `plugins/provider-llm/` | `tianyan-provider-llm` | 插件：**聊天 + 视觉 provider**（`chat` / `vision` / `discovery` / 日志装饰，约 2,450 行 + 39 个迁移测试）+ 连通性探针（配置页「测试连接」）；`async-openai` 随之离开 core（ADR-049 期 2 序 6） |
| `plugins/provider-embed/` | `tianyan-provider-embed` | 插件：**文本嵌入 provider**（宽容 usage 解析 / 用量上报 / 查询缓存 FIFO 256，约 700 行 + 10 个迁移测试）；用量上报**契约** `EmbeddingUsageSink` 留 core（实现方在通道层）（期 2 序 6） |
| `plugins/tool-pipeline/` | `tianyan-tool-pipeline` | 插件：**工具管线出口侧**——观测监听器（统计 / Trace / GEPA 执行记录 / 失败规则学习，213 生产行）+ 超预算输出落盘（ADR-047，181 生产行）+ 工具目录渲染（96 生产行），394 生产行搬出 core + 14 个迁移测试 + 2 个新增；契约 `ToolPostExecuteListener` / `ToolPipelineWiring` / `ToolPipelineFactory` 留 core（`agent::tool_registry::pipeline`，期 3-T5） |
| `gui-native/` | `tianyan-gui-native` | **GPUI 原生客户端（第二个桌面壳，独立 workspace）**：自带 `[workspace]`，**不进主 workspace**（零脚本改动、零 IDE 索引污染、零 CI 风险；代价 = `core`/`server` 编两份）。是**唯一的产品前端**（`tauri/` **已于 2026-10 删除**、`gui-vite/` **已于 2026-11 退役**），连同一个内嵌 server、走同一套 HTTP/SSE 协议。验证结论与实测数据见 `gui-native/README.md`（含 GPUI 内存对比、IME/Markdown/虚拟列表实测）；打包四节（静默安装 / 同版本升级 / 验收 / 自动更新编排）见 [`docs/operations/release-msi.md`](docs/operations/release-msi.md) |

**依赖铁律**（破一条拆包收益全部回吐）：插件只依赖 `core`（kernel-api）；插件之间零互赖；
`core`/`server` 不认识任何插件 crate。**认识具体插件 crate 的只有三处**：`assembly/`
（清单与其唯一实现，ADR-049 期 4）+ `app/`（装配器，只写启动编排）——`tauri/` 曾是第二个装配器，**已于 2026-10 删除**；
`desktop/`（桌面启动编排）**不在此列**——它只调 `tianyan_assembly::full()`，不直接
`use` 任何插件 crate，因此铁律不因它从三处变四处；
另有一个**终端消费者** `tools-ops/`（运维脚本，依赖插件但**不被任何 crate 依赖**，
与两个装配器的区别是它不提供装配清单，且可在服务未启动时直接改数据——期 4d）。
装配经 `server/src/assembly/` 的类型擦除插槽——**10 个插槽按「变化原因」分 4 组**
（ADR-049 期 4b 分组，理由见该模块 crate 文档；**原 11 个**，期 4c 退役「技能质量评审」
后 `policies` 组少一根 `SkillReviewFactory`——判断改由演化智能体依据技能使用统计 +
用户原话完成）：
`manifests`（`ToolPluginFactory` / `TaskFactory` / `ToolPipelineFactory`——三份有序清单）、
`storage`（`StorageBackendFactory` / `VectorStorageFactory`）、
`policies`（`ApprovalGateFactory` / `ContextFactory` / `TaskReviewerFactory`）、
`providers`（`ProviderLlmFactory` / `ProviderEmbedFactory`）；
`TaskFactory` 由装配层把 `task-builtin::tasks()` 与 `task-evolution::tasks()` 拼接成一份
清单——`splice(1..1, …)` 让演化任务紧跟 `summary_generation`，跨插件顺序只有装配清单知道；
`ApprovalGateFactory` 把 `policy-approval::gate()` 包成 `Arc<dyn ApprovalGate>`——
**未装配 = 不装配审批门控**，工具走「未装配直接放行」分支 + 启动期 `warn!`，
比存储降级（`error!`，「重启即丢数据」）轻一个量级：默认配置本就是 `ApprovalMode::Autonomous`，
且命令黑名单在 `SecurityPolicy` 守卫层另有拦截）；
`ContextFactory` 把 `policy-context::pipeline()` 包成 `Arc<dyn ContextPort>`——
**未装配 = `NullContextPort`**（空可注入上下文 + 永不压缩）+ 启动期 `warn!`，
而 `start_server*` 对它设了**启动期硬前置**（与结构化存储同手法：缺了上下文策略，
agent 没有 system 前缀、上下文只增不减，服务起得来但行为已不成立）。
`TaskReviewerFactory` 把 `policy-review::task_reviewer()` 包成 `Arc<dyn TaskReviewer>`
（插槽是**一次搬移的必要配套**：`LlmTaskReviewer` 的构造点在 core 的
`AgentBuilder::build`，没有注入路径就等于没搬）——**未装配 = 不自审**
（`background_self_review` 为真时也直接投递通知 + `warn!`）。
「是否自审」这个**成本开关**仍由内核判；提示词 / 解析口径 / 截断预算全在插件。
**历史（期 4c 退役）**：期 3-T3 同批装的第二根评审插槽 `SkillReviewFactory`（把
`policy-review::skill_reviewer` 包成 `Arc<dyn SkillReviewPort>`，供技能统计的 `reviews`
字段）**已删除**——它的写侧零生产者（前端只可能读到空数组），而技能质量判断改由演化
智能体依据「技能使用统计 + 用户原话材料 + 技能清单」自主完成。
`ProviderLlmFactory` / `ProviderEmbedFactory` 把 `provider-llm::services()` /
`provider-embed::services()` 包成能力集合（`ProviderLlmServices` / `ProviderEmbedServices`，
**定义在 core** 的 `model::assembly`，三方共用），本库再用它们拼 `ModelServices`
（容器已从 core 搬到 `server/src/model_services.rs`：core 内零生产使用者 + D5 装配职责）——
**未装配 = core 的「未配置」降级服务**（`Unconfigured{Chat,Embedding,Vlm}Service`，
调用即报“应用未配置”）+ `warn!`，服务端照常启动（前端显示配置向导）。
连通性探针（`ConnectionProbe` 契约在 core）随 provider-llm 的能力集合一起产出：
「测试连接」用用户**当场输入**的 endpoint/key 造临时客户端，必须复用同一套构造语义。
`ToolPipelineFactory` 把 `tool-pipeline::pipeline_factory()` 包成
`Arc<dyn Fn(&ToolPipelineWiring) -> Vec<Arc<dyn ToolPostExecuteListener>>>`——
**返回的顺序即注册顺序，顺序即语义**（ADR-047 原文：观测监听器在**前**，数据层须见
原始结果；输出落盘是**改写者**，必须排最后）；顺序在插件里一行可见
（`ToolPipeline::into_listeners` 的 `vec![观测, 落盘]`），并由插件侧行为式测试锁定
（跑完整条管线后看 GEPA 数据层记的是原文还是预览）。
**未装配 = 一个 post-execute 监听器都不注册**（注册表不再自带任何监听器）：大输出不落盘
（只受既有 `executor::truncate` 截断限制）、统计 / Trace / GEPA 记录 / 规则学习全停
+ 装配期 `warn!` 指路——与 ADR-047「落盘失败保持原结果（best effort）」同一档语义，
`create_app`（47 处 lib 单测 + 嵌入式）因此照常起实例。
任务的清单项与能力集合（`TaskRegistration` / `TaskServices`）**定义在 core**
（`core/src/scheduler/task_assembly.rs`，server 侧 re-export）：若住在 server，任务插件
就得依赖 server 库（破第一条铁律）。

## 代码规范

- `unsafe_code = "deny"` — 禁用 unsafe
- Clippy: `unwrap_used`、`expect_used`、`unwrap_in_result` 均为 `warn`
- 公开 API 用 `///` / `//!`，不要用 `//` 行注释
- 所有错误用 `TianyanError`，禁止引入新错误类型。`TianyanError` 仅保留 4 个变体（`Io` / `Json` / `Toml` / `Custom`），模块内部错误通过 `Custom(String)` 传递，调用方在消息中携带"模块前缀：详情"，**严禁新增变体**
- **错误分类用语义谓词**（ADR-014）：构造用 `not_found` / `conflict` / `invalid_input` / `permission` / `timeout`，判定用 `is_not_found` / `is_conflict` / `is_invalid_input` / `is_permission` / `is_timeout`。**禁止在调用方对错误消息做字符串匹配分类**（`starts_with` / `contains` 判定错误类别）——分类契约只在 `core/src/common/error.rs` 一处

## 设计原则

> 只收**不可违反的形状约束**（设计原则的单一出处就是本节）。模块职责 / 关键文件 / 装配细节 → [module-map](docs/architecture/module-map.md)；契约分级 → [contracts](docs/architecture/contracts.md)。

- **Seam 原则**：*One adapter = hypothetical seam. Two adapters = real seam.* 不因「未来可能需要」引入 trait；测试 mock 算 adapter，有生产实现 + 测试 mock 才是真实 seam —— `ChatService`（1 生产 + 5 mock）/ `EmbeddingService` / `VlmService` / `ServiceDiscovery` 均属真实 seam。
- **Trait 实现直接写在 `impl Trait for Struct`**：不定义中间 inherent 方法做委托。
- **泛型边界**：被其他 crate 实例化的类型优先 `Arc<dyn Trait>`（泛型只做模块内部封装；对外暴露先看 trait object 兼容性）。
- **共享基础设施归被依赖方**：跨模块共享的具体类型放**被依赖方 / 叶模块**，消费方经单向 re-export 保留下游路径 —— 先例 `SqliteDb` → `vfs::backend`、`LoggingConfig` / `TokenEstimator` → `common`；目标是**无依赖环**（ADR-007）。
- **层间职责分明**：持久化层只管存，执行层才决定用什么模型执行；中间层既拿不到上游值、也无权读配置时，传 `Option::None` 让下游自己决断。
- **Providers 的 fail fast 是跨层语义**：请求层内置指数退避重试（见「常见陷阱」），「fail fast」仅指不跨 Agent 层 / 不跨任务重试；`RetryService` 已移除，**禁止重新引入**独立的请求重试封装层。
- **初始化分离**：VFS 基础设施 → `vfs/vfs_impl.rs::initialize()`，应用内容（soul.md / learned 目录）→ `server/src/lib.rs::bootstrap_app_vfs()`；`DEFAULT_SOUL` 用 `include_str!`（见「常见陷阱」两条）。

## 文档写作层次

**对外文档 = 完整成果；缘由与代价 = ADR / CHANGELOG / 对话回复。** 不要混层。

- `README.md`、`docs/` 面向使用者的部分只写**是什么、能做什么、怎么用、结构长相**——读它的人要拿走的是成果。
- 选型缘由、方案对比、取舍与代价（"代价（诚实记账）""为什么不选 X""比 A 慢但…"）、自我辩护式说明，**一律不写进对外文档**：写进 ADR（决策记录本就承担取舍）、`CHANGELOG.md`（变更证据）或与会话中的口头解释（用户原话："咱们两个交互的时候，有时候要交代清楚缘由。但是对外的、统一的文档，是完整的成果，不要写那么多防御性描述"）。
- 事实性数字（内存实测、包体大小、耗时）可以留在对外文档——它们是成果的一部分，**对比列、"已用实测数据换来的确定成本"这类论证框架不要**。
- 反面样例（2026-11 用户指出）：根 README 曾有一节「为什么是「前后端同栈」」，用「典型 Electron/Tauri 应用 vs 天演」对比表 + 「代价（诚实记账）：GPUI 生态年轻…」段。正确形态是一节「全栈 Rust」：分层表 + 实测数字 + 同栈带来的 4 条事实，不含对比列与代价段。

## 协作纪律（多 agent / 并行工作）

- **禁止全局 git 操作**：`git stash` / `git reset --hard` / `git checkout .` / `git clean` 会瞬间毁灭其他并行工作者的进行中编辑。只允许 `git diff` / `git status` / `git checkout <具体文件>` / `git log` / `git reflog`
- **禁止按进程名批量杀进程**：`Get-Process tianyan-gui-native | Stop-Process -Force`（以及任何按名字过滤的 `taskkill /IM`、`killall`）会连同**用户正在用的桌面客户端**一起杀掉；更糟的是 agent 自己就跑在那个 server 上——实测 2026-11-02 两次「命令被中断」正是自伤（把宿主连同用户界面一起杀了）。杀进程**只按 PID**，且只杀**本会话自己启动并记录过**的 PID（后台命令用 `background:true` 时返回的 `pid`）。需要「让 exe 不被占用才能重编译」时，先确认那个 PID 是自己起的，再 `Stop-Process -Id <pid>`；判断「某端口有没有服务」不要只看 `Get-NetTCPConnection`（可能取不到数据而误报「无人监听」），用 `/health` 直连确认
- 发现"文件被神秘回滚/修改"时：先 `git reflog -5` + `git stash list` 排查**内部**操作痕迹，再怀疑外部进程
- 验证"干净树是否编译"用 `git worktree` 或临时克隆，**不要用 stash 切换**
- **先扫依赖库有什么，再从原语搭**：`gpui_kit` 是门面 crate，`gpui_kit::component` 就是
  `gpui_component` **整棵组件树**（`avatar` / `button` / `carousel` / `chart` / `dialog` /
  `dock` / `form` / `input` / `list` / `menu` / `searchable_list` / `sidebar` / `stepper` /
  `tab` / `table` / `text` / **`message_scroller`** …）。做新页面 / 新组件前先扫一遍：读
  `~/.cargo/registry/src/*/gpui-kit-*/src/lib.rs` 的 re-export 表，或直接
  `ls ~/.cargo/registry/src/*/gpui-component-*/src/`。**实测代价**：消息列表从 `gpui::list`
  裸原语自己搭，为「上滚跳项」和「切会话不保位」连修五轮，最后落到的方案
  （`ListAlignment::Top` + `measure_all` + 锚点 `(item_ix, offset_in_item)` 恢复）
  与依赖里现成的 `gpui_component::message_scroller` 内核一致——它一直可用，只是没人翻过那层目录

## 架构导航

**完整文档地图 → [`docs/README.md`](docs/README.md)**（按「我要做什么」索引）。这里只留最常查的。

核心架构决策 → [`docs/architecture/decisions/`](docs/architecture/decisions/)（54 份，全量索引见该目录 `README.md`）。最常查的九份：

- [ADR-001: VFS 双层摘要索引](docs/architecture/decisions/001-vfs-dual-layer-index.md) — 项目基础机制（**检索两铁律**见「模块速览」下方注）
- [ADR-018: 会话权威存储迁至 SQLite](docs/architecture/decisions/018-session-authoritative-sqlite.md) — 会话不经 VFS 的唯一例外
- [ADR-023: 配置目录与数据目录分离](docs/architecture/decisions/023-config-data-dir-separation.md) — 数据目录**只读**、搬迁是唯一写入通道（§4）
- [ADR-027: 会话时序链模型](docs/architecture/decisions/027-session-timeline-chain.md) — 无分支链；截断只发生在组装层
- [ADR-030: 统一 Agent 循环框架](docs/architecture/decisions/030-unified-agent-loop.md) — 主 agent 与子代理同构
- [ADR-035: 会话工作集](docs/architecture/decisions/035-session-working-set.md) — 物化上下文缓存，非第二权威
- [ADR-040: 会话排他写事务](docs/architecture/decisions/040-session-exclusive-write-transaction.md) — 单写者；回退全或无
- [ADR-049: 插件化边界与装配模型](docs/architecture/decisions/049-microkernel-plugin-boundaries.md) — 微内核 + 三条依赖铁律
- [ADR-052: GUI 自动化端口](docs/architecture/decisions/052-gui-automation-port.md) — `/state` + `/act`；动作必须复用真实点击路径

被否决的方向（避免重复讨论；触发条件满足时据此重新评估）→ [REJECTED.md](docs/architecture/decisions/REJECTED.md)

专题文档（机制级）→ [`context-pipeline`](docs/architecture/context-pipeline.md)（上下文组装+压缩）· [`event-protocol`](docs/architecture/event-protocol.md)（事件/SSE 协议）· [`model-provider-notes`](docs/architecture/model-provider-notes.md)（reasoning 回传+缓存）· [`task-runtime`](docs/architecture/task-runtime.md)（三类任务/并发/取消）· [`operations/`](docs/operations/)（troubleshooting / data-health-check / release-msi / ci-pipeline）

模块索引 → [`docs/architecture/module-map.md`](docs/architecture/module-map.md) — **唯一的模块文档**（旧的 `module-descriptions` / `module-relationships` 已归档到 `docs/archive/`，不再维护）
设计原则（不可违反的形状约束）→ 本文「## 设计原则」一节
Harness 工程（外部资料整理，非天演机制）→ [`docs/reference/harness-engineering-overview.md`](docs/reference/harness-engineering-overview.md)
重构实践（审查 / 波次 / QA 纪律）→ [`docs/development.md`](docs/development.md) 的「重构实践」章
已知限制（用户可见的长期形状约束）→ [`docs/known-limits.md`](docs/known-limits.md)
契约分级台账（S / A / B，**改契约前必读**）→ [`docs/architecture/contracts.md`](docs/architecture/contracts.md)

## 模块速览

> 每个模块的详细职责、关键文件、装配细节见 [`docs/architecture/module-map.md`](docs/architecture/module-map.md)（唯一的模块文档）。本表只给「有这个模块、它大概干什么、它受什么约束」。

| 模块 | 位置 | 一句话 | ⛔ VFS 约束 |
|------|------|--------|-------------|
| `common` | `core/src/common/` | 通用类型 / 错误（`TianyanError` 4 变体 + 语义谓词）/ 日志 / token 估算 / 截断 / 摘要 | — |
| `config` | `core/src/config/` | 配置结构与全局槽位（`get_config`）；加载 / 持久化 / 迁移 / 校验在 `service/config` | — |
| `db` | `core/src/db/` | 统一写入门面（ADR-020）：单连接 + schema 集中 + 业务域 Repository；只依赖 `common` | — |
| `roles` | `core/src/roles/` | 角色基础类型（ADR-016 纯类型层） | — |
| `role_store` | `core/src/role_store.rs` | 角色 VFS 存储（依赖 vfs + roles） | — |
| `vfs` | `core/src/vfs/` | 统一存储检索层（L0/L1/L2 + RRF）；只留 `StorageBackend` 契约 + `UriMapper`，后端实现在 `plugins/storage-sqlite` / `storage-vector` | — |
| `agent` | `core/src/agent/` | Agent 循环 / 工具注册表 / 可插拔工具管线 / 后台委托（ADR-026 异步 + 双信号量、ADR-030 同构、ADR-040 排他写）；监听器在 `plugins/tool-pipeline` | 所有工具操作通过 VFS |
| `context` | `core/src/context/` | 上下文契约 + 组装机制（`ContextAssembler` / `ContextPort`）；策略实现在 `plugins/policy-context` | 检索仅通过 `DualLayerRetriever`（在 `tianyan_policy_context`） |
| `knowledge` | `core/src/knowledge/` | 知识库导入管道 | ❌ **不建独立检索管道**，导入→VFS→SummaryEngine |
| `skills` | `core/src/skills/` | 技能 = VFS 方法论文档（发现 / 读取，**无执行语义**） | ❌ **不全量加载**，L0 发现→L2 按需 |
| `session` | `core/src/session/` | 会话存储与回忆（`SessionStore` + FTS）；SQL 收敛本模块 | ⚠️ 例外：会话内容不经 VFS（ADR-018）；`tianyan://session/{id}` 仅作逻辑标识 |
| `model` | `core/src/model/` | 模型服务契约 + 载荷 + 装配原语；实现全在 provider 三 crate | — |
| `scheduler` | `core/src/scheduler/` | 定时任务机制 + 装配原语；具体任务全在 task 插件 | 定时任务产物写入 VFS |
| `observability` | `core/src/observability/` | 指标 / 用量 / Trace / 执行日志（SQL 经 db Repository） | — |
| `executor` | `core/src/executor/` | 工具执行支撑 + 语义化编辑 + 文件浏览 + 代码智能 + 图片冻结副本；审批 / 自审契约留 core | — |
| `snapshot` | `core/src/snapshot/` | 工作区快照（回退 / 重做 / gzip / GC / diff） | ⚠️ **ADR-006 例外**：独立文件存储于 `{data_dir}/snapshots/`，不经 VFS |
| `events` | `core/src/events/` | 事件触发（文件监听 + webhook → 总线 → 规则动作） | — |
| `goals` | `core/src/goals/` | 长期目标 + 进度跟踪（会话绑定） | ⚠️ 例外：运行期结构化产物 |
| `todos` | `core/src/todos/` | 待办清单（会话绑定） | ⚠️ 例外：运行期结构化产物 |
| `notification` | `core/src/notification.rs` | 通知通道抽象（`NotificationSink`；桌面实现在 gui-native） | — |

> **注（VFS 检索两铁律，ADR-001「对子系统的约束」）**：① 分数 = **余弦相似度**（查询侧显式 `DistanceType::Cosine`；默认 L2 会让 `1 - distance` 变成 `2cos-1`，0.6 阈值实际要求余弦 0.8 ⇒ 永远只加载 L0）；② 命名空间过滤**必须下推 DB 层**（`only_if`），取回 top-k 再内存筛会因全局 top-k 被占满而归零。
>
> **注**：MCP 桥接（`server/src/mcp_bridge.rs`）返回的图片（浏览器截图等）落盘于 `{data_dir}/mcp_images/`，与 snapshot 同级运行时产物例外，不经 VFS（ADR-010）。
>
> **注（工具图片冻结副本，ADR-050）**：`view_image` 读取的图片复制到 `{data_dir}/session_images/{sha256}.{ext}`（内容寻址、同图去重、不清理）；tool 结果只存引用（`session_images/<sha>.png`），组装期按引用加载为 data URL 随该条 tool 消息发给模型。与 `tool_output/` 同类运行期产物，不经 VFS。
>
> **注（运行期结构化产物，ADR-022/024）**：`goals.json`/`todos.json`（会话任务与目标）、
> `scheduled_agent_tasks.json`/`scheduler_state.json`（调度器状态）为**运行期状态文件**
> （非知识/记忆/技能内容），不经 VFS——与快照、MCP 图片同类的结构性例外。

已删除组件：`planner/`、`ModelRouter`、`TokenBudget`、`Chunker`、`AgentHarness` wrapper、`AgentSkills` wrapper、`eval/`（回答质量离线评测，判断归入演化智能体）、`memory/`（MemoryExtractor 提取器——ADR-017 收敛后残骸清理）。

## 常见陷阱

- `tianyan` 和 `tianyan-core` 是同一个包（lib name: `tianyan`），导入用 `tianyan::...`
- **`plugins/storage-sqlite/src/{sqlite,local}.rs`、`plugins/policy-approval/src/workflow.rs`、`plugins/policy-context/src/context/**` 内禁止 `crate::…`**（只允许 `tianyan::…`；`policy-context` 额外允许 `crate::context::…`——它自己的模块树，详见其 crate 文档）：core 单测以 `#[cfg(test)] #[path]` **同源包含**这些文件（dev-dep 会让 core 编译两份 → `StorageBackend`/`ApprovalGate`/`ContextPort` 等 trait 出现两个不兼容实例，实测 22 处 E0308/E0277）。写 `crate::` 时它解析到 core 而非插件，报错位置完全不相干。改文件布局须同步 `core/src/vfs/backend/mod.rs`、`core/src/executor/approval/mod.rs` 与 `core/src/context/mod.rs` 的 `#[path]`
  （provider 三件套**不在此列**：它们的测试随文件整体迁移，core 里没有残留的单测需要同源包含——见 ADR-049 期 2 序 6；`plugins/tool-pipeline/src/**` 同此：观测/落盘/目录的测试全部随迁，core 无 `#[path]` 引用）
- **dev-dependency 环是允许的，别和上一条混为一谈**：`server --dev--> tianyan-assembly --> server`（集成测试用 `tianyan_assembly::policies()`，而 assembly 依赖 server 取 `AssemblyServices` 与 10 个插槽类型别名）。Cargo 允许——dev-dep 不参与**构建**图的环检测，测试目标的依赖图里 server lib 只编一份。与 `#[path]` 同源包含的区别：那里同一批源文件被编进**两个** crate（trait 出现两个不兼容实例），这里是两个独立 crate 的正常依赖。代价：`cargo test -p tianyan-server` 会连带编译 LanceDB 链（首次变慢，换来「9 个工厂只有一份实现」）
- **provider 域是三个 crate，不是两个**：`tianyan-provider-client`（共享客户端 + 状态码语义分类，无插槽）← `tianyan-provider-llm` / `tianyan-provider-embed`（能力插件，互相零依赖）。原因是孤儿规则：`impl EmbeddingService for AsyncOpenAIClient` 只能写在定义该类型的 crate 里，而客户端被聊天/嵌入共用，构造语义（UA/方言/双预算超时/动态会话头/重试）必须只有一份。两个能力插件各用一个 **newtype + `Deref`**（`ChatClient` / `EmbeddingClient`）满足孤儿规则——`Deref` 让字段与方法访问**逐字保持**（`self.base_url` / `self.send_chat_request(...)` 不改），改动只有类型名
- **插件测试要用 core 的测试替身（`MockVfs` / `MockChatService`）时开 `test-utils` 特性**：`core/src/test_utils` 与 `model::MockChatService` 的 `cfg` 是 `any(test, feature = "test-utils")`，插件侧写 `[dev-dependencies] tianyan = { path = "../../core", package = "tianyan-core", features = ["test-utils"] }`（`mockall` 因此是 core 的**可选**依赖，默认构建不含）。不这么做只能把替身复制进插件——两份实现同时漂移，测试随即失去意义（ADR-049 序 5a）
- `ServiceDiscovery::is_available()` 是 `async` —— 别忘了 `await`
- 配置文件固定为 `~/.tianyan/tianyan.toml`（`TIANYAN_CONFIG` 环境变量可覆盖，e2e 用）；数据目录由配置 `storage.data_dir` 决定（默认 `%LOCALAPPDATA%/tianyan`），两者分离、搬迁只动数据（ADR-023）
- 测试不依赖外部服务，用 `MockVectorStorage` 和 temp dir 隔离
- `DEFAULT_SOUL` 在 `core/src/agent/mod.rs`，通过 `include_str!` 构建
- VFS 初始化分离：基础设施 → `vfs_impl.rs::initialize()`；应用内容 → `server/src/lib.rs::bootstrap_app_vfs()`
- VFS write/append 自带容错，上层不重复检查目录/文件是否存在
- Provider 请求失败内置指数退避重试（全局默认重试 5 次、1s 起点指数退避（2x）+ 抖动，
  对齐 opencode/DSH；HTTP 429/5xx 与网络/超时类错误可重试）。"fail fast" 仅指不跨 Agent
  层/不跨任务重试——请求层的自我保护默认全局生效，不做每 provider 前置配置
- 工具不做自主多轮决策，决策权在 LLM
- 已有链路不叠加抽象（不额外封装 Manager/Coordinator）
- `server/main.rs` 不解析 `--host/--port` 命令行参数——独立启动默认监听 `127.0.0.1:3000`（`TIANYAN_PORT` 可覆盖，走 `ServerConfig::from_env`），QA 时直接测 3000
- **手工跑 Web 端 playwright 时后端仍须避开 3000**（gui-vite 已退役、e2e 不再进 CI，此条仅手工需要）：`gui-vite/playwright.config.ts` 的 `E2E_PORT`（默认 3099，`TIANYAN_E2E_PORT` 可覆盖），vite 经 `TIANYAN_API_TARGET` 代理到该端口——本地 3000 常被运行中的桌面应用（`tianyan-gui-native`）占用，若共用则「跑 e2e」与「用应用」互斥（后端条目 `reuseExistingServer: false` 会因端口冲突响亮失败）
- 配置热更新 API（`PUT /api/v1/config`）会持久化写入 `tianyan.toml`——QA/测试改动配置后必须恢复，别留污染
- **PowerShell + cargo 陷阱（会伪造"编译失败"）**：PowerShell 5.1 下 `cargo ... 2>&1 | Select-String` / `Out-String` 会把 cargo 写到 **stderr** 的内容（warning 列表、`Blocking waiting for file lock`）当作错误抛出（`NativeCommandError`，exit 1）——即使编译实际成功。判据是输出里是否出现 `Finished` / `test result:`。脚本内用 `$ErrorActionPreference="Continue"`，交互排查可用 `cmd /c "cargo ..."`
- 事件总线为**有界通道**（4096）：订阅者（事件处理器/SSE）积压时丢弃事件并计数（`EventBus::dropped_events()`）——实时通知尽力而为，权威数据在 SQLite/日志文件；不要把"事件必达"当作完整性契约
- **改了工具描述 / 参数 schema 必须重生成工具目录**：`core/src/agent/tool_registry/builtin_tools.rs` 的 `def_*` 描述或 `*Params` schema 一变，`docs/architecture/tool-catalog.md` 即漂移——tag 推送时 Quality 的 `Tool catalog freshness` 门禁必红，而本地只有 `scripts/test.ps1 lint` 覆盖它（手工跑 `cargo fmt` / `clippy` 不会，极易漏到发布阶段，0.5.7 实发事故）。改完跑 `.\scripts\gen-tool-catalog.ps1` 重生成，并以 **`.\scripts\test.ps1 lint` 作为验收入口**（本地统一门禁，勿用手工命令替代）。**命令单一来源**：命令本体在 `.cargo/config.toml` 的 alias（`cargo catalog-check` / `cargo catalog-gen`），CI 步骤 / 脚本 / 文档均调别名（勿在任何一处内联 `cargo run -p ... --example tool_catalog` 命令行——0.5.9 实发事故：生成器 core→server 迁移后 CI 内联的旧命令未同步，tag 推送后门禁红、不出包）
；落盘失败保持原结果（best effort）；目录 `{data_dir}/tool_output/`
- [ADR-048: 会话内 prune](docs/architecture/decisions/048-session-internal-prune.md) — ❌ **已回退**（实施当日）：事后裁剪已落库的工具输出必然导致前缀缓存分叉（改历史 = 从该位置起末尾全部重算），且会在"本不会压缩"的会话里凭空引入分叉 —— 控制点应在**写入侧**（ADR-047）而非组装侧；否决理由见 [REJECTED.md](docs/architecture/decisions/REJECTED.md) #22
- [ADR-049: 插件化边界与装配模型](docs/architecture/decisions/049-microkernel-plugin-boundaries.md) — 微内核化，**编译期插件化**（不做 dylib / WASM / 运行时 eval，维持 REJECTED #6 插件市场否决）：三条依赖铁律（插件只依赖 kernel-api / 插件零互赖 / 内核不认识插件）+ S·A·B 契约分级 + 内核四件套 K1~K4（**19.4K 现实值**，非设计稿 15K）+ **装配器二分**（thin-channel + assembly-manifest；实测 server 11.8K 源码不薄则降幅从 95% 掉到 76%）+ 装配清单**有序**（前缀缓存生命线）+ **期 4 清单收敛**（`assembly/`：11 个插件工厂的唯一实现 + `full()` / `policies()` 双清单 + 4 条跨插件顺序断言）+ 复用既有 `DynamicToolExecutor` 不新建 Plugin 体系；**6 期 + 6 道数据闸门**（判据 = 实测 56,541 源码行，非设计稿 33K）；VFS/ADR-001/018/030/035/038/047 对账明确不动存储边界
