# 契约分级台账（ADR-049 D3 落地）

**日期**: 2026-10-20
**状态**: 期 0 交付物（分级已定，**尚未冻结** —— 冻结时点 = 期 3 结束）
**用途**: 回答三个问题 —— 这个 trait 归哪一级、改它会影响谁、谁来保证它不被乱改。

---

## 一、为什么要分级

Rust 下「契约 = 成本」：契约变更会穿透依赖图，改动成本随消费者数量放大。
把「想改就改」的自由让渡给插件内部，把「不许乱动」的约束集中到最薄的一层 —— 这是让自由与稳定共存的最小代价。

| 级别 | 含义 | 变更纪律 | 谁能改 |
|---|---|---|---|
| **S** | 全部插件依赖的「世界形状」 | **冻结**：只能新增变体 + 弃用周期；新增 trait 方法必须有默认实现 | 需 ADR 级别的评审 |
| **A** | 装配原语 + 任务/通道契约（装配器与少量插件消费） | 谨慎演进：先加**可选参数**再加门；新增方法须有默认实现 | 常规评审 |
| **B** | 实现细节 / 单一消费者 / 插件内部契约 | **自由**：改动不外溢 | 直接改 |

---

## 二、34 个现存 trait 的归属与分级

统计口径：`pub trait` 静态扫描（`core/src` + `server/src`，2026-10-20 / HEAD `8109428`）。

### S 级 · 冻结（14 个）

| # | trait | 位置 | 目标归属 | 主要消费者 |
|---|---|---|---|---|
| 1 | `DynamicToolExecutor` | `core/src/agent/tool_registry/mod.rs:150` | kernel-api | 全部工具插件（内置 30 + MCP 动态工具） |
| 2 | `ToolPreExecuteListener` | `core/src/agent/tool_registry/pipeline.rs:63` | kernel-api | 可观测性 / spill / 策略插件 |
| 3 | `ToolGuard` | `core/src/agent/tool_registry/pipeline.rs:83` | kernel-api | 审批/安全策略插件 |
| 4 | `ToolPostExecuteListener` | `core/src/agent/tool_registry/pipeline.rs:93` | kernel-api | spill（ADR-047）/ 统计 |
| 5 | `VfsCore` | `core/src/vfs/traits.rs:26` | kernel-api | 所有读写 VFS 的插件 |
| 6 | `ContentStore` | `core/src/vfs/traits.rs:64` | kernel-api | VFS 后端 / 摘要 |
| 7 | `VfsSearch` | `core/src/vfs/traits.rs:95` | kernel-api | 检索类工具 / 上下文策略 |
| 8 | `VirtualFileSystem` | `core/src/vfs/traits.rs:139` | kernel-api | 工具 / 记忆 / 技能 / 知识 |
| 9 | `StorageBackend` | `core/src/vfs/backend/mod.rs:19` | kernel-api | `storage-sqlite` / `storage-local` |
| 10 | `VectorStorage` | `core/src/vfs/vector/traits.rs:12` | kernel-api | `storage-vector`（唯一 lancedb 消费者） |
| 11 | `ChatService` | `core/src/model/traits.rs:19` | kernel-api | 循环（K1）+ 全部依赖模型的插件 |
| 12 | `EmbeddingService` | `core/src/model/traits.rs:88` | kernel-api | 向量化 / 检索 |
| 13 | `VlmService` | `core/src/model/traits.rs:158` | kernel-api | 图像理解（多模态链路） |
| 14 | `SessionManager` | `core/src/session/manager.rs:19` | kernel-api | 循环（每轮读写会话） |

### A 级 · 谨慎演进（11 个）

| # | trait | 位置 | 目标归属 | 说明 |
|---|---|---|---|---|
| 15 | `AgentCoordinator` | `core/src/agent/coordinator.rs:94` | kernel-api（通道端口） | 装配器用它跑一轮；thin-channel 的唯一 agent 入口 |
| 16 | `StreamEventDeliver` | `core/src/agent/stream_forward.rs:55` | kernel-api（通道端口） | 流式事件投递目标（SSE / Tauri / 测试） |
| 17 | `TaskEventSink` | `core/src/agent/background.rs:65` | kernel-api | 后台任务事件出口 |
| 18 | `TaskNotifier` | `core/src/agent/background.rs:237` | kernel-api | 任务完成通知 |
| 19 | `TaskWaker` | `core/src/agent/background.rs:250` | kernel-api | 唤醒轮触发 |
| 20 | `TaskHandler` | `core/src/scheduler/task_scheduler.rs:114` | kernel-api | 全部任务插件（6 个 `task-*`） |
| 21 | `TaskRegistrar` | `server/src/scheduled_tasks/manager.rs:71` | assembly-manifest | 定时任务注册 |
| 22 | `TaskResultSink` | `server/src/scheduled_tasks/manager.rs:33` | assembly-manifest | 任务结果回写 |
| 23 | `TaskTrigger` | `server/src/api/events/processor.rs:20` | assembly-manifest | 事件规则触发任务 |
| 24 | `NotificationSink` | `core/src/notification/mod.rs:11` | kernel-api | 桌面通知（tauri 注入） |
| 25 | `DocumentParser` | `core/src/knowledge/parser.rs:61` | plugins/tools-knowledge | 知识导入（PDF/DOCX/XLSX） |

### B 级 · 自由（12 个）

| # | trait | 位置 | 理由 |
|---|---|---|---|
| 26 | `CommandNotifier` | `core/src/executor/command.rs:60` | 命令执行插件内部 |
| 27 | `CommandWaker` | `core/src/executor/command.rs:72` | 同上 |
| 28 | `CommandEventSink` | `core/src/executor/command.rs:82` | 同上 |
| 29 | `ApprovalPendingNotifier` | `plugins/policy-approval/src/workflow.rs:33`（随实现迁出 core，ADR-049 期 3-T1） | 审批策略插件内部 |
| 30 | `TaskReviewer` | `core/src/executor/review.rs`（模块原名 `judge.rs`，ADR-049 期 3-T3 实现迁出后改名） | 评审策略插件内部（单消费者：子代理自审）；实现 `LlmTaskReviewer` 在 `plugins/policy-review`，装配层经 `TaskReviewerFactory` 注入 |
| 31 | `EvolutionReviewExecutor` | `core/src/scheduler/evolution_executor.rs` | 演化任务插件的唯一外部能力（ADR-049 期 2 序 5b 已提入 core：实现 `AgentEvolutionExecutor` 在 server，经 `TaskServices.evolution_executor` 注入——若留在插件里，server 就得依赖插件，破 D2-1） |
| 32 | `EmbeddingUsageSink` | `core/src/model/usage.rs`（ADR-049 期 2 序 6 从 `model/provider/middleware.rs` 提到 `model` 根） | 嵌入用量回传：**实现方是通道层**（`UsageLogEmbeddingSink` 写 `usage_logs`）、**消费方是插件**（`plugins/provider-embed` 的 `LoggedEmbeddingService`）——契约必须住在两者之上的 core，否则通道层要 `use tianyan_provider_embed::…`（破 D2-1） |
| 33 | `ServiceDiscovery` | `core/src/model/traits.rs:73` | 适配器内部（模型列表探测） |
| 34 | `SummaryService` | `core/src/vfs/summary/engine.rs:51` | 摘要策略插件内部 |
| 35 | `ApprovalGate` | `core/src/executor/approval/gate.rs:44` | 审批门控 seam（**ADR-049 期 3-T1 新增**）：内核只认 `Arc<dyn ApprovalGate>`，实现在 `plugins/policy-approval`。按 B 级管理的理由——只有一个消费者面（内核的 4 个审批调用点）与一个实现（`ApprovalWorkflow`），属「插件自有的小契约」而非跨插件公共协议；方法集按**调用点证据**最小化到 4 个（`authorize` / `status` / `respond_to_approval` / `record_user_confirmation_by_fingerprint`），冻结时点（期 3 结束）再评估升不升 A 级 |
| 36 | `ContextPort` | `core/src/context/port.rs` | 上下文工程 seam（**ADR-049 期 3-T2 新增**）：内核只认 `Arc<dyn ContextPort>`，实现在 `plugins/policy-context`（`ContextPipeline`）。按 B 级管理的理由——一个消费者面（内核 3 个调用点：`agent_core.rs:478` 加载注入 / `:783` 会话压缩 / `:751` 压缩点摘要提取）+ 一个实现，属「插件自有的小契约」；方法集按调用点证据最小化到 3 个，**没有** `run()` / `compress_if_needed()` / `compressor()` 之类无生产调用点的方法。**顺序契约 `compress_for_session` → 落库成功 → `ws.reshape_after_compression()`（ADR-035 §2）留在内核执行**，不进本 trait——插件只产出压缩结果。未装配时内核用 `NullContextPort` 兜底（`start_server*` 另有启动期硬守卫）。冻结时点（期 3 结束）再评估升不升 A 级 |
| ~~37~~ | ~~`SkillReviewPort`~~ | — | **已退役（ADR-049 期 4c）**：技能质量判断改由**演化智能体**依据「技能使用统计（近 30 天调用次数 / 处理异常 / 均耗时 / 最后调用）+ 用户原话材料 + 技能清单」承担；契约 `SkillReviewPort` / `SkillReview`、实现 `SkillReviewer`（`review_conversation`）与插槽 `SkillReviewFactory` 一并删除，`/api/v1/skills/stats` 的 `reviews` 字段与前端两处渲染同步下线。**编号留空不重排**（重排会牵动全表引用）。`REVIEWS_PREFIX` 常量保留在 `core/src/skills/mod.rs`（历史 `_reviews` 目录仍须排除在技能列表外）。原为 B 级（期 3-T3 新增，1 个消费者 + 1 个实现） |
| 38 | `ConnectionProbe` | `core/src/model/assembly.rs` | 连通性探测 seam（**ADR-049 期 2 序 6 新增，期 2 唯一新增 trait**）：通道层只认 `Arc<dyn ConnectionProbe>`，实现 `ProviderConnectionProbe` 在 `plugins/provider-llm`，随 `ProviderLlmServices::probe` 由工厂产出。方法集 **1 个**（`probe(endpoint, api_key, model) -> Result<Vec<String>>`）。**为什么不给探针单开插槽**：探测用用户当场输入的 endpoint/key 造**临时**客户端，必须复用插件那套构造语义（UA 单点 / 方言解析 / 双预算超时 / 动态会话头 / 重试策略）；独立插槽会让装配层多一个必填项，或让第二个工厂复制这套语义。**错误分类不进契约**：`is_permission` / `is_not_found` / `is_timeout` → 用户文案仍是通道层的事（ADR-014 消费端）。未装配 provider 工厂时探针为 `None` → `POST /api/config/test-connection` 返回「连接测试不可用」 |

> 小计：S 14 / A 11 / **B 12** = **37**（含期 3-T1 新增的 `ApprovalGate`、期 3-T2 新增的
> `ContextPort`、期 2 序 6 补做的 `ConnectionProbe`；期 3-T3 新增的 `SkillReviewPort`
> 已于期 4c 退役，编号留空不重排）。
> 反指标「每期净增 pub trait ≤ 3」：期 3 净增 **3**（T1 / T2 / T3 各一——正好用满），
> 其中 T3 那根已于期 4c 退役；期 2 序 6（补做）净增 **1**。期 4 起不得再增。

---

## 三、收敛映射：34 → 24（期 0.5 / 1 执行）

目标是"减少跨 crate 的契约数量"，不是为了好看。每一条合并都必须有真实理由。

| 合并 | 从 | 到 | 理由 |
|---|---|---|---|
| 1 | `CommandNotifier` + `CommandWaker` + `CommandEventSink` | `CommandChannel` | 三者都是"命令执行 ↔ 外界"的同一根管子，永远一起注入、一起替换 |
| 2 | `TaskNotifier` + `TaskWaker` | `TaskWake` | 唤醒轮语义：通知与唤醒是同一动作的两面，拆开会导致一个实现两处注入 |
| 3 | `ToolPreExecuteListener` + `ToolPostExecuteListener` | `ToolListener`（带 `Phase` 参数） | 管线是同一个 pre→post 生命周期，两个 trait 让"只想看一侧"的插件也必须实现另一半 |
| 4 | `TaskRegistrar` + `TaskResultSink` | `TaskRegistryPort` | 装配期注册与运行期回写是同一端口的两个方法 |
| 5 | `AgentCoordinator` + `StreamEventDeliver` | `RuntimePort` | 通道只需要"跑一轮 + 收事件"，两者是同一调用面的两半 |
| 6 | `EmbeddingUsageSink` | 并入 `EmbeddingService` 的可选钩子 | 单方法 trait，不值得独立存在 |
| 7 | `TaskReviewer` + `EvolutionReviewExecutor` | `ReviewStrategy` | 都是"让另一个模型评审产物"，同一策略面 |
| 8 | `ServiceDiscovery` | 并入 `ModelPort`（模型列表探测） | 单方法 + 单消费者 |
| 9 | `ApprovalPendingNotifier` | 并入 `NotificationSink`（带 kind） | 同一个通知面 |
| 10 | `SummaryService` | 并入 `ContentStore` 的 `summarize` 扩展点 | 摘要只对内容存储有意义，独立 trait 会让两处必须同时注入 |

**结果**：34 → **24 个**（S 14 保持不变，A 11 → 5，B 9 → 5）。

> 收敛不是必须一次做完：期 0.5 只做与工具/装配相关的第 1–5 条，其余在期 2/3 随插件迁移顺带完成。

---

## 四、S 级冻结的机械守护

### 4.1 冻结文件清单

```
core/src/common/types/          # URI / StructuredMessage / Part / ContentLevel
core/src/common/error.rs        # TianyanError + 语义谓词
core/src/vfs/traits.rs          # VfsCore / ContentStore / VfsSearch / VirtualFileSystem
core/src/vfs/backend/mod.rs     # StorageBackend
core/src/vfs/vector/traits.rs   # VectorStorage
core/src/model/traits.rs        # ChatService / EmbeddingService / VlmService
core/src/session/manager.rs     # SessionManager
core/src/agent/tool_registry/pipeline.rs   # ToolListener / ToolGuard
core/src/agent/tool_registry/mod.rs        # DynamicToolExecutor（仅 trait 定义段）
```

### 4.2 规则（期 0 起生效，期 3 后升级为硬失败）

1. **允许**：新增 enum 变体、新增带默认实现的 trait 方法、新增文件、新增类型。
2. **禁止**：删除/重命名既有变体与方法签名、修改既有变体字段、把 `Option<T>` 改成 `T`。
3. **检查方式**：对上述文件执行 `git diff`，若出现 `-` 行（除注释/空行外）且不含 `#[deprecated]` 标记 → 提示。
4. **实现位置**：`scripts/check-contract-freeze.ps1`（期 0 骨架，CI 在期 3 后接入）。

### 4.3 演进流程（弃用周期）

```
新增变体 v2  →  旧变体标 #[deprecated(note = "用 v2，将在 0.X.0 移除")]  →  两个版本周期  →  删除旧变体
```

---

## 五、待决问题

1. **`ToolListener` 的合并时机**：合并会改动 `pipeline.rs` 的注册签名（期 0.5 的 0.5.4 已要求保留 spill/监听在注册表层），需与工具契约改造同批验证。
2. **`RuntimePort` 的粒度**：通道是否需要感知"轮开始/轮结束/流式块"三种事件的区分，还是统一为事件流 —— 期 1 试点后定。
3. **S 级冻结与 `#[non_exhaustive]`**：是否给 S 级 enum 加 `#[non_exhaustive]`（防下游穷举 match），需评估对内部 match 的影响（期 3 决定）。
4. 本表数字与 ADR-049 D3 的「A 级 10 个」有出入（实测 11，且 B 级 9 个为本次首次明确）—— 以本表为准，ADR 的 10 为初估。

---

## 附录 A · `common` 模块二分清单（期 0.4 交付物）

**为什么必须二分**：`common` 入度 **18 个模块 / 344 处引用**（全仓最高），但它混着两类变化原因完全不同的代码：
契约（随**协议**变，S 级）与工具（随**运维偏好**变，B 级）。若整体当 kernel-api，改一行日志格式就是 S 级契约变更，
全插件重编 —— 这正是要避免的。

### A. 契约面 → `kernel-api`（≈ 2,633 行）

| 文件 | 行数 | 内容 | 为什么属契约 |
|---|---|---|---|
| `common/types/structured_message.rs` | 738 | StructuredMessage / Part | 插件间"通用语言"，ADR-002 |
| `common/types/uri.rs` | 435 | TianyanUri + 三层语义 | 所有 VFS 访问的地址形状 |
| `common/types/message.rs` | 288 | Message / MessageRole | 模型与存储共用的消息形态 |
| `common/types/memory.rs` | 199 | 记忆分类类型 | VFS 记忆空间的语义定义 |
| `common/types/metadata.rs` | 157 | 条目元数据 | 三层内容的伴随信息 |
| `common/types/namespace.rs` | 109 | 命名空间枚举 | URI 一级路径的合法集合 |
| `common/types/content.rs` | 89 | ContentLevel（L0/L1/L2） | 三层检索的基础语义 |
| `common/types/token.rs` | 86 | TokenUsage | 用量记账的公共结构 |
| `common/types/embedding.rs` | 72 | Embedding | 向量契约的载荷类型 |
| `common/types/content_part.rs` | 48 | 多模态 Part | 图片输入链路（ADR-010） |
| `common/types/tool.rs` | 45 | 工具相关公共类型 | Tool 契约的伴随类型 |
| `common/types/injectable.rs` | 40 | 可注入上下文标记 | 前缀组装（ADR-004） |
| `common/types/search.rs` | 27 | 检索请求/结果类型 | 检索契约载荷 |
| `common/types/mod.rs` | 33 | 模块聚合与再导出 | — |
| `common/error.rs` | 267 | TianyanError 4 变体 + 语义谓词 | **分类契约单点**（ADR-014） |

### B. 工具面 → `kernel-impl`（≈ 1,389 行）

| 文件 | 行数 | 内容 | 为什么不进契约面 |
|---|---|---|---|
| `common/logging.rs` | 535 | 日志初始化/轮转/格式 | 随运维偏好变；格式改一次不该触发全插件重编 |
| `common/token_estimator.rs` | 248 | token 估算启发式 | 估算策略可调（B 级），且只有内核/上下文策略用 |
| `common/llm_judge.rs` | 186 | LLM 评审工具函数（`parse_llm_json` / `truncate_output`） | **留内核工具面**（ADR-049 §B.5 第 4 条，期 3-T3 实测复核）：6 个消费者里 **4 个非评审**——`vfs/summary/engine.rs:119`（内核摘要解析）/ `agent/background.rs:42`（任务通知截断）/ `agent/tool_registry/evolution_ops.rs:13`（演化综述）/ `plugins/tools-web`（抓取正文截断）；移进 `policy-review` 会让内核反向依赖策略插件（破 D2-1）。插件只 `use tianyan::common::llm_judge::…` |
| `common/panic_hook.rs` | 144 | panic 现场落盘 | 进程级保障机制，无插件消费者 |
| `common/http.rs` | 115 | HTTP 客户端工厂 | 适配器内部实现细节 |
| `common/truncate.rs` | 86 | UTF-8 安全截断 | 纯工具函数，被 spill/命令输出复用 |
| `common/binary.rs` | 75 | 二进制文件探测 | 工具实现辅助 |
| `common/mod.rs` | 18 | 模块聚合 | — |

### C. 判定规则（争议时的裁决线）

> **这个类型会不会出现在插件的函数签名里？** 会 → 契约面（kernel-api）；只在实现内部用 → 工具面。

按此规则复核两处易错点：

- `TokenUsage` 属契约（`ChatService` 的返回值里有它）；而 `token_estimator`（估算器）属工具 —— **类型与估算器分家**。
- `ContentLevel` 属契约（VFS 三层的入参）；而 `truncate`（截断）属工具 —— **语义与字节操作分家**。

> 小计：契约面 2,633 行 / 工具面 1,389 行 ≈ 4,022 行（`common` 文件口径 4,040 行，差额为测试与空行）。
