# DeepSeek Harness（DSH）对标审查：差异、差距与可吸收点（2026-08）

> 状态：调研完成（代码事实核对：deepseek-harness 主分支 + 天演本仓库）
> 日期：2026-08
> 范围：天演 vs DeepSeek Harness（`everything is a plugin`，Cordis 驱动）——定位、架构范式、扩展机制、能力差距、可吸收设计点
> 衔接：[capability-gap-analysis.md](./capability-gap-analysis.md)（vs 业界 2026 主流产品）、[competitive-landscape.md](./competitive-landscape.md)、[REJECTED.md](./decisions/REJECTED.md)
> 基线：代码事实（DSH `docs/` + `packages/*/README.md` + `vendor/cordis`；天演 `core/src`、`server/src`、`gui-vite/src`）

---

## 0. 结论摘要（TL;DR）

1. **定位不同，差距不等于缺陷**。DSH 是 DeepSeek 开源发布的 **harness 产品/生态平台**（开发者预览，快速迭代，面向第三方插件作者，npm 分发 + Python SDK + ACP/JSON-RPC 例子）；天演是**本地优先、单用户、综合智能体**（Rust 桌面应用）。DSH 的"一切皆插件"是其**产品形态本身**（可分发、可组合、可替换），天演的静态组合是"本地单机产品"的合理形态。多数 DSH 能力天演没有，属于**定位性差异**而非疏忽——天演 REJECTED.md 已对其中多项（插件市场 #6、OS 沙箱 #11、网络策略 #12、ACP #14、全异步事件 #17）做过明确决策。
2. **天演与 DSH 在核心机制上高度同构**（独立收敛的证据）：会话日志即单一真相源（天演 JSONL StructuredMessage ≈ DSH append-only SessionEvent log + surface 投影）、压缩锚点（compression_marker ≈ compaction replace 语义）、工具化（ToolRegistry ≈ ctx.tools）、前缀稳定缓存（天演 ADR-012 前缀快照 ≈ DSH KV-cache 意识的设计）。**天演在记忆/检索层（VFS L0/L1/L2 + RRF）反而领先 DSH**（DSH 的 context 家族只是 session-reference/agent-instructions/time-context，无向量检索）。
3. **插件化架构是 DSH 最值得学习的部分，但正确姿势不是"引入插件框架"，而是"把 DSH 插件化背后的五个具体机制拆出来，逐个评估吸收"**：
   - ① 工具执行管线瀑布化（pre-execute → guard → execute → post-execute → result 的可插拔监听）
   - ② 工具展示契约（presentCall/presentResult：工具自描述 UI 渲染意图）
   - ③ 作用域（scope）化注册（per-agent 工具可见性/限制，天然映射天演 delegate 角色）
   - ④ 配置分层覆盖（bundle/patch 层，天然映射天演 tianyan.toml 覆盖层）
   - ⑤ 类型化事件目录（typed events + 生成目录，文档=代码）
   - 而"插件分发/市场"（REJECTED #6）与"全异步事件总线"（REJECTED #17）维持否决。
4. **工程方法论上 DSH 的"生成式目录 + 验证脚本"纪律值得吸收**：DSH 的 tool-catalog/config-catalog/cordis-catalog 全部由脚本从源码生成并在 CI 校验 freshness；天演的工具清单目前散落在 3 处手写文档且已漂移（见 §7）。
5. **DSH 有、天演真没有且建议补的能力**：模型可编写的动态工作流（workflow 包，呼应 gap 分析"能代码固化的并行别交给 LLM"）、CLI/SDK（headless 一键跑任务）、循环卫生守卫（重复工具提醒、超时策略）。

---

## 1. 定位与形态对比

| 维度 | 天演 | DSH |
|------|------|-----|
| 定位 | 本地智能代理系统（个人 PC 上的综合智能体） | 开源 agent harness（"模型之外的一切"的工程化载体） |
| 语言/形态 | Rust（5 crate 单 workspace）+ React GUI + Tauri | TypeScript monorepo（49 个顶层包族 / 226 个实际 npm 包 + 2 apps + 9 vendored，pnpm）+ Web/CLI + Python SDK |
| 分发 | 本地构建/安装脚本 | npm 包（`npx @deepseek-ai/dsh web`），插件可 npm/git 分发 |
| 开发者目标 | 无（单用户） | 插件作者（`dsh-plugin` GitHub topic、用户文档含插件教程 publish.md） |
| 扩展模型 | 编译期静态注册 + 配置 + MCP 动态工具 | 运行时插件树（Cordis），一切皆可替换/热重载 |
| 产品状态 | 内部迭代中 | developer preview，明确"会有破坏性变更" |
| 运行界面 | Tauri 桌面 + 浏览器 GUI（3000 端口） | CLI + Web（3080 端口）+ headless 一次性运行 + ACP/JSON-RPC 协议面 |

**一句话**：DSH 卖的是"harness 平台"，天演是"harness 成品"；DSH 的插件化是平台性的（可分发可组合），天演的静态组合是产品性的（可预测可审计）。

---

## 2. 架构范式对比

| 机制 | 天演 | DSH | 评注 |
|------|------|-----|------|
| 组合方式 | 编译期模块 + 启动期 builder 注入（`agent_builder.rs`、`with_approval_workflow`） | 运行时 Cordis 插件树：profile（命名组合）→ bundle（分发层）→ patch（覆盖层，按行 id 整行替换 config） | 天演：无运行时可替换；DSH：`dsh --dump-config` 打印整棵树，任何一行可 patch |
| 扩展点 | ToolRegistry 静态注册（改 5 处加一个工具）+ `DynamicToolExecutor`（仅 MCP 用）+ 技能（call_skill） | 类型化事件（emit/waterfall/parallel/serial）+ Service 注册表（ctx.*）+ scope 化注册 | DSH 的"每个产品特性 = 某个文档化扩展点上的监听器"是可检验的 microkernel 声明 |
| 工具管线 | 固定管线：SecurityPolicy → ApprovalWorkflow（五级）→ VerificationGate，启动注入，策略固定 | 可插拔瀑布：`tools/pre-execute`（allow/deny/ask）→ 单调 guard → `tools/execute`（包装）→ `tools/post-execute`（改结果）→ `tools/result`（只读观察） | DSH 的 guard 是**单调的**（只能 deny 不能 allow），这是防竞态的高明设计 |
| 会话真相源 | JSONL `StructuredMessage` + SessionHeader 前缀快照（ADR-002/012） | append-only `SessionEvent` log + surface 投影（`deriveMessages()`），"model-visible means logged" 不变量 | **概念同构**；DSH 把"派生历史"与"人类回放"分两个投影，天演 marker 截断类似 |
| 上下文工程 | VFS L0/L1/L2 双层摘要 + RRF 融合 + 前缀缓存（**强项**） | session-reference（从日志派生）+ agent-instructions（AGENTS.md）+ time/tmux context（**弱项**） | 天演领先；DSH 没有向量检索/记忆层（靠 session log 全量 + compaction） |
| 多 agent | `delegate_to_agent`（角色化 roles.rs、嵌套 3、后台、join 信号） | subagent provider registry（in-process/fork/ACP/Codex/Claude Code/dsh-sdk 多种 provider）+ workflow 引擎 | 天演同构能力 ✓；DSH 多 provider 与模型可写 workflow 是天演没有的 |
| 安全 | 审批流 + 快照回退 + 命令级审批策略（信任模型：本地单用户） | sandbox seam（landlock/sandbox-exec/Windows ACL 受限令牌）+ guard + 权限切换器 + approval 服务 | 定位差异（天演 REJECTED #11/#12）；DSH 的 monotonic guard 思想可学 |
| UI 渲染 | 前端按 chunk_type 硬编码渲染（tool_calls 基本不渲染） | 工具自带 `presentCall`/`presentResult` 展示契约（card 词汇表：generic/terminal/diff/search/read/web），host/client 各自投影 | **真差距**：DSH 让工具自描述 UI，UI 与工具解耦 |
| 技能 | 内置 7 技能 + GEPA 进化引擎（业界独有） | skill provider 注册表（filesystem/embedded/remote provider）+ 目录快照 | 天演 GEPA 领先；DSH 的 provider 中立可学 |
| 调度/提醒 | scheduler（cron 任务族 + reminder） | schedule（session 本地提醒，状态存于会话日志） | 同构 |
| 目标/计划 | `current_goal` 死字段（REJECTED 后置） | `ctx.goals` 持久化同会话目标 + round-driver | DSH 有、天演后置 |
| 可观测性 | AgentMetrics + Trace span（SQLite）+ 检索轨迹 | session/event 流 → telemetry 插件（session log 即 trace） | 同构；DSH 把 trace 建在日志上，零额外埋点 |

---

## 3. 插件化架构深度分析（用户重点问题）

### 3.1 DSH 的 Cordis 插件化：机制拆解

DSH 基于 vendored 的 **Cordis**（Koishi 生态的 TypeScript 插件框架，核心仅 6 个源文件：context/events/fiber/registry/service/reflect——子代理逐文件核对，确认如下机制）。五个核心思想（`docs/cordis-primer.md`）：

1. **插件即 Service**：函数插件（`inject` + `apply(ctx)`）或 `Service` 子类，生命周期由 Cordis 挂载到当前上下文。代码确认：`registry.ts` 定义插件三形态（Function/Constructor/Object），元数据 `{name, Config?, inject?, provide?, intercept?}`；`service.ts` 的 `Service` 基类构造时即向 ctx 提供自身，随 fiber 卸载自动注销。
2. **上下文即服务仓库**：`Context` 是 Proxy，属性读取经 `ReflectService` 解析到服务仓库（`context.ts`）；插件按 key 找服务而非 import 具体实现。
3. **依赖声明即加载顺序**：`inject` 声明所需服务，`ReflectService.notify()` 在服务提供/注销时唤醒/挂起依赖纤维（`reflect.ts`）——加载顺序由服务需求表达，而非手动 boot 序列。配置 schema 用 vendored **Schemastery**（`z.object/array/…` 链式 DSL，约 900 行单文件）。
4. **类型化事件通信**：**五种**分发模式（emit 观察 / parallel 并行 / serial 串行 / bail 短路 / waterfall around-中间件），`@mode` 标签让生成目录可校验声明与派发点一致；监听器作为 effect 随 fiber 销毁，`thisArg` 携带 scope 过滤分派。
5. **注册即可逆效应**：prompt 段、工具 schema、adapter、provider、监听器都经 `ctx.effect()`/`ctx.on()` 安装；fiber 生命周期 `PENDING→LOADING→ACTIVE→FAILED→DISPOSED/UNLOADING`，卸载时**逆序回滚**——HMR、热换 provider、事务回滚免费获得。

**产品层组合**（`docs/architecture.md`）：
- **profile**：$DSH_HOME 下的命名组合（web/headless 是内置模板），列出 bundle 栈 + 用户 patch。
- **bundle**：Cordis 配置行 + 代码的分发格式（`dsh.bundle` 指向 patch 文件）。`dsh-base`（模型 adapter/工具/持久化/沙箱/审批/设置/凭据/遥测）→ `dsh-web-app`（浏览器应用）/ `dsh-headless`（一次性运行，无服务器）。
- **patch 分层**：bundle 依次应用 → profile 的 `cordis.patch.yml` → 用户 home 级 → `--patch` 覆盖。后层按行 id 整行替换 config。`dsh plugin add` 即 npm/pnpm 安装 + 追加 bundle 层。
- **一切皆插件**：模型 adapter、工具注册表、会话日志、agent loop 本身都是插件行——**没有需要打补丁的特权核心**，扩展 = 在旁边挂一个插件，卸载自动回卷。
- **作用域（scope）**：注册可挂到具体 agent 的 ctx 上（`agent.ctx`），子 agent 继承 + 限制（`ToolRestriction` allow/deny 过滤）。
- **自我引用**：`extensions/tool-cordis` 让 **agent 自己检查运行时、定义并运行模型编写的动态包**——agent 修改自己的 harness。
- **UI 也是插件**：Web 客户端通过 `ConversationNodeDefinition` + keyed renderer 挂业务节点；任何 UI 都从 `session/event` 流渲染。
- **preset vs extensions（两种"扩展"的层次区分）**：`agent-presets` 是**部署期静态组合**（`cordis.yml` 预设，每会话选定，可声明 `isolate` realm）；`packages/extensions` 是**运行期动态创作**——`tool-cordis` 让 agent 自己检查运行时（`cordisInspect`）、定义并运行模型编写的动态包（`node:vm` 沙箱，文档明言"非安全边界"，默认不进任何产品树、显式 opt-in）。前者是"用户/部署者为会话选组合"，后者是"agent 自改 harness"。
- **hooks 桥**：`hooks-claude-code`/`hooks-codex` 把 Claude Code/Codex 的 `hooks.json` 方言桥接到 harness 拦截点（PreToolUse → `tools/pre-execute` 等）——外部生态协议在扩展面上复用，而非另开通道。

### 3.2 天演当前的扩展机制（代码事实）

- **内置工具：编译期静态注册**。新增一个工具需改动 5 处：`tool_params.rs` 参数结构体 → `register_builtin_tools()` → `*_ops.rs` 执行器 → `execute_single` match 分支 →（可选）短路选择表。`grep "plugin|插件"` 全库 0 命中。
- **唯一运行时注入点**：`DynamicToolExecutor` trait + `register_dynamic_tool()`，目前**只有 MCP 桥接**在使用（`mcp_bridge.rs`）。
- **技能系统**："过程知识"通道。GEPA 学习技能写 VFS `skill/` 命名空间（L0 发现 → L2 按需），call_skill 桥接；无 handler 时只返回操作指引。
- **配置**：单层 TOML（tianyan.toml 三级查找）+ 热更新 API（持久化写回）。
- **审批/验证**：`ApprovalWorkflow`/`VerificationGate`/`SecurityPolicy` 启动时构造注入，策略在运行期固定，无监听器注册面。`Action` 枚举 9 变体，6 个危险工具统一经 `ensure_approved()` 门控。
- **事件**（T1 已落地）：`events/` 模块 EventBus（mpsc pub/sub）+ FileWatcher + EventRule，webhook `POST /api/v1/events` → 触发 cron 任务/唤醒会话——用于**自动化触发**，非扩展点（与 DSH 的事件作为扩展点不同）。

### 3.3 关键判断：天演应该怎么"吸收"插件化

**不要**：移植 Cordis/引入插件框架。理由：Rust 无运行时动态加载生态（CDylib 插件体系成本极高）；天演"已有链路不叠加抽象"原则；插件分发已被 REJECTED #6 否决且触发条件未满足（本地单用户不公开分发）。

**要**：把 DSH 插件化背后的**机制**逐个映射到 Rust 可行的形态：

| DSH 机制 | 天演对应改造 | 价值 | 成本 |
|---------|-------------|------|------|
| 工具管线瀑布（pre/post-execute 事件） | `ToolRegistry` 增加 `pre_execute`/`post_execute` 监听器列表（trait object），审批/验证/审计/统计改为监听器 | 审批策略、命令审批、审计日志、超时、重试全部解耦可组合；第三方（MCP 工具、未来插件）可挂策略 | 中（重构 execute_single 路径，需先铺好 trait） |
| 单调 guard | 在审批链尾部加"只允许拒绝"的守卫层 | 防"监听器顺序把拒绝变允许"竞态 | 低 |
| 展示契约 presentCall/presentResult | 工具定义增加 `presentation` 元数据（card 意图），前端按 card 渲染 | 前端 tool card 从硬编码变数据驱动，MCP 工具也能有好看卡片 | 中（前端渲染器 + 后端 schema 扩展） |
| scope 化注册 | `delegate_to_agent` 的 roles（工具白名单）升级为正式 per-agent 工具可见性机制 | 子 agent 工具面与主 agent 解耦，为未来并行实例铺路 | 中 |
| 配置分层覆盖 | tianyan.toml 支持"默认层 + 用户层"合并（不要求 TOML 继承，启动时两层 merge + 热更新写用户层） | 升级不污染用户配置；DSH 热更新写 patch 层同理 | 低 |
| 类型化事件目录 + 生成脚本 | 用 build.rs / xtask 从 `register_builtin_tools()` 生成工具目录 markdown，CI 校验 freshness | 解决工具清单文档漂移（现在已 25 vs 21 漂移） | 低 |
| 注册可逆效应 | （低优先）技能/动态工具的注册/注销配 disposer | 为未来"技能卸载/热重载"铺路 | 低 |

**不吸收**（对照 REJECTED）：插件分发市场（#6）、OS 沙箱（#11）、网络策略（#12）、ACP 控制中心（#14）、全异步事件总线（#17——但注意：进程内同步类型化事件用于工具拦截，与跨 agent 异步通信是两回事，前者值得做）。

---

## 4. 能力维度差距表（天演 ← DSH 视角）

| 能力 | 天演 | DSH | 判定 |
|------|------|-----|------|
| 会话单一真相源 | ✅ JSONL + marker | ✅ event log + surface | 同构（天演略简） |
| 记忆/检索 | ✅✅ VFS L0/L1/L2 + RRF（**领先**） | ◐ 无向量层 | 天演领先 |
| 技能 | ✅✅ GEPA 进化（**业界独有**） | ✅ provider 目录 | 天演领先 |
| 前缀缓存工程 | ✅ ADR-012 快照 + 固定前缀顺序 | ✅ KV-cache 意识（request/header 重建） | 同构 |
| 工具执行策略可插拔 | ❌ 启动注入固定管线 | ✅ 瀑布 + 单调 guard | **DSH 领先 → 可吸收** |
| 工具 UI 展示契约 | ❌ tool_calls 前端不渲染 | ✅ presentCall/presentResult card | **DSH 领先 → 可吸收** |
| per-agent 工具作用域 | ◐ roles 白名单（delegate 专用） | ✅ scope + ToolRestriction | DSH 领先 → 可吸收 |
| 配置分层 | ❌ 单层 TOML | ✅ profile/bundle/patch | DSH 领先 → 可吸收 |
| 动态工作流（模型可写编排） | ❌ 无 | ✅ workflow 包 + worker thread | **真差距 → 建议评估** |
| CLI / headless / SDK | ❌ 无 | ✅ CLI + headless + TS/Python SDK | 真差距（定位相关，可选） |
| 循环卫生（重复工具提醒/超时） | ❌ 无（有 max_turns 委托限制） | ✅ guard 族 | 真差距（低成本） |
| 目标持久化（goal） | ❌ current_goal 死字段 | ✅ ctx.goals | 真差距（低成本） |
| 沙箱 | 🚫 否决（#11） | ✅ sandbox seam（多平台后端） | 定位差异 |
| 插件生态 | 🚫 否决（#6） | ✅ dsh-plugin 分发 | 定位差异 |
| 子 agent provider 多样性 | ◐ 单实现 + 角色 | ✅ 6 种 provider（含外部产品） | 定位差异 |
| 多模态 | ✅ 图片输入 + MCP 截图 | ◐（未深查） | 天演持平/领先 |
| 评测 | ✅ LLM-as-Judge 四维（离线，pub(crate)） | ◐ 未发现独立 eval 包（trace 可回放，具备 trace-grading 基础） | 天演持平 |
| 文档-代码一致性 | ◐ 人工维护（已漂移 3 处） | ✅ 生成目录 + CI 校验 | **DSH 领先 → 可吸收** |

---

## 5. 天演可吸收清单（按优先级）

### A 级（低风险高收益，建议排期）

1. **工具管线瀑布化**（§3.3 第一行）：把审批/验证/审计从"启动注入"改为"pre/post 监听器"。这是吸收插件化思想的**最小核心**——一旦管线可插拔，天演就获得了 DSH 的扩展面骨架，而无需插件框架。
2. **工具展示契约**：工具定义携带 UI 呈现元数据；前端按 card 渲染 tool 调用（read/terminal/diff/search/web）。直接改善 GUI 可用性（现在 tool_calls 无渲染）。
3. **生成式工具目录**：xtask/build.rs 生成工具清单 markdown，替换手写文档（已漂移）。
4. **单调 guard 思想**：审批链末端加只减权限的守卫（成本极低，防未来监听器竞态）。

### B 级（中期，视路线图）

5. **配置分层覆盖**：默认层 + 用户层 merge；热更新只写用户层。
6. **per-agent 工具作用域正式化**：roles.rs 的白名单机制升级为通用 scope 机制。
7. **循环卫生守卫**：重复工具调用提醒（DSH repeat-tool-reminder 模式：tools/post-execute 附加上下文）、工具超时策略（DSH timeout-policy 模式：tools/execute 包装）。
8. **动态工作流评估**：对照 gap 分析 §5.3"能代码固化的并行别交给 LLM"——天演当前 delegate + join 信号是 LLM 决策路径，可评估"模型可写的编排脚本"形态（Rust 里可用 JSON 描述的有向图或嵌入脚本语言，而非 DSH 的 TS worker）。
9. **压缩管线对照**：DSH compaction 是 `toolResultPruner → tokenMeter → LLM 摘要` 的可重放管线，且摘要复用会话自身 system prompt/tools（防打爆 KV cache）——天演 `ContextCompressor` 可对照此顺序与"复用会话配置做摘要"技巧。
10. **会话全文检索**：DSH sessionQuery 提供 FTS5 全文搜索（opt-in）；天演 session 查询只有 CRUD——会话检索对"记忆即历史"场景有增量价值。
11. **spill 模式**：超大工具输出落盘换 locator（spill-policy 挂在 post-execute 决策）——天演大输出目前靠截断，spill 可保全文可查。

### C 级（观望/定位相关）

12. CLI/headless（`tianyan --run "task"`）——若出现无人值守长任务需求（REJECTED #9 触发条件之一）时可一并考虑。
13. goal 持久化——REJECTED 后置项"执行规划 todo"，DSH 的 ctx.goals 证明其价值，若做任务状态持久化时可顺带。
14. extensions 式"agent 自写插件"（DSH tool-cordis）——远期观察；天演若先完成 A1 管线事件化，此能力只需"动态注册面"即可低成本复刻（sandbox 非安全边界需明示）。

### 明确不吸收

- Cordis 本身/运行时插件加载（Rust 成本与收益不匹配）
- 插件市场/分发（REJECTED #6 维持）
- OS 沙箱（#11 维持）、网络策略（#12 维持）、ACP（#14 维持）
- 全异步事件总线（#17 维持——但进程内类型化事件另说，见 §3.3）

### DSH 十大设计点 × 天演吸收判定（子代理报告提炼，附 Rust 落地形态）

| # | DSH 设计点 | 天演判定 | Rust 落地形态（子代理建议） |
|---|-----------|---------|---------------------------|
| 1 | 无特权核心 + 一切皆插件（连 agent loop 都可替换） | ◐ 吸收**内核**（工具管线事件化、策略外置），不做全量插件化——天演"已有链路不叠加抽象"原则优先 | `trait AgentLoop`/`trait ToolRegistry` 接口化 + 动态注册表，核心 crate 只定义接口 |
| 2 | 注册即可逆副作用（`ctx.effect` 返回精确 disposer，卸载逆序回滚） | ◐ 动态工具/技能注册配 disposer（低成本，为未来卸载/重载铺路） | RAII guard / `Drop` 逆序清理 + 类型化 disposer |
| 3 | inject 依赖声明驱动加载顺序（notify 唤醒） | ✗ 不需要——Rust 编译期 + builder 组合已足够，运行时 DI 图收益为零 | （若未来需要）`dyn Any` 服务仓库 + 依赖图唤醒；当前不做 |
| 4 | 事件分派模式化（5 模式 + @mode + scope 过滤） | ✅ 吸收：工具管线 pre/post 监听器 + 单调 guard（A1/A4），进程内同步即可 | `enum DispatchMode {Emit, Parallel, Serial, Bail, Waterfall}` + 注册时声明模式 |
| 5 | capability seam 三分法（Definition/Provider/Consumer） | ◐ 已有雏形（`StorageBackend` seam，ADR-005），显式化为通用模式即可 | trait + 默认实现 + 消费者只依赖 trait |
| 6 | 会话日志唯一真相源 + "模型可见 ⟺ 已入日志" | ✅ **已同构**（StructuredMessage JSONL + 实时持久化）；可补硬不变量文档化 | append-only 事件流 + 从事件流派生 `Vec<Message>` 的纯函数 + 快照检查点 |
| 7 | 工具七段管线 + 单调 guard + 审批 fail-closed | ✅ 吸收（A1/A4）——天演审批链可对齐 fail-closed 语义 | `enum PreDecision {Allow, Deny, Ask}` + `fn guard(&self, exec) -> Option<String>` |
| 8 | 配置 = patch 层组合 + `!!js` 延迟求值 + `--dump-config` 等价 | ◐ 吸收简化版：默认层 + 用户层 merge（B5）；dump-config 等价性天演无需求 | TOML 分层 + id 定向 merge + `--dump-config` 测试断言 |
| 9 | 生成器 + 校验器对（gen-X + --check + verify-spec，文档永不漂移） | ✅ 吸收（A3）——这是 DSH 工程纪律的核心资产 | build.rs/xtask 生成 + `trybuild`/`cargo test` 门禁 + 从代码生成 API 文档 |
| 10 | 双面包 UI + 浏览器内跑 Cordis Loader（客户端插件图） | ✗ 不需要——本地 Tauri + 静态 React 足够，插件化 UI 是 Web 生态特解 | — |

> 注：另有补充提名——Agent Note 决策记录制度（天演 ADR 体系已近似，可落成 `docs/adr/` + CI 格式门禁）、`toolResultPruner → tokenMeter → LLM 摘要`可重放压缩管线（天演 ContextCompressor 可对照）、渐进式技能披露（天演 L0/L1 已同构）、"升级=审批中介重试"模型（escalation 严格加宽 + 一次性授权）、脚本 gate DAG 调度。

> **LLM 缝对照**：DSH 的 `LlmAdapter` 注册表 + 单一流 API（`stream()` 返回统一 `StreamChunk` 词汇）+ `HarnessError` 稳定 code 体系——与天演 `ModelServices`（trait 容器，不路由不重试）+ **ADR-014 语义错误谓词**概念同构，两边的"错误分类契约单一化"是独立收敛的又一例证。

---

## 6. 工程方法论吸收（DSH 的隐藏资产）

DSH 的工程纪律（约 100+ 个 `scripts/*.ts` 每个带 `.spec.ts`）本质是把 Harness 理论（天演 docs/harness核心思路 已总结）**机械化**：

| DSH 实践 | 天演现状 | 建议 |
|---------|---------|------|
| 生成目录（tool/config/cordis/persistence catalog 由脚本从源码生成，CI 校验 freshness；连 agent-lifecycle.md 的时序图都是 `gen-doc-graphs.ts` 生成的；`gen-tool-catalog` 甚至**真启动每个工具插件**读 `ctx.tools.schemas()`——"运行时注册是 schema 唯一真相源"） | 手写文档（已漂移 3 处） | **优先吸收**（A3） |
| 双语文档 + `@mode`/`@dshScopeScan` 标签让声明可校验 | 单语文档，无机器可校验标签 | 可学：给核心 trait/事件加可校验注解 |
| 每个 package 有 README 声明"Model Experience / KV Cache effect / Known Limitations" | 无此格式 | 天演模块文档可补"对模型可见性/前缀缓存影响"节——直接服务 ADR-012 前缀零漂移纪律 |
| Agent Notes 制度（`.agents/notes/` 路径编码 lifecycle/class/日期；**强制 `## Alternatives considered` 节**；"每个非平凡改动必须在同一 PR 加/更新至少一条 Agent Note"；归档树冻结校验） | ADR 体系 + REJECTED（已有类似机制，但无"每个改动带决策记录"门禁） | 可选增强：给非平凡 PR 加"决策记录"约定 |
| "特性 → 机制"映射表（extension-cookbook：每个产品特性 = 某个扩展点监听器） | 无对应物 | 若做 A1（管线瀑布），建议同步产出"天演特性 → 扩展点"映射表 |
| i18n 三件套（md + zh.md + i18n.yaml 存两侧 git blob hash，字节级配对校验）+ 100% 覆盖率门禁 + lefthook 预提交 | 单语中文文档，无对应门禁 | ✗ 不需要（单语项目）；覆盖率门禁可参考 |

---

## 7. 附注：审查中发现的文档漂移（以代码为准）

- 工具数：代码实为 **25**（`register_builtin_tools()` 25 个注册 + `execute_single` 25 个分支一一对应，本人核对）；system-architecture.md（25）正确，capability-gap-analysis.md 的"21 内置"已过时。
- 技能数：文档 6 vs 代码 7（handlers/ 7 个文件）。
- chat 路由：system-architecture.md 列的 `/chat/regenerate`、`/chat/edit` 已不在路由中。
- harness-engineering-overview.md §13.1 说"架构约束未实现"，但 `core/tests/structural.rs`（rg 依赖方向检查）已存在——已推翻。
- `eval` 模块：`pub(crate)` + `allow(dead_code)`，离线定位未暴露到 API。

> **漂移治理状态（2026-08 落地）**：工具清单漂移已由 A3 生成式目录根治——
> `docs/architecture/tool-catalog.md` 由 `cargo run -p tianyan-core --example tool_catalog` 从
> 运行时注册表生成（DSH gen-tool-catalog 纪律），`scripts/gen-tool-catalog.ps1 -Check` 挂入
> `scripts/test.ps1` lint 门禁；`capability-gap-analysis.md`/`module-relationships.md`/
> `module-descriptions.md` 的手写工具清单已改为指向生成目录。

## 7.5 吸收落地记录（对照 §5 清单）

| 清单项 | 状态 | 落地位置 |
|--------|------|---------|
| A1 工具管线瀑布化 | ✅ | `core/src/agent/tool_registry/pipeline.rs`（pre-execute 监听器 / 单调守卫 / post-execute 监听器）+ `execute_single` 三段接线；观测尾部迁移为内置 post-execute 监听器（`observability.rs`，统计/Trace/GEPA 历史/规则学习） |
| A2 工具展示契约 | ✅ | `ToolPresentation` 枚举（`model/types/tool.rs`）+ ToolRegistry 展示意图映射 + SSE `tool_call` 事件（`AgentStreamChunk.tool_call`）透传；前端 `ToolCallCard.tsx` 按 card 渲染 |
| A3 生成式工具目录 | ✅ | `core/examples/tool_catalog.rs` + `scripts/gen-tool-catalog.ps1`（生成/-Check）+ `docs/architecture/tool-catalog.md`；freshness 挂入 `scripts/test.ps1` lint |
| A4 单调 guard | ✅ | `pipeline.rs::ToolGuard`（只允许拒绝，无允许分支；pre-execute 之后、执行之前） |
| B5 配置分层覆盖 | ✅ | 默认层（serde default 字段级）+ 用户层（TOML 覆盖）合并语义固化测试；热更新仍写用户层 |
| B7 循环卫生守卫 | ◐ 部分 | 委托深度 guard / 审批 fail-closed 已有；重复工具提醒/超时策略留待 |
| 其他（B6/B8-B11、C 级） | ⬜ | 留待后续轮次评估 |

---

## 8. 结论

天演与 DSH 的**底层架构哲学高度一致**（会话日志真相源、工具化、前缀缓存工程、压缩锚点），差异主要在上层形态：DSH 用插件化把"harness 平台"做成了可分发可组合的产品；天演用静态组合把"harness 成品"做成了本地可靠的单机应用。天演不该学 DSH 的"形"（Cordis 插件树），但该学它的"神"：

1. **工具执行管线从"固定管线"走向"可插拔瀑布"**——这是吸收插件化最核心、成本最可控的一步，且不与任何 REJECTED 决策冲突；
2. **工具展示契约**——让工具自描述 UI，一次性解决前端 tool 渲染空白；
3. **生成式文档目录**——消灭文档漂移，让"代码是唯一事实来源"机械化；
4. **工程纪律对照**（Model Experience/KV Cache 声明）服务天演已有的前缀零漂移设计。

---

**文档版本**: 2026-08
**依据**: deepseek-harness（`docs/architecture.md`、`docs/cordis-primer.md`、`docs/subsystems/tools.md`、`docs/subsystems/session.md`、`docs/cookbook/extension-cookbook.md`、`docs/user/develop/basic/publish.md`、`packages/*/README.md`、`vendor/cordis/src` 逐文件核对 + 子代理对 packages/core、goal、workflow、subagent、sandbox、mcp/acp、client/web、scripts 的代码级核实）+ 天演本仓库（`core/src`、`server/src`、`gui-vite/src`、`docs/architecture/*`）
