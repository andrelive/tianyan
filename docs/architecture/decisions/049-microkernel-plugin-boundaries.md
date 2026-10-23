# ADR-049: 插件化边界与装配模型（微内核化）

**日期**: 2026-10-20
**状态**: ✅ 已采纳（期 0 实施中）
**影响范围**: 全仓编译边界 —— 4 个 crate → 约 26 个编译域；涉及
`core/src/{agent,executor,context,model,vfs,session,common,config,scheduler}` 的模块归属、
`server/src/state.rs`（装配器二分与字段契约化）、`tauri/src`、`mcp/src`、
`Cargo.toml`（workspace members 与依赖）、`scripts/`（新增重编带度量）、
`docs/architecture/contracts.md`（新增契约分级表）

---

## 背景

### 1. 问题：改一行代码要重编 5.6 万行

实测（HEAD `67d40bf`，`cargo build --workspace`，debug，只 `touch` 文件不改内容）：

| 改动位置 | 重编 crate | 重编源码行数 | 墙钟 |
|---|---|---|---|
| `executor/web.rs`（一个工具） | core + server + mcp + tauri | **64,328** | 97.5s |
| `agent/loop.rs`（循环机制） | 同上 | **64,328** | 97.3s |
| `common/types/uri.rs`（S 级契约） | 同上 | **64,328** | 61.9s |

**三种代价完全相同。** 这不是"某个模块太大"，而是**没有编译边界**：
rustc 的编译单元是 crate，单 crate 内部的模块划分对编译成本零作用。

口径说明：文件行数 98,953；真正参与 `cargo build` 的是 **64,328 行源码**
（core 49,057 / server 13,379 / mcp 867 / tauri 1,025），其余 34,625 行（35%）是测试
（独立测试文件 + `#[cfg(test)] mod` 段，只在 `cargo test` 编译）。

> 口径纪律：统计"测试行"必须只认 `#[cfg(test)] mod`（Rust 惯例：测试模块在文件末尾）。
> 早期用"第一个 `#[cfg(test)]` 之后全算测试"会把仅修饰 helper 的标注当成测试块起点，
> 高估测试、低估源码（曾得出 56,541 的错误分母）。度量脚本 `scripts/measure-rebuild.ps1` 已按精确口径实现。

### 2. 问题：演进耦合与装配散点

- 近 30 天 Rust 侧改动热点：`agent` 157 次、`server/api` 74、`executor` 69 —— 全在最重的模块上。
- `agent` 出度 **16 个模块 / 428 处引用**，是唯一"全知者"（循环 + 工具表 + 后台任务 + 角色学习混居）。
- `common` 入度 **18 个模块 / 344 处引用** —— 它事实上是 kernel-api 的基座，但里面混着
  契约（`types/` 2,368 行 + `error` 267）与工具（logging / token 估算 / UTF-8 截断 / HTTP 工厂 / panic_hook ≈ 1,389 行）。
- `AppState` 31 个依赖字段：11 个已是 `Arc<dyn 契约>`，**20 个仍是具体类型**；
  `state.<field>` 访问点 207 处 —— 装配知识只存在于"接线现场"，无清单、无校验。

### 3. 设计稿与实测的三处偏差（本 ADR 以实测为准）

《微内核架构设计 v0.1》（`.scratch/microkernel-design/index.html`）给出的三处假设经实测修正：

| 设计稿 v0.1 | 实测 | 后果 |
|---|---|---|
| 现状重编带 ≈ 33K 行 | **56,541 行源码**（98,953 文件行） | 收益空间更大，但闸门判据必须按真基线定 |
| 期 1 直接"拆 executor 为工具插件" | `agent` 直接调用 `executor` 的 **40+ 个具体函数**（`execute_read_file` / `apply_patch_action` / `web::execute_web_search` / `shell::HINT_*` / `SecurityPolicy` 14 处 …） | 必须先做「工具契约与注册形状」，否则跨 crate 撞私有可见性 + 反向依赖 |
| "拆 executor"是一件事 | executor 是**四簇**：工具实现 3,692 / 命令执行 2,941 / 安全审批 2,187 / 任务评审 284 | 整块搬会得到换名的大核；审批属策略、命令属工具、评审属策略 |
| 期 4 才做装配收敛 | `server` 有 **11,843 行源码**且 20 个字段是具体类型 | 不先变薄，改工具仍重编 13.4K（只降 76%，而非 95%） |

### 4. 边界正确性只能由数据裁决

微内核化最大的风险不是"做错"，而是"一口吃成胖子"。边界切错时，改接口的成本会变成全局的。
因此本 ADR 的核心不是架构图，而是**每期一个边界 + 一道数据闸门**：先建测量，再投资工程量。

---

## 决策

### D1 · 编译期插件化，不做运行时动态加载

插件 = 独立 crate，随发行版编译链接；**不做** dylib 热插拔、不做运行时 `eval`、不做 WASM 沙箱
（后者留给未来"第三方不可信扩展"场景，当前无需求）。

**为什么**：Rust 无稳定 ABI，dylib 路线需要 `unsafe`（与仓库 `unsafe_code = "deny"` 冲突）；
编译期组装零运行时开销、类型安全、无 ABI 负担。动态性让给**数据层**（P6：技能/规则/角色/记忆全走 VFS，
零编译扩展），静态性留给**代码层**。

### D2 · 三条依赖铁律

1. **插件只依赖 `kernel-api`** —— 不依赖内核实现，不依赖其他插件。
2. **插件之间零互赖** —— 协作只走两条经内核的通道：事件总线（发布/订阅）、契约查询（装配期注入 `Arc<dyn Trait>`）。
3. **内核不认识任何具体插件** —— 内核代码里不得出现具体工具名/插件名。

装配器是唯一"什么都知道"的角色。

**为什么**：单向依赖是编译防火墙（内核不需要知道插件 → 加 100 个插件内核不变大）；
也是概念防火墙（内核 API 面上只有"抽象的工具/存储/模型"）。破一条，拆包收益全部回吐。

### D3 · 契约稳定性分级 S / A / B

| 级别 | 对象 | 纪律 |
|---|---|---|
| **S** | URI / StructuredMessage / Part / ContentLevel / 时序链序号 / TianyanError / Event / 14 个核心 trait（ToolPlugin、VFS 5 件套、Model 4 件套、SessionManager） | **冻结**：只能新增变体 + 弃用周期；新增 trait 方法必须有默认实现 |
| **A** | 装配原语（`register(ctx)` 形状、清单 schema、生命周期）+ 任务/通道契约 10 个 | 谨慎演进：先加可选参数再加门 |
| **B** | 插件内部的一切（参数解析、错误文案、压缩阈值、prompt 模板、方言数据、任务业务、插件自有小契约） | 自由 |

**为什么**：Rust 下"契约 = 成本"，契约变更会穿透依赖图。把"想改就改"的自由让渡给插件、
把"不许乱动"的约束集中到最薄的一层，是让自由与稳定共存的最小代价。
**冻结时点 = 期 3 结束**（此前以"未冻结"演进，用工具 + 适配器两类真实消费者压测）。

### D4 · 内核四件套（K1–K4），规模目标 19.4K

| 组件 | 内容 | 规模 | 变化原因 |
|---|---|---|---|
| **K1 循环骨架** | turn 控制流、收尾判定（无工具调用即完成）、取消（drop 即取消）、回退（先文件后链）、工具管线（pre → 守卫 → 审批 → 执行 → post）、重试/预算**位置**、前缀零漂移 | 2.9K | 正确性 |
| **K1' 工作集** | ADR-035 物化上下文（单写者、压缩点刷新） | 1.1K | 正确性 |
| **K2 消息与事件** | URI 三层语义、StructuredMessage/Part、时序链 seq/压缩点、事件定义 + 总线接口 | 2.4K + 0.2K | 协议演化 |
| **K3 契约与机制基座** | 24 个契约 trait 声明（1.2K）+ VFS 机制（1.2K）+ 会话 Store（1.6K）+ DB 门面（0.6K）+ 配置加载（3.3K） | 5.4K + 3.4K + 3.3K | 生态需求 |
| **K4 装配原语** | 注册表（工具/任务/监听器/服务）、`register(ctx)`、依赖拓扑求解、生命周期、清单校验 | 1.4K + 调度 1.2K | 装配方式 |

**明确的反例**（进内核即错）：prompt 文案、工具选择策略、压缩阈值、模型路由规则、任何具体工具名、
SSE 推送策略、具体事件的业务处理、`PluginManager` 大对象、运行时热插拔。

**规模现实**：api 5.4K + impl 14.0K = **19.4K**，而非设计稿的 15K。
差额主要是配置加载 3.3K 与存储机制 3.4K。两条路径留给期 3 裁决：
① 配置加载外移为 `service-config`（→16.1K）；② 修改闸门为 `api ≤ 6K && impl ≤ 15K`。
**规模闸门不得凌驾于依赖规则**：不得为达标把 `SessionStore` 挪出内核（ADR-018 说它是会话唯一权威，循环每轮读写，
挪出去等于让内核依赖插件，违反 D2-3）。

### D5 · 装配器二分（thin-channel + assembly-manifest）★ 本 ADR 新增

通道层（`server` 主体 / `tauri`）**只依赖 `kernel-api`**，通过 `Arc<dyn Trait>` 访问能力，
**不认识任何插件 crate**；另设 `assembly-manifest`（约 500 行）作为唯一 `use` 具体插件 `register()` 的地方。

**为什么（量化理由）**：拆插件只贡献约 75% 降幅（64,328 → 约 16,000）；装配器二分后到 **95%**（→ 约 3,300）。
因为 `server` 有 13,379 行源码且 20 个字段是具体类型——它若不薄，改任一插件都要重编它。

**结构要求（关键细节）**：清单**不能住在 `tianyan-server` 库里**，否则"server 依赖 manifest、manifest 依赖插件"
会让改插件照样重编 server。正确形态是：清单住在**最终 bin crate**（`tianyan-app`，约 600 行，只含 `main` + 装配清单），
`tianyan-server` 保持为**只依赖 kernel-api 的 lib**。于是：改插件 → 重编「插件 + app bin」，server lib 不动。

**真实成本（实测）**：20 个字段契约化 + 约 80–100 处 `state.<field>` 访问点适配（总 207 处 / 37 字段，
其中 `working_sets` 16 / `snapshot_manager` 14 / `clipboard_*` 21 / `session_leases` 10 / `todo_store` 6 /
`role_store` 5 / `goal_store` 5 / `event_bus` 4 需改）。约 2–3 天。

**因此「装配器二分」提前到与"工具契约与注册形状"同批**（原设计稿放在期 4），否则闸门 A 的数据会被误判为"拆包无效"。

### D6 · 装配清单是有序数据，不是代码

每个插件暴露唯一入口 `register(ctx)`，声明"贡献什么 / 依赖什么契约"；装配器持有**有序数组**清单，
按依赖拓扑求解，缺件/成环在**启动期响亮失败**。

**为什么必须有序**：工具表顺序决定提示词前缀的字节序列。用 HashMap 会让同一工具集每次请求产生不同排列
→ 前缀缓存命中率归零。现有 `DynamicToolTable` 注释已记录该坑，并有
`test_dynamic_tool_order_is_registration_order_and_stable` 守护 —— 清单把这条纪律从"代码习惯"升级为"数据结构约束"。

**发行版 = 同一份内核 + 不同清单**：desktop（全量）/ headless（去桌面项）/ minimal（去 web/mcp/scheduler）。

### D7 · 边界判据：按"变化原因"切，不按"代码像不像"

唯一判据：**需求变了要不要动核？** 会 → 插件；不会 → 内核。

由此得出 `common` 必须二分：契约面（`types/` + `error`，进 kernel-api）vs 工具面
（logging / token_estimator / truncate / http / panic_hook / binary，留在内核实现或 service）。
若整体当 kernel-api，改一行日志格式就是 S 级契约变更。

`context`（3.4K：assembler / pipeline / intent / retriever / loader）是干净的策略簇，直接成为
`policy-context` 插件，不必动内核。

### D8 · 复用既有抽象，不新增 Plugin 体系

插件契约**已经存在**：`DynamicToolExecutor`（`agent/tool_registry/mod.rs:150`）的
`tool_name() / definition() / execute(session_id, args)` 就是所需全部；`DynamicToolTable` 已是有序注册表；
`ToolRegistry` 已支持 pre/guard/post 管线与 spill（ADR-047）。

期 0.5 的动作是**删掉 `execute_single` 里 30 分支的 `match`**（`mod.rs:952 → 993`），让内置工具实现既有 trait ——
**净简化**。禁止新建 `PluginManager` / `PluginLifecycle` / `PluginContext` 四件套（抽象税会超过编译收益）。

**实施记录（期 0.5 第一刀，2026-10-20）**：分派查表化已落地，但形态与设计稿有一处实质偏差，值得留档。

设计稿写"内置工具实现 `DynamicToolExecutor`"，实测**不能**直接这么做：28 个 `execute_xxx` 方法要读
`ToolRegistry` 的 30 个状态符号（`security_policy` 16 处 / `background_tasks` 7 处 / `vfs` /
`command_tasks` / `session_manager` / `web_client` / `role_registry` …）。独立 struct 必须持有这些依赖；
若持 `Arc<ToolRegistry>` 则形成**循环引用**（registry → 动态工具表 → 插件 → registry）永不释放。
本期改为**函数指针表**：`BuiltinToolMeta.execute: fn(&ToolRegistry, sid, args, sub) -> ToolFuture`
——无所有权、无循环，且保留「编译期插件化」（本 ADR 明示不做运行时 dylib/eval）。
真正的「插件持依赖」形态留给期 1：那时 `*_ops.rs` + `executor` 实现一起搬进插件 crate，依赖经
`ToolContext`（0.5.3 待办）显式传入，才谈得上"工具自己知道怎么执行"。

查表化顺带暴露一个**结构事实**：`execute_single` 的 `match` 是第二份隐式执行清单。协议工具 `submit_result`
（ADR-030 结果落盘，**不暴露给 LLM**）只存在于 `match` 分支、不在 `BUILTIN_TOOLS` 表里——改成查表后它立刻
变成"未知工具"，9 个委托链测试当场失败。修复不是补分支，而是把这条结构显式化：`BUILTIN_TOOLS`
（模型可见，27 项）与 `PROTOCOL_TOOLS`（协议工具，1 项，不进 definitions / 展示意图 / 角色白名单）**分表**，
`agent_ops::is_protocol_tool` 由执行表派生——白名单豁免与"可执行"同源，不再有第三份名字清单。

**实施记录（期 0.5 第二刀：工具依赖显式化，2026-10-20）**：新增 `agent/tool_context.rs`，把工具域
依赖从注册表私有字段搬到显式结构。

依赖边界是实测出来的，不是设计的：六个工具域（`file_ops` / `fs_ops` / `code_ops` / `symbol_ops` /
`repo_map_ops` / `knowledge_ops` / `evolution_ops`）总共只用到 **10 个符号**——`security_policy`（16 处）、
`resolve_tool_path`（7）、`ensure_approved`（4）、`resolve_base_dir`（3）、`vfs` / `session_store` /
`knowledge_ingestor` / `execution_log` / `session_recall` / `repo_map_cache`；而 `agent_ops`（委托 / 任务面板）
用到 **32 个**，其中 `execute_parallel` / `child_registry` / `definitions` / `spawn_background_delegate` /
`build_delegate_tools` 是**内核能力**（子代理生命周期、工具集构造），不属于"工具干活需要的外部世界"。

因此落地形态是：`ToolContext`（10 字段 + 5 个共用方法，`ToolRegistry` 持一份并 `Deref` 到它）承载七个工具域，
`agent_ops` 留在 `impl ToolRegistry`。`Deref` 是刻意的过渡选择——注册表内部逻辑与全部既有调用点
（`registry.resolve_tool_path(..)` / `registry.security_policy` / `ensure_approved`）**零改动**，
重构成本集中在一处，行为风险最小。

期 1 的输入因此变清晰：工具域只剩 **16 处** `crate::executor::` 引用（file 8 / fs 3 / symbol 3 / code 1 /
repo_map 1），随插件整体搬走；内核侧的 executor 引用（coordinator 7 / agent_core 5 / background 5 /
rollback 4 / builder 4）是机制级（`kill_all_running_children` / `Action` / `SecurityPolicy` 等），不属于工具耦合。

**实施记录（期 1 首刀：装配器二分 + 插件骨架，2026-10-20）**：

**装配器二分落地形态**：新建 `app` 包（`tianyan-app`，bin 名仍为 `tianyan-server`，产物名不变），
作为**全仓唯一 `use` 具体插件类型的地方**。server 库经 `ToolPluginFactory`（类型擦除：
`Box<dyn FnOnce(&ToolContext) -> Vec<Arc<dyn DynamicToolExecutor>>>`）接收清单，自身零插件依赖。
启动链新增 `create_app_with_plugins` / `start_server_with_plugins` /
`start_server_with_shutdown_signal_and_plugins`，旧签名全部保留（测试与嵌入式走旧路径，传 `None`）。
桌面端（tauri）与独立 bin 共用插件 crate 的 `plugin_factory()` —— 清单单一事实源在插件内。

**为什么 bin 必须独立成包**（本轮实测的硬约束）：Cargo 的依赖粒度是 **package**。若清单写在
`tianyan-server` 包自己的 bin 里，整个 server 包就依赖插件 crate → 改一行工具重编 server 库
（13.4K 源码），闸门 A 直接失效。把 bin 挪到独立包后，改插件只重编「插件 + bin（~90 行）」。

**闸门 A 首测数据**（`cargo check --workspace`，touch `plugins/tools-fs/src/lib.rs`）：
重编 crate = 仅 `tianyan-tools-fs`；重编源码 **512 行 / 0.8%**；墙钟 **3.38s**。
对照期 0 基线（改一个工具 = 63,999 行 / 100% / 106.7s）：降 **125×**（行数）/ **31×**（墙钟）。
判据 ≤3.5K 达标。

**发现的域内交叉依赖**（决定了 fs 域不能切片迁移）：core 的 `read_file`（`executor/actions.rs`）在
目录模式下委托 `executor::fs::execute_list_dir`（源码注释写明"唯一目录列举实现"）。因此把
`executor::fs` 单独搬进插件会让 core 反向依赖插件——**fs 域必须整域迁移**（read_file / write_file /
apply_edit / apply_patch / glob / list_dir 六个工具一起走），并在同一刀里拆分 `actions.rs`
（read/write 与命令执行混在一个文件）。这是期 1 剩余工作的主体。

**实施记录（期 1 第三刀：实现搬移撞上的边界，2026-10-20）**：尝试把 `executor/fs.rs` 与
`executor/actions.rs`（read/write 实现）整体搬进插件，**撞到一条真实边界并回退**（未留残留改动）。

边界不是技术难度，而是**服务层复用**：`server/src/api/workspace/services.rs` 直接
`use tianyan::executor::execute_read_file` 做工作区文件预览——读取实现是**工具与 HTTP 服务共用的
设施**，不是工具专有；而 `read_file` 的目录模式又委托 `execute_list_dir`（唯一目录列举实现），
于是 fs 域实现与 server 强绑定。

**修正后的期 1 结论**：
- **可搬（已完成）**：工具的定义、参数校验、安全门控、审批、错误包装、结果序列化 —— 全部在
  `plugins/tools-fs`。改这些只重编插件（实测 512 行 / 3.38s）。
- **本期不可搬**：`read`/`write` 与 `fs` 的**执行实现**——它们同时服务工具与 HTTP API，属"共享
  设施"。要搬走必须先给 server 一条独立于工具实现的读取路径（新契约），那是期 3
  （kernel-impl / 适配器分离）的活。

因此闸门 A 的准确表述是「**改工具行为（定义/门控/包装）→ 只重编插件**」；改共享设施
（读取 / 列举算法）仍全量。原判据"改一行工具 ≤3.5K"里的"一行工具"应落到前一类，后一类待期 3
重新测量。

**实施记录（期 1 收尾 + 闸门 A 复测，2026-10-20）**：`tools-code`（`symbol_outline` /
`repo_map` / `grep`）整域迁移完成——代码智能域 **server 侧零复用**（普查确认），所以实现
可以整体搬走：`symbols`(14.6K) + `repo_map`(18.8K) + `search_engine`(11.1K) + `search`(3.8K)
及测试一并迁入 `plugins/tools-code`，tree-sitter 与 ignore/regex 随之离开 core。

参数契约按先例留 core：新增 `executor/search_types.rs`（`OutputMode` / `SearchOptions` /
分页常量）；`RepoMapCache` 随实现进插件并由工具实例自持（照 `RepoMapCache` 的处理——它此前
挂在 `ToolContext` 上，是 core 反向依赖插件的入口）。

**闸门 A 复测（装配已接入，数据可信）**：改 `plugins/tools-code/src/repo_map.rs` 一行 →

| 指标 | 数值 |
|---|---|
| 重编 crate | `tianyan-tools-code` + `tianyan-tauri` + `tianyan-app` |
| **`tianyan-server`(13.4K) + `tianyan-core`(49K)** | **零重编** |
| 重编源码 | **2,655 行 / 4.1%** |
| 墙钟 | **5.87s** |

对照期 0 基线（63,999 行 / 100% / 106.7s）：**降 24×（行数）/ 18×（墙钟）**，判据 ≤3,500 行达标。
（此前的 512 行是「插件未接装配」的乐观值，不可引用。）

**序 0（零代码收益）**：迁移 tools-code 时只搬了代码、漏了清 core 的依赖声明——`tree-sitter`×6 /
`ignore` / `regex` / `cargo_metadata` 在 `core/src` **零引用**却仍在编译链上，ADR 宣称的
「tree-sitter 只进 tools-code」当时并未兑现。已删除这 9 个死依赖。

> 坑：`kamadak-exif` 的 **crate 名是 `exif`**（package ≠ lib name），`knowledge/image` 在用；
> 按 package 名 grep `exif` 会漏判为死依赖，删掉后编译报 `cannot find module or crate exif`。

### 期 2 普查结论（迁移判据修正 + 顺序）

**判据 3 必须二分**（本轮普查的关键修正——混为一谈会得出「整个 core 都不可搬」的错误结论）：

| 类型 | 含义 | 处置 |
|---|---|---|
| **3a 业务复用** | HTTP handler 直接调实现干活（`api/workspace/services.rs` 用 `execute_read_file` / `apply_edit_action`） | 真阻塞，**实现不可搬** |
| **3b 装配接线** | `server/{lib,state}.rs` 构造 / 存字段 | 按 D5 **本应搬到 bin**，属可解成本，不是边界 |

tauri 全域只引 `tianyan::{config,common,agent,notification}`，对候选模块复用为 0。

**迁移顺序（自包含度 × 收益，先易后难）**：

| 序 | 目标 | 关键理由 |
|---|---|---|
| 0 | 清 core 死依赖（已完成） | 零代码，兑现依赖面收窄 |
| 1 | `tools-todo` + `tools-goal`（1 crate，834 行） | 工具实现**住在 server**（`api/todos/tool.rs` 631 + `goals/tool.rs` 203），core 对 store 零引用 → 唯一「零前置改造」簇；今天改一行 todo 要重编 server 13.4K |
| 2 | `tools-web`（`executor/web.rs` 1258） | server 零复用；前置仅 `ToolRegistry.web_client` 字段改为插件自持 |
| 3 | `storage-vector`（lancedb，收益最大） | `VectorStorage` trait 已在 core；箭头族依赖随之离开 |
| 4 | `storage-sqlite`（sqlite 518 + local 414） | rusqlite 仍留 core（db/session/observability 共用）→ 收窄的是重编面积而非依赖面 |
| 5 | `task-evolution` + `task-builtin`（3627，2 crate） | 前置：`EvolutionReviewExecutor` trait 提到 core |
| 6 | `provider-llm` + `provider-embed`（3185，最难）✅ **2026-10-21 完成**（见「实施记录（期 2 序 6）」） | 两个前置已解：`ModelServices` 容器外移 `server/src/model_services.rs`；连通性探测改走 `ConnectionProbe` 契约 |

**隐性耦合（迁移前必须先解，照 `RepoMapCache` 先例）**：`ToolRegistry.web_client`、
`ToolContext.knowledge_ingestor`、`ToolRegistry.usage_stats` / `rule_recorder`（observability）、
`EvolutionReviewExecutor`（trait 定义在待搬文件、实现在 server → 必须先提到 core）、
`ModelServices`（容器 + 构造器，须整体外移 assembly）、
`knowledge/image → model::provider::vision::infer_mime_type`（两域都搬会形成**插件间依赖**，违 D2-2）。

**实施记录（序 2：tools-web，2026-10-20）**：`executor/web.rs`（1383 行，含
`SearchResult` / `WebSearchClient` / `validate_public_url` 与全部测试）整域迁入
`plugins/tools-web`（`web_search` / `web_fetch`），`scraper` 依赖随之离开 core。

- **客户端来源**：`PluginServices` 增 `web_config: WebConfig`（core 配置类型），插件在
  `tools(&config)` 里 `WebSearchClient::new(config)` 构造；server 库仍零插件依赖
- **降级语义必须保留**（迁移期最容易丢的东西）：`enabled = false` → 返回空清单
  （迁移前 `AgentBuilder` 有 `if web_config.enabled` 门控，而 `WebSearchClient::new`
  本身**不读** enabled —— 不在插件里显式判断，"禁用"就会失效）；构造失败（未知后端 /
  缺 key）→ `warn!` + 空清单（对齐原 builder 的 match 失败分支），**装配不 panic**。
  两条都有测试锁定（`test_tools_empty_when_disabled` / `test_tools_degrade_on_invalid_config`）
- `ToolRegistry.web_client` 字段 + `with_web_client` 一并删除：web 客户端不再经注册表，
  改由插件自持（与 `RepoMapCache` 同一处理模式——这是 core 反向依赖插件的入口）
- `AgentBuilder.web_config` 字段 / `with_web_config` 移除：web 配置只经 `PluginServices`
  进入装配（否则该字段成为 never-read 死字段）
- 渲染契约：`MIGRATED_TOOL_PRESENTATIONS` 加 `web_search` / `web_fetch` → `Web`

**实施记录（序 3：storage-vector，2026-10-21）**：`vfs/vector/lancedb/`（`mod.rs` 501 +
`batch.rs` 124 + `tests.rs` 323 = 948 行，**16 个测试全迁**）整域迁入
`plugins/storage-vector`，`lancedb` / `arrow-array` / `arrow-schema` 随之离开 core。

- **契约留 core**：`vfs/vector/mod.rs` 只剩 `VectorStorage` trait（`vfs::VectorStorage`
  公开路径不变）；`vfs::LanceDbVectorStore` 导出删除，插件导出 `tianyan_storage_vector::LanceDbVectorStore`
- **装配扩展点 `AssemblyServices`**（`server/src/assembly.rs`）：把「插件工具清单」与
  「向量存储工厂」打成一个结构，而不是给每个启动入口再加参数——入口本是
  `listener × 关闭信号 × 插件` 的组合，再加一维即组合爆炸。旧签名（`create_app` /
  `start_server_with_plugins` 等）**全部保留**且语义等价「只装插件工具 + 空向量存储」；
  `app`/`tauri` 改用 `*_with_assembly`。工厂用 `Arc<dyn Fn>`（非 `FnOnce`）：数据目录
  搬迁后的重启会再次装配
- **降级语义显式化**：未装配工厂时 `NoopVectorStorage`（写入丢弃 / 检索恒空 / 行数 0），
  启动 `warn!` 指路；**首次向量写入再 `warn!` 一次**（`AtomicBool` 只打一次）——丢弃数据
  若不留下证据，生产误配的唯一症状是「语义检索永远为空」，看不出是降级还是数据缺失
  （ADR-038 可见失败）。降级实现放 server 不放 core：它是「没插插槽时怎么办」的装配决策，
  core 只定义契约
- **依赖面证据**（`cargo tree --depth 1`）：`tianyan-core` 与 `tianyan-server` 均无
  `lancedb` / `arrow*` / `datafusion`；`tianyan-storage-vector` 直接依赖 `lancedb 0.33` +
  `arrow-array`/`arrow-schema 58.4`。**这是本序与序 4 性质上的分水岭**：序 3 收窄的是
  **依赖面**（编译期最重的箭头链整体改道），序 4 的 `rusqlite` 搬不走（db/session/
  observability 共用），只收窄重编面积
- 4 个 core `examples/`（`memory_cleanup` / `memory_path_repair` / `rebuild_overviews` /
  `skill_import`，含 `import-skills/` 12 篇源文档）随实现迁入插件——它们只依赖 core 公开
  API + `LanceDbVectorStore`。**遗留**：这些是一次性运维脚本而非插件功能，更干净的做法是
  `tools-ops` bin 包（与 `app/tauri` 里重复的 12 行 `vector_factory()` 同一类问题：要彻底
  去重需引入「装配辅助 crate」，超出本序范围）

**实施记录（序 4：storage-sqlite，2026-10-21）**：`vfs/backend/sqlite.rs`（518 行 / 10 测试）
与 `vfs/backend/local.rs`（414 行 / 4 测试）**整文件**迁入 `plugins/storage-sqlite`
（crate `tianyan-storage-sqlite`，导出 `SqliteBackend` / `LocalFileBackend`）。

- **契约留 core**：`vfs/backend/mod.rs` 只剩 `StorageBackend` trait（公开路径
  `vfs::StorageBackend` 不变），`vfs::{LocalFileBackend, SqliteBackend}` 导出删除；
  `backend/sqlite_db.rs` **必须留 core**——它只是 `crate::db::sqlite_db` 的 re-export
  兼容层，实体（共享连接 + schema 集中）在 `db`，搬走会造成 core → 插件的反向依赖
- **`rusqlite` 搬不走：本序收窄的是重编面积，不是依赖面**（与序 3 性质上的分水岭）：
  `db/*`（`Database` 门面 + 4 个 Repository，ADR-020 全系统写入门面）与 `session/*`
  （ADR-018 会话权威）共有 80 处 rusqlite 用法，删依赖需先拆 ADR-020。证据：
  `cargo tree -p tianyan-core --depth 1` 仍含 `rusqlite 0.31`（预期，不要"修"）；
  `cargo tree -p tianyan-storage-sqlite --depth 1` 含 `tianyan-core` + `rusqlite`
- **装配**：`AssemblyServices` 加第三插槽 `StorageBackendFactory`
  （`Arc<dyn Fn(&StorageConfig, Arc<Database>) -> Result<Arc<dyn StorageBackend>>>`，
  **同步**——两个 `new` 都只存字段，建表/建目录在 `initialize` 里）；`config.storage.backend`
  的 **adapter 选择语义搬进装配层**（`app/src/main.rs` / `tauri/src/server.rs` 的
  `storage_factory()`）；`initialize_vfs_for_app` 退化为
  `assembly.storage_backend(&config.storage, database.clone())?`
- **降级语义（本序最需要留档的决策）：未装配工厂 → server 侧 `EphemeralStorageBackend`
  （进程内存，语义照 `SqliteBackend` 抄），不是 `Err`**
  - 为什么不 `Err`（原倾向）：`create_app` / `start_server*` 这组**不带装配的公开入口**
    是序 3 明确「保留且语义等价」的 API 面（`create_app` 一家就有 47 处 lib 单测 + 11 处
    集成测试调用）。存储是启动硬依赖，`Err` 等于让这批入口在**任何**构建里都起不来
    ——那是删 API，不是收敛装配
  - 为什么不用「本地文件兜底」：`LocalFileBackend` 在插件里，server 库构造不出来；
    在 server 侧再写一份文件后端＝两份实现同时漂移
  - 可见性（ADR-038，比序 3 的向量降级更严）：装配期 `error!` 一条 + **首次写入再
    `error!` 一条**（`AtomicBool` 只打一次）。向量降级只是「没有语义检索」，这里的
    降级是「VFS 内容不落盘、重启即丢」，所以用 `error` 级
  - 判别力：`server/src/assembly.rs` 4 条测试锁住「兜底可用 / 首次写入留痕 / 工厂优先 /
    可重复调用 / 前缀语义与 `SqliteBackend` 同口径」；`app/tests/assembly.rs` 加
    **装配级断言**「另开连接到同一 db 文件读回 + Local 分支真的建目录」，锁住「装配层
    真的注入了插件实现而不是兜底」
- **★ dev-dependency 反向引用方案实测不可行（与「临时 workspace 最小实验」的结论有出入）**：
  core 的单测（`#[cfg(test)]`）必须把 core 的类型传进插件 API（`StorageConfig` /
  `Arc<Database>` / `dyn StorageBackend`）。`cargo test -p tianyan-core --lib` 把 core
  编译**两份**（带 `cfg(test)` 的测试单元 + 插件依赖的普通 lib），于是 trait 与上述
  类型出现两个互不兼容的实例 → `[dev-dependencies] tianyan-storage-sqlite` 编译必失败
  （实测 22 个 `E0308`/`E0277`）。临时实验之所以通过，是因为它没有跨 crate 传 `a` 的类型
  - **落地替代（同源包含）**：`core/src/vfs/backend/mod.rs` 用
    `#[cfg(test)] #[path = "../../../../plugins/storage-sqlite/src/{local,sqlite}.rs"]`
    把插件源码**同源包含**进来，`core/src/lib.rs` 加 `#[cfg(test)] extern crate self as
    tianyan;` 让那份源码里的 `tianyan::…` 在 crate 内指向自身。**一份源码、零复制、
    零漂移**；正常构建（`build`/`check`/发布）完全不参与，core 的生产依赖图里没有任何
    插件（`cargo tree -p tianyan-core` 无 `tianyan-storage-*`）。代价：core 的测试期
    代码与插件源码有**文件级耦合**（已就地写注释说明），且需 `#[path]` 这种非常规手法
  - 判别力硬约束满足：`scheduler/tasks/evolution_task.rs::real_vfs()` 仍是**真实
    `SqliteBackend`**（真实条目形态），没有为省依赖退化成文件后端/桩
- `plugins/storage-vector/examples/*`（4 个一次性运维脚本）随之改
  `use tianyan_storage_sqlite::SqliteBackend`：给 `storage-vector` 加了一条**仅 examples
  用的 dev-dep**（插件间 dev 边，正常依赖图不受影响）；更干净的做法仍是搬进
  `tools-ops` bin 包（序 3 遗留）
- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**；`tianyan-core --lib` **1261 passed**（与 HEAD 同数）；`tianyan-server
  --lib` **183 passed / 1 ignored**（179 + 4 条新装配测试）；`tianyan-storage-sqlite` 14 /
  `storage-vector` 16 / `tianyan-app` 7 / `tools-code` 32 / `tools-web` 40 / `tools-todo` 13 /
  `tools-fs` 5 全绿（含 server 集成测试：e2e 14 / config_api 5 / shutdown 1 /
  snapshot_reload 2）；`cargo catalog-check` OK
- **测试守恒**：HEAD 1261（core `--lib`）= 现在 core 1261（其中 14 个来自 `#[path]`
  同源包含）+ 插件 14（同源重跑，非新增）。**零放弃、零降级**
- **遗留**：
  1. ~~`server/tests/*` 的集成测试仍走兜底内存后端~~ **已修（同一提交内）**：迁移初版
     让 22 个集成测试从真实 `SqliteBackend` 退化为内存兜底（`StorageBackendType` 的
     默认值就是 `Sqlite`，所以这是**判别力回退**而非等价替换）。修法：`tianyan-server`
     加 `[dev-dependencies] tianyan-storage-sqlite`（**无环**——插件只依赖 core，不依赖
     server，因此不会出现 core 那种「同一 crate 两个编译单元」），`server/tests/common/
     factory.rs::test_assembly()` 按 `config.storage.backend` 分发到真实
     `SqliteBackend` / `LocalFileBackend`（与 `app`/`tauri` 同一份选择逻辑），6 处
     `create_app` + 1 处 `start_server_with_shutdown_signal` 改用 `*_assembly` 入口。
     **口径**：`AssemblyServices::default()` 只服务「不想配后端」的嵌入式/单测路径，
     **集成测试一律用 `test_assembly()`**——否则测的是生产不存在的配置
  2. `StorageBackend` trait 仍在 core，是「跨 `cfg(test)` 类型同一性」问题的**根源**：
     期 3 若把契约提到独立 `kernel-api` 包，core 单测就能干净地用 dev-dep（届时可删
     `#[path]` 手法）
  3. 4 个运维 examples 的归属（`tools-ops` bin 包），以及 `app`/`tauri` 里重复的
     `vector_factory()` / `storage_factory()`（要彻底去重需引入「装配辅助 crate」）

**实施记录（序 5a：task-builtin，2026-10-21）**：5 个内置定时任务**整文件**迁入
`plugins/task-builtin`（crate `tianyan-task-builtin`）——`summary_task.rs` 616、
`gc_task.rs` 435、`reminder_task.rs` 415、`snapshot_gc_task.rs` 130、
`usage_stats_flush_task.rs` 98（含各自的内联测试）；`evolution_task.rs`（1909 行 +
需前置提取的 `EvolutionReviewExecutor` trait）**不动**，留给序 5b。

- **core 侧零生产引用**（全仓普查确认）：`core/src/scheduler/tasks/mod.rs` 只剩
  evolution 的导出，`scheduler` 模块因此从「5 实现 + 机制」变成「机制 + 装配原语」。
  残留的两处只是注释（`agent/builder.rs` / `vfs/traits.rs`）
- **契约与机制留 core**：`TaskHandler`（执行契约）、`TaskContext`（运行时世界）、
  `TaskScheduler`（间隔扫描 + 补跑）、`TaskDefinition`（注册机制）
- **★ 装配原语 `TaskRegistration` / `TaskServices` 定义在 core
  （`core/src/scheduler/task_assembly.rs`），不是 `server/src/assembly.rs`**
  ——与任务书初稿的偏差，理由是硬的：这两个类型被**三方**同时引用（server 消费清单、
  装配包包工厂、插件生产清单）。若住在 server，`plugins/task-builtin` 就必须依赖
  `tianyan-server`——破 **D2-1「插件只依赖 kernel-api」**的第一条铁律，也与
  `ToolContext` 住在 core 的先例不一致（内核定义能力集合、插件消费）。
  server 侧**re-export** 成插槽契约（`tianyan_server::{TaskFactory, TaskRegistration,
  TaskServices}`），对装配层而言 API 面与初稿一致
- **装配：`AssemblyServices` 第四个插槽 [`TaskFactory`]**
  （`Arc<dyn Fn(&TaskServices) -> Vec<TaskRegistration> + Send + Sync>`，**同步**）：
  `TaskServices` 由 server 在调用工厂前 **await 好**（聊天模型服务取
  `AppState::shared_model_services()` 的**共享实例克隆**，不重复建 HTTP client 池），
  工厂里只做「按配置构造处理器 + 判启用条件」。字段：`config`（注册期配置快照）、
  `usage_stats`、`snapshot_manager: Option<_>`、`session_manager`、
  `notification_sink`、`chat` + `chat_model`、`working_sets: Option<_>`。
  `app` / `tauri` / 集成测试的 `test_assembly()` 均注入
  `Arc::new(|s| tianyan_task_builtin::tasks(s))`
- **注册顺序 / 名字 / 显示名 / 间隔逐字保持**：插件 `tasks()` 返回
  `summary_generation`(6h) → `garbage_collection`(6h) → `usage_stats_flush`(60s) →
  `snapshot_gc`(12h, 仅 `snapshot_manager` 为 `Some`) → `reminder`(`interval_min*60`,
  仅 `reminder.enabled`)，server 逐条 `register_task`。`evolution` 仍在本库，
  迁移前它排在 `summary_generation` 之后 → 按「清单第 1 项之后」**插入**
  （用位置而非任务名，序 5b 迁走后这段插入自然消失）。顺序不是审美问题：
  `RegisteredTask` 的注释写明「注册顺序 = 扫描顺序」= 同一轮到期任务的执行顺序
- **降级语义**：未装配工厂 → **不注册任何内置任务** + `warn!` 一次指路（不静默、
  不 `Err`）。**与迁移前的唯一行为差**：`start_server*`（不带装配的公开入口）
  迁移前会注册这 5 个任务，现在不会——生产装配（`app`/`tauri`）与集成测试
  （`test_assembly()`）都已注入，覆盖的是生产形态。`create_app` 路径迁移前后都不注册
  任务（注册在 `start_server_inner` 里），47 处 `create_app` 单测的可观测行为不变
- **★ 测试替身必须跨 crate 可见：core 新增 `test-utils` 特性**（本序唯一新增的
  公开面）。这 5 个文件的测试全部基于 `MockVfs` / `MockChatService`（`MockVfs`
  能注入 `move_calls` / `summary_vector_error` 等故障面，重写成真实 VFS 会丢判别力），
  而 `core::test_utils` 原本只在 `cfg(test)` 存在——插件测试拿不到。做法：
  `core/Cargo.toml` 的 `mockall` 改为**可选依赖** + `test-utils = ["dep:mockall"]`
  （默认构建仍不含 mockall，`cargo tree -p tianyan-core --edges normal` 无此项），
  `test_utils` / `MockChatService` 的 `cfg(test)` 放宽为
  `cfg(any(test, feature = "test-utils"))`，插件以
  `[dev-dependencies] tianyan = { …, features = ["test-utils"] }` 打开。
  **一份替身、零复制、零漂移**；生产构建的依赖图不受影响
- **最小 pub 面提公（1 处）**：`agent::background::persist_system_message` 由
  `pub(crate)` → `pub`。提醒任务是它的第三个调用方（ADR-039 §4 唯一写入口），
  不提公就只能复制一份写入逻辑（两个单写者）或退回 `SessionManager` 直写（更糟）
- **逐行比对证据**：`git diff --no-index`（HEAD 原文件 vs 插件文件）——
  `summary_task` 19 行 / `gc_task` 9 / `snapshot_gc_task` 7 /
  `usage_stats_flush_task` 8 / `reminder_task` 43 行改动，**全部是 `crate::` →
  `tianyan::` 的路径前缀**；`reminder_task` 多出的 7 行是 `cargo fmt` 对 3 处
  （`format!` / 两个函数签名）因路径变长（+2 字符/处）超过 100 列后的换行，无语义
  改动。唯一的行为改写是 `(interval_secs.max(60) / 60) as u64` 去掉恒等 `as u64`
  （clippy `unnecessary_cast`；`interval_secs` 本就是 `u64`）
- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace
  --all-targets` **零 warning**；`tianyan-core --lib` **1230 passed**（= 基线 1261 − 31
  搬走的测试）；`tianyan-core --lib scheduler` 45 passed（调度器机制仍绿）；
  `tianyan-server --lib` **186 passed / 1 ignored**（183 + 3 条新插槽测试）；
  `tianyan-task-builtin` **37 passed**（31 迁移 + 6 新清单门禁）；
  `tianyan-app` assembly 7 / server 集成 e2e 14 / config_api 5 / shutdown 1 /
  snapshot_reload 2 全绿；`cargo catalog-check` OK
- **测试守恒**：HEAD core 1261 = 现在 core 1230 + 插件 37（其中 31 条为迁移，
  逐条同名同断言；6 条为新增的**清单门禁**——顺序/名字/显示名/间隔/启用条件/可重复
  调用）。**零放弃**
- **遗留**：
  1. `evolution_task.rs` 与 `EvolutionReviewExecutor` 仍在 core（序 5b：先提 trait
     到 core，再迁实现）
  2. 任务清单的**跨插件顺序断言**仍缺：今天只有插件内测试锁顺序，装配层没有
     「清单顺序 = 执行顺序」的断言（期 4 清单收敛时补）
  3. `TaskServices` 是全量能力集合（8 字段），序 5b 的演化任务可能需要
     `session_recall` 之外的东西（它在 `TaskContext` 里，不需要动）

**实施记录（装配安全加固：把「忘记装配」从运行期隐患变成启动期硬失败，2026-10-21）**：
序 3/4/5a 引入插槽后，`AssemblyServices` 各插槽都可缺省，降级路径分别是「无语义检索」/
「内容不落盘」/「不注册维护任务」。前两者里**存储缺失是数据安全问题**：
`EphemeralStorageBackend` 让服务器起得来、内容不落盘、重启即空，而症状表现为
「检索永远为空」而非报错。两道防线：

- **编译期（删入口）**：删除全部不带装配的启动变体——`start_server` /
  `start_server_with_listener` / `start_server_with_shutdown_signal` /
  `start_server_with_plugins` / `start_server_with_shutdown_signal_and_plugins` /
  `start_server_default` / `create_app_with_plugins`。删除前逐条 `git grep` 确认
  **全仓零外部调用者**（只剩库内互调与 doc 示例）。API 面收敛为
  `create_app` / `create_app_with_assembly` / `start_server_with_assembly` /
  `start_server_with_listener_and_assembly` / `start_server_with_shutdown_signal_and_assembly`。
- **运行期（守卫）**：光删入口不够——`start_server_with_assembly(cfg, cfg,
  AssemblyServices::default())` 仍能编译。`start_server_inner` 入口加硬前置
  `has_storage_factory()`，缺失即 `Err(invalid_input("装配缺陷：…StorageBackendFactory…"))`，
  失败发生在**启动前**（清晰报错）而不是运行期第一次写入丢数据时。
  有测试锁定（`test_start_server_requires_storage_factory`，删守卫即红）。

**为什么 `create_app` 不设守卫**：它是「构造对象图」而非「跑服务」，实例生命周期由调用方
自己负责；47 处 lib 单测依赖它。区分点因此是「是否启动服务」而非「是否有装配」——
这也是两条路径严格度不同的正当理由。降级路径的 `error!`/`warn!` 与「首次写入再告警一次」
保留，作为最后一道可观测性兜底（ADR-038 可见失败）。

**实施记录（序 5b：task-evolution，2026-10-21）**：`core/src/scheduler/tasks/`（`mod.rs` +
`evolution_task.rs` 2,035 行 / 29 测试）**整目录删除**，演化任务整文件迁入
`plugins/task-evolution`（crate `tianyan-task-evolution`，导出 `EvolutionTask` +
`pub fn tasks(&TaskServices) -> Vec<TaskRegistration>`）。**core 从此没有任何具体任务
实现**——5a 之后剩下的最后一份也走了。

- **trait 提取（本序的硬前置）**：`EvolutionReviewExecutor` 原样提到
  `core/src/scheduler/evolution_executor.rs`，公开路径
  `tianyan::scheduler::EvolutionReviewExecutor`（`scheduler` 模块加 `mod` + `pub use`）。
  签名唯一差异：`std::result::Result<String, TianyanError>` → 别名 `Result<String>`
  （clippy `unused_qualifications`；`Result<T>` 就是那个类型的别名，**类型同一、行为
  零变化**，与 core 其余代码写法一致）。
- **`TaskServices` 新增一字段**：`pub evolution_executor: Option<Arc<dyn
  EvolutionReviewExecutor>>`（core `scheduler/task_assembly.rs`）。理由：
  `AgentEvolutionExecutor` 是 **server 侧业务实现**（它要 `AgentCoordinator` 与工作集
  注册表，在专用演化会话里委托 `evolution_reviewer` 角色跑一轮主 agent），必须留在
  server；插件只认识 core 的契约。接线因此正好是 D2 的形状——**server 构造实现 → 放进
  能力集合 → 插件用它构造 `EvolutionTask`**，插件之间零互赖。该字段的 `None` 语义与
  `snapshot_manager: None` 不同（后者是「世界缺一块 → 跳过该任务」，前者是「**能力**
  未提供 → 任务无从构造」），已在字段文档里写明。
- **装配**：`server/src/lib.rs` 的「内置定时任务」块删掉 `let at = 1.min(registrations.len());
  registrations.insert(at, …)` 整段过渡代码（5a 的位置 hack 如期消失）；
  `AgentEvolutionExecutor::new(...)` 的构造移进 `TaskServices` 的组装处：
  `evolution_executor: if app_config.evolution.enabled { Some(Arc::new(...)) } else { None }`
  （参数照旧：`state.agent_lock()` / `state.working_sets()` /
  `state::resolve_chat_model(&app_config)` / `cfg.evolution.review_role`）。server 库现在
  只做「递能力集合 + 按返回顺序 `register_task`」。`server/src/evolution_executor.rs`
  只改一行 import（`scheduler::tasks::EvolutionReviewExecutor` →
  `scheduler::EvolutionReviewExecutor`），实现逻辑零改动。
- **★ 顺序（本序唯一需要设计的部分）**：注册顺序 = 扫描顺序 = 执行顺序，演化任务必须
  保持**清单第 2 位**（`summary_generation` 之后）。但「第 2 位」是**跨插件属性**：
  `task-builtin` 不知道演化任务存在，演化插件也不知道别人排在哪，只有装配清单知道
  「哪个插件该排在哪」；让插件互相 import 来拼接会破 **D2-2**。因此选「方案 (ii)」：
  两个插件各返回自己的（相对）顺序，**装配层显式拼接**，且用 `splice` 而非 `insert`
  循环，让纪律在一行里可见：

  ```rust
  let mut registrations = tianyan_task_builtin::tasks(services);
  // 顺序纪律：evolution 紧跟 summary_generation（清单第 2 位，与迁移前一致）
  registrations.splice(1..1, tianyan_task_evolution::tasks(services));
  registrations
  ```

  三处装配（`app/src/main.rs` / `tauri/src/server.rs` / `server/tests/common/factory.rs`
  的 `test_assembly()`）同形同源；`tianyan_task_builtin::tasks()` 因此**保持既有 5 项
  不动**（不再有任何位置妥协）。
- **降级语义（逐字保持）**：`config.evolution.enabled == false` → 不注册；
  `evolution_executor == None` → 不注册 + `warn!` 一次（能力缺失却已启用＝装配缺陷，
  静默不注册的症状是「演化永远不跑」——ADR-038；生产里 server 用同一个 `enabled` 判
  是否构造实现，故这条分支只在嵌入式/测试装配出现）；`interval_from_cron(&cron)` 为
  `None` → `warn!` + 回退 `24*3600`（**文案照抄**迁移前 server 里那段，一字未改）；
  `max_items_per_run` 原样透传 `EvolutionTask::new`。
- **★ 测试期插件间 dev-dep（如实记一条）**：`real_vfs()` 的两个「真实后端形态」用例
  必须用真实 `SqliteBackend`（`find_entry` 只匹配目录的历史缺陷就是被 Mock 目录形态
  掩盖的），而 core 的 `cfg(test)` 同源包含手法在独立 crate 里用不上。做法：
  `plugins/task-evolution` 的 `[dev-dependencies]` 加 `tianyan-storage-sqlite` + `tianyan
  （features = ["test-utils"]）`。**这是测试期依赖边，生产图不受影响**：
  `cargo tree -p tianyan-task-evolution --edges normal --depth 1` 只有
  core/async-trait/chrono/serde/serde_json/tokio/tracing。
  向量脚手架从 `vfs::MockVectorStorage`（`cfg(test)` 私货）换成
  `tianyan::test_utils::InMemoryVectorStorage`（同为 8 维，语义等价；被锁定的链路是
  结构化条目，不是向量）。**已知：插件间现存在两条测试期 dev-dep 边**——
  `storage-vector → storage-sqlite`（序 4 的 examples，一次性运维脚本）与本条。
  干净解法仍是「装配辅助 crate / `tools-ops` bin 包」（序 3/4 遗留，期 4 视情况）。
- **行为零变化的证据**：`git diff --no-index`（HEAD 原文件 vs 插件文件）**35 增 / 39 删**，
  逐行归类为：① `crate::` → `tianyan::` 路径前缀（**24 处**，含测试里的
  `crate::config::…` / `crate::test_utils::…`）；② trait 定义块移出（11 行，含尾随空行）；
  ③ `real_vfs()` 的脚手架与注释改写（`SqliteBackend` 改走 dev-dep、向量换
  `InMemoryVectorStorage`）；④ 新增 3 行模块文档指向 core 的 trait。**任务名 / 显示名 /
  间隔 / 启用条件 / cron 回退 `warn!` 文案 / `TaskResult` 语义 / 全部断言无一处改动**。
- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**；`tianyan-core --lib` **1201 passed**（= 基线 1230 − 29 搬走的测试）；
  `tianyan-task-evolution` **34 passed**（29 迁移 + 5 新清单门禁：注册元数据/间隔换算/
  enabled 关闭/执行器缺失/可重复调用）、`tianyan-task-builtin` **37 passed**（不变）；
  `tianyan-server --lib` **187 passed / 1 ignored**（不变）；`tianyan-app` assembly 7 /
  server 集成 e2e 14 / config_api 5 / shutdown 1 / snapshot_reload 2 全绿；
  `cargo catalog-check` OK；`cargo tree -p tianyan-task-evolution --edges normal --depth 1`
  无 `tianyan-server`、无 `tianyan-storage-sqlite`。
- **测试守恒**：HEAD core 1230 = 现在 core 1201 + 插件 34（其中 29 条为迁移，逐条同名
  同断言；5 条为新增**清单门禁**）。`tianyan-task-builtin` 37 不动 ⇒ 全仓任务测试
  71 = 37 + 34。**零放弃、零降级**。
- **遗留**：
  1. 跨插件顺序仍**无断言**（`splice(1..1, …)` 只在装配层一行里，没有「清单顺序 =
     执行顺序」的测试；与 5a 遗留 2 同一件事，期 4 清单收敛时统一补）；
  2. `TaskServices` 已长到 9 字段（8 + 演化执行器）。它确实是「装配期能力全集」，但
     期 4 清单收敛时值得按插件分组（每个插件一份能力接口），否则新增插件会持续往
     同一结构塞字段；
  3. `AgentEvolutionExecutor` 仍在 server（正确：它是通道层业务），但它依赖
     `AgentCoordinator` + `WorkingSetRegistry`——期 3 抽 `kernel-impl` 时这两者的归属
     会决定它是否需要一个中间适配器。

---

### 期 3 普查结论（2026-10-21，实测 42,382 生产行；**推翻两处 D7 前提**）

口径与期 0 的 `scripts/measure-rebuild.ps1::Get-SourceLines` 一致（排除
`*_tests.rs` / `/tests/` / `test_utils`，内联测试只算 `#[cfg(test)] mod` 之前）。

**规模**：`core/src` 193 文件 / 72,821 行 = **生产 42,382（58.2%）+ 测试 30,439
（41.8%）**。最大模块 `agent/` 22,919（生产 12,444 / 测试 10,475）——与§待决问题 3
记的「`agent` 22,994 行中 10.5K 是测试」吻合，口径自洽。

**闸门 B 测算（文件级归属，可复核）**：

| 判据 | 实测 | 结论 |
|---|---|---|
| api（trait + 纯类型）≤ 6K | **4,552** | ✅ 余 1,448 —— **契约面不紧张** |
| impl ≤ 15K | **16,067** | ❌ 超 1,067 |
| impl（config 只搬 loader 753 后） | 15,314 | ❌ **仍超 314** |
| impl（config 二分：loader 753 外移 + 模型方言 950 随 `provider-llm`） | **14,364** | ✅ 余 636 |

**口径复原验证**（本次普查可信度的关键证据）：api 4,552 + impl（不含
`background.rs` 1,300）14,767 = **19,319** ≈ §D4 目标 19.4K；再减 config 3,011 =
**16,308** ≈ §D4「配置外移后 16.1K」。**两处误差 < 1.2%** → 文件级归属与 D4 的隐含
口径同一套，数字可直接用于设计。

**★ 推翻前提 1：`context` 不是「干净的策略簇」**。`context/assembler.rs`（341 生产行）
是 K1 的**组装机制**：`loop.rs:13` 与 `agent_core.rs:26` 直接持有 `ContextAssembler`
（纯静态函数 `assemble` / `message_to_structured` / `normalize_tool_pairs` /
`structured_to_messages`，零 I/O），生产调用点 `agent_core.rs:655/762/764/1215`、
`loop.rs:1452`。它承载 ADR-043「工具对连续性规范化留内核」与「前缀零漂移」。
→ 正确切法：`assembler.rs` **留 kernel-impl**，其余 **1,729** 行才是 policy-context。
（原 D7 写「`context` 3.4K 可直接成为插件、不必动内核」）

**★ 推翻前提 2：`skills/` 里的 GEPA 进化引擎已不存在**。`skills/mod.rs:7-10` 记录其
已删除（原因：`apply_evaluation` 的 Deprecate 分支用 `write_content` 覆盖写，会把技能
正文替换成弃用标记）。期 3 表里「`role_learning`→`policy-learning`（含 skills 里的
GEPA）」的 **GEPA 部分作废**；`role_learning.rs`（573 生产）仍在，但全仓**零生产
消费者**（只有自身 + 导出 + 测试；server/tauri/app 无构造点）→ 搬它是「搬死代码」，
**应先裁决「接线还是删除」，不宜为迁移而迁移**。

**五条硬结（都有 file:line 证据）**：

1. **`AgentLoop.tool_registry: ToolRegistry` 按值持有具体类型**（`loop.rs:196`，
   1,343 行）。`ToolRegistry` 同时是 approval / command / security / role_router /
   working_sets 的宿主 → **K1「可搬」的真正前提是先把宿主拆成契约注入**，而不是
   搬 `loop.rs` 文件本身。
2. **`WorkingSetRegistry` 重演 fs 域阻塞（3a 业务复用）**：
   `server/src/api/sessions/services.rs:56/81/247-249/398` 直接调 `reg.ensure()` /
   `ws.delete()` / `reg.remove()`；`state.rs:284/386/738` 持有，`:459` 注入
   `set_boundary_push` 回调；它还是 `pub struct`（无 trait）→ 必须先抽
   `WorkingSetPort`（约 6 方法），否则搬了就破 D5。
3. **`context/retrieval/retriever.rs:42` 持 `Option<Arc<UsageStats>>` 具体类型**
   （写 SQLite 的 261 生产行）→ 抽 `policy-context` 需先给 kernel-api 一个
   `UsageReporter` trait，否则插件将依赖 kernel-impl（违 D2-1）。
4. **`common/llm_judge.rs` 不能进 `policy-review`**：它只有 3 个纯函数，**6 个消费者
   里 4 个非评审**（含内核 `vfs/summary/engine.rs:119`）→ 移进 policy-review 会让
   内核反向依赖策略。正确处置：**留 kernel-impl 工具面**（与 `contracts.md` 附录 A 一致）。
5. **`rollback.rs` 不可文件级独立**：`:24 use agent::agent_core::Agent` + `:59 impl Agent`
   —— 它与 `agent_core.rs:128 impl Agent` 是同一类型的扩展 impl 块。切 crate 时
   必须与 `agent_core` 同单元，**不要按文件切**。

**好消息两条**：`context/compression` 与 `WorkingSet` **编译期零耦合**（`grep` 0 命中；
真正的耦合是 `agent_core.rs:807-818` 的「摘要落库成功后才
`reshape_after_compression`」顺序契约，位置在协调器而非 `context/`）；
`agent/cancel.rs` **完全自包含**（零 `crate::` 引用，68 生产行），是最干净的 K1 切入口。

**裁决与执行（2026-10-21，期 3 开工前）**：

- **路线**：T1（policy-approval）→ T2（policy-context）→ T3（policy-review）→
  T4（config 二分），**T5（kernel-api/impl 切 crate）留到期 4 单独一期**。
- **`role_learning` 已删除**（573 行 + 2 个测试）：零生产消费者（全仓只出现在自身 /
  `agent/mod.rs` 导出 / 自身测试），符合「不为迁移而迁移」。**删除中发现一处业务复用**：
  `server/src/api/roles.rs:215` 用 `categorize_task_by_keyword`（角色面板按任务类型分组
  统计）——该函数是**纯函数、零 I/O、零状态**，与引擎无关，故单独迁到 `agent/roles.rs`
  保留（`tianyan::agent::categorize_task_by_keyword` 路径不变，server 零改动）。
  教训与序 1 的 fs 域同型：**删死代码前必须按「符号」而非「模块」grep 复用面**
  （按模块名 grep 会漏掉 `categorize_task_by_keyword` 这类不含模块名前缀的导出）。
  core 测试 1201 → 1199（−2）。

**实施记录（期 3-T1：policy-approval，2026-10-21）**：`core/src/executor/approval/workflow.rs`
（725 生产行）**整文件**迁入 `plugins/policy-approval`（crate `tianyan-policy-approval`，
导出 `ApprovalWorkflow` / `SessionApprovalNotifier` / `ApprovalPendingNotifier` /
`build_approval_pending_text` / `gate()`，5 个公开符号）。**`types.rs` 与纯函数
`is_user_confirmation` 留 core**——理由不是"顺手"，而是它们出现在 **A 级 trait
`AgentCoordinator` 的签名**里（`coordinator.rs:169` `approval_status` /
`:180` `respond_approval`）：搬走就得让内核 `use tianyan_policy_approval::…`，
破 D2-1/D2-3（与 `vfs::StorageBackend` / `vfs::VectorStorage` 同一条边界）。

- **★ 新增 1 个 pub trait `ApprovalGate`**（`core/src/executor/approval/gate.rs`），
  全批**只此一个**（反指标：每期净增 pub trait ≤ 3）。4 个方法，每个都有调用点证据：

  | 方法 | 内核侧调用点 | 说明 |
  |---|---|---|
  | `authorize(session_id, action, allow_human_wait)` | `agent/tool_context.rs:150` `ensure_approved`（内部 `:162`/`:164` 两个入口合一）→ `write_file` / `apply_edit` / `apply_patch` / `execute_command` 四个危险工具域 | 唯一的判定入口；`allow_human_wait = !subagent`（ADR-011 子任务边界） |
  | `status() -> ApprovalStatusSnapshot` | `agent/coordinator.rs:474` `approval_status` | 配置 / 待处理 / 最近 50 条审计 / 已确认指纹数；`pending_confirmations` 属核心侧状态（`Agent::pending_approval_fingerprints`），由协调器**覆写** |
  | `respond_to_approval(...)` | `agent/coordinator.rs:502` `respond_approval`（GUI 审批面板） | 同名同签名委托给固有方法（用 `<ApprovalWorkflow>::…` 完全限定，不依赖"固有优先"规则） |
  | `record_user_confirmation_by_fingerprint(fp)` | `agent/tool_registry/mod.rs:552` `confirm_pending_approval`（"询问用户"降级链路） | 指纹由 **core** 的 `action_fingerprint`（从 `workflow.rs` 提到 `types.rs`）生成——两侧共用一份算法，否则"用户确认了却没被认账"会静默发生 |

  为什么**不**把 `ensure_approved` 整体搬进 trait：它除了"问审批"，还要在拒绝时写
  `ToolContext::pending_approval_fingerprints`（`Agent` 轮末读它生成追问）并把拒绝翻成
  统一错误文案——那份状态在 core。契约切在**真正跨 crate 的那条线**上：`authorize`
  只回答"准不准"，拒绝后的降级动作留 core。
- **装配：`AssemblyServices` 第五个插槽 `ApprovalGateFactory`**
  （`Arc<dyn Fn(&SecurityConfig, Arc<WorkingSetRegistry>, SharedNotificationSink)
  -> Arc<dyn ApprovalGate> + Send + Sync>`，**同步**，形态照 `StorageBackendFactory` /
  `TaskFactory`）。工厂里有 `WorkingSetRegistry` 一参（与初稿的差异，理由是硬的）：
  审批挂起通知必须经 ADR-039 的**唯一写入口**落成会话 System 消息，且必须是 `Agent`
  用的**同一个 Arc**；而工作集是 `AppState::new` 内部创建的，所以门控在
  `AppState::new` / `reload_agent`（组合根）里构造，而不是在 `lib.rs`。
  「`SecurityConfig` → 自动审批规则」的映射（`blocked_commands` → Deny、`allowed_commands`
  → Approve、`prompt_commands` → 总是询问、`approval_mode` → `mode`）从
  `AgentBuilder::build` 搬进插件的 `gate()`——单一事实源在插件内，三处装配各写一行包装。
- **降级语义（本序的取舍）**：未装配工厂 → `AgentBuilder` 得到 `None` → **不装配审批门控**，
  `ToolContext::ensure_approved` 走既有的「未装配直接放行」分支（该分支本来就在），
  `build()` 里 `warn!` 一条指路。**为什么允许降级**：① 后果比存储降级轻一个量级——
  默认配置本就是 `ApprovalMode::Autonomous`（黑名单外全放行，ADR-033），且命令黑名单在
  `SecurityPolicy` 守卫层**另有**一份拦截；② `create_app`（构造对象图，47 处 lib 单测 +
  嵌入式）靠"不要求装配"才能起实例，`Err` 等于删入口。可见性由装配期 `warn!` 兜底
  （ADR-038：静默降级的唯一症状会是"审批面板永远空的 + 黑名单少一层复核"）。
  `create_app` 路径的可观测差异仅"`approval_status` 返回默认配置而非 `security.approval_mode`
  派生的配置"，`test_assembly()`（集成测试）已注入真实现，测的是生产形态。
- **顺序与语义逐字保持**：`workflow.rs:353` always-ask → `:372` `Autonomous` 短路 →
  `:398` `Safe`→Approve → `:409` `Critical`→Deny → `:427` `is_action_confirmed` →
  `:440` `Interactive` 等人工 → `:450` 兜底 Deny；`default_timeout_secs = 300`、
  `max_pending_approvals = 100`、`persist_records = true`、默认规则 `safe_operations`
  全部逐字未动。
- **★ 两处 core 侧最小提公（跨 crate 后不可省，B 级）**：
  1. `executor::classify_command_risk` / `CommandRisk` / `split_command_segments`
     （后者的 `pub(crate)` → `pub`）与 `executor::command::judged_command_name`
     （`pub(crate)` → `pub`）经 `core/src/executor/mod.rs` 暴露。理由：这些是
     **执行层（`SecurityPolicy`）与审批层共用**的风险/取词/分段单一事实源
     （T0-2 修的正是"只按首词定级"的绕过），搬一份进插件＝两份口径同时漂移。
     `security` 模块本身仍是 `pub(crate)`：经 `executor` 根只开 3 个符号
     （`classify_command_risk` / `CommandRisk` / `split_command_segments`），
     外加 `command::judged_command_name` 一个，公开面收窄到实际需要。
  2. `action_fingerprint` 从 `ApprovalWorkflow` 的关联函数提为 `types.rs` 的自由函数
     （零 I/O、零状态），core（`remember_pending_approval`）与插件（确认集合）共用。
- **★ core 单测的「同源包含」**：3 个 core 用例必须用**真实**实现
  （`loop_tests::test_run_approval_denied_returns_tool_error` 的 Confirm 短路、
  `agent_ops_tests` 两条子任务审批用例的 Interactive 不挂起 / 指纹共享——用替身会把
  这些语义抹平），而 core 的 `[dev-dependencies] tianyan-policy-approval` 行不通
  （core 编译两份 → `ApprovalGate` / `ApprovalWorkflowConfig` 两个不兼容实例，
  同序 4）。因此照 `storage-sqlite` 先例：`core/src/executor/approval/mod.rs` 在
  `#[cfg(test)]` 下用 `#[path]` 同源包含 `plugins/policy-approval/src/workflow.rs`。
  **一份源码、零复制、零漂移**，生产构建完全不参与（`cargo tree -p tianyan-core`
  无该插件）。代价：被包含文件只许 `tianyan::…` 绝对路径（已写进 crate 文档 + AGENTS.md）。
- **逐行比对证据**（`git diff --no-index`，HEAD `workflow.rs` vs 插件 `workflow.rs`）：
  **98 增 / 31 删**，逐行归类——① `crate::…` → `tianyan::…` 路径前缀 **18 处**
  （`grep -c crate::`：HEAD 18 → 插件 1，剩下的那 1 处是本文件头文档里的文字示例）；
  ② `use super::types::*`（1 行）展开为显式清单（9 行，名字与 glob 提供的完全相同，
  因插件 crate 根没有 `types` 模块可 glob）；③ `action_fingerprint` 关联函数 5 行移出 +
  2 处 `Self::action_fingerprint` → `action_fingerprint` + 1 处文档链接；
  ④ `check_auto_approval` 的 `pub(super)` → `pub(crate)`（插件里 `workflow` 是 crate 根的子
  模块，`tests` 是兄弟模块，`pub(super)` 语义会变窄；模块本身私有，无公开面影响）；
  ⑤ 3 行文档措辞（"Agent 装配层注入" → "装配层注入" 等）；⑥ **末尾新增 50 行**
  `impl ApprovalGate for ApprovalWorkflow`（4 个方法，逐方法委托）。
  **无一处 `assess_risk` / 规则引擎 / 短路顺序 / 审计 / 超时默认值的语义改动。**
  测试文件同理：`git diff --no-index` HEAD `approval_tests.rs` vs 插件 `approval_tests.rs`
  = **6 增 / 16 删**，只有 import 头（2→6 行）与 `test_is_user_confirmation`（留 core，10 行）
  两处；**23 个用例的函数体逐字未动**。
- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**；`tianyan-core --lib` **1176 passed**（= 基线 1199 − 23 搬走的用例）；
  `tianyan-policy-approval` **23 passed**；`tianyan-server --lib` **189 passed / 1 ignored**
  （187 + 2 条新插槽测试）、`--tests` 5 / 14 / 1 / 2 全绿；`tianyan-app` 7 passed；
  `cargo catalog-check` OK；`cargo tree -p tianyan-policy-approval --edges normal --depth 1`
  = core / async-trait / chrono / tokio / tracing / uuid，**无 `tianyan-server`**。
- **测试守恒**：HEAD core 1199 = 现在 core 1176 + 插件 23（其中 23 条为迁移，逐个同名同断言；
  另 1 条 `test_is_user_confirmation` 测的是 core 的纯函数，留在 core）。**零放弃、零降级**。
- **遗留**：
  1. `ApprovalGate` 的 4 个方法里有 2 个（`status` / `record_user_confirmation_by_fingerprint`）
     只是把既有固有方法原样暴露——期 3 结束时若确认"审批状态快照"该由协调器自己组装，
     可把 `status()` 收回成 3 个方法的更窄契约（先按实际调用面留着，别提前抽象）；
  2. `AgentStatus` 里的 `pending_confirmations` 由协调器覆写，等于插件返回的该字段恒空——
     属"契约里有一个字段实现不拥有"的小瑕疵，若要消除需引入单独的数据结构
     （目前只有一个消费者，不值当）；
  3. `plugins/policy-approval` 与 `core` 的 `workflow.rs` 存在**文件级耦合**（`#[path]` 的
     路径硬编码在 `core/src/executor/approval/mod.rs`）：改插件文件布局须同步，否则 core
     测试直接编译失败——这条既是守卫也是负担（与序 4 同型）。

**实施记录（期 3-T2：policy-context，2026-10-21）**：`core/src/context/` 的
**策略面整体迁入** `plugins/policy-context`（crate `tianyan-policy-context`，14 个公开
符号）——`pipeline.rs`（395 生产行）、`compression/mod.rs`（509）、`retrieval/`
（808：intent 387 + retriever 292 + loader 59 + types 54 + mod 16），合计 **1,729 生产行
+ 1,755 测试行**。**`assembler.rs`（341 生产行）按普查结论留 core**（D7 原文已修订，见
「★ 推翻前提 1」）。这是**期 3 的主体收益**：改 prompt 模板 / 压缩阈值 / 检索 RRF 参数 /
意图分类关键词，此前要重编 core（42.4K）+ server（13.4K），现在只重编本插件 + 装配包。

- **★ 新增 1 个 pub trait `ContextPort`**（`core/src/context/port.rs`），本批只此一个
  （期 3 至今净增 pub trait = 2 ≤ 反指标 3）。方法集按**真实调用点**最小化到 3 个：

  | 方法 | 内核侧调用点 | 说明 |
  |---|---|---|
  | `load_injectable(query, workdir)` | `agent_core.rs:478` `ensure_injectable` | 加载可注入前缀（soul + AGENTS.md + learned rules + memories）；调用方按会话缓存，`soul` 非空即视为已加载 |
  | `compress_for_session(messages, session_id, recent_input_tokens, force, existing_summary)` | `agent_core.rs:783` `maybe_compress_and_persist` | 判定+执行压缩，产出带 `compression_marker` 的摘要消息；`None` = 不需要压缩 / 空摘要丢弃 |
  | `extract_marker_summary(marker)` | `agent_core.rs:751` 同函数（取「本会话已有摘要」） | 从链上最后一个压缩点剥出纯摘要文本，作增量摘要输入 |

  **★ 第三个方法是勘察后补的**（任务书初稿只列了 2 个）：普查漏掉了
  `agent_core.rs:751` 的 `ContextPipeline::extract_marker_summary(&sm)` —— 静态关联函数
  调用，随类型搬走就会断。**为什么不把它留 core 当纯函数**：摘要的包装格式
  （`[对话摘要] … [摘要结束]`）由 `compress_for_session` **写出**，剥壳算法若留 core，
  改一句摘要话术就会让「已有摘要」静默提取失败（增量摘要退化为从零摘要，不报错、只是
  丢上下文）。产生者与消费者因此必须同 crate——与 T1 的 `action_fingerprint` 同理，只是
  方向相反（那次是提到 core 共享，这次是留在插件共享）。
  **没有进 trait 的方法**：`run()` / `compress_if_needed()`（前者 `#[cfg(test)]`，后者只
  被 `compress_for_session` 内部调用）、`compressor()`（只有测试用）、`fallback_load_rules()`
  / `load_memories()`（私有）——「没有调用点的方法不进 trait」。

- **★ 不抽 `UsageReporter` trait（勘察建议，本批裁决不做）**：`retriever.rs` 持
  `Option<Arc<UsageStats>>` 直接写 SQLite 统计，普查曾建议先给 kernel-api 一个
  `UsageReporter` 才不违 D2-1。**本批不改**，理由：① 今天 core 就是唯一的 kernel 包，
  所有插件（`storage-sqlite` / `task-builtin` / `task-evolution` / `policy-approval` /
  `tools-*`）都直接依赖它，仓库里根本没有「kernel-api vs kernel-impl」这条缝；
  ② `UsageStats` 只有一个实现，此刻抽 trait 无法用第二实现压测形状——YAGNI，形状错了要等
  有第二个实现才暴露，那时改 trait 的成本远高于现在；③ 正确时机是 **T5 切
  `kernel-api` / `kernel-impl`**：届时 `UsageStats` 归属与 `WorkingSetPort` 一起定，
  才有真实的 seam 需求。插件因此直接 `use tianyan::observability::usage_stats::UsageStats`。

- **★ 顺序契约留在内核（不搬代码、只留语义）**：`agent_core.rs:807-818` ——
  `compress_for_session` 返回的 `StructuredMessage` 带 `compression_marker`，**只有摘要
  落库成功后**才允许 `ws.reshape_after_compression()`（ADR-035 §2 段化）。该顺序在
  `Agent::maybe_compress_and_persist` 里执行；插件只产出压缩结果，连 `WorkingSetRegistry`
  都拿不到（`ContextPort` 签名里没有它）。顺序颠倒会推进消费水位 → 把尚未注入的异步通知
  误标为已消费 + 段起点前移到库中不存在的压缩点。这条已写进 `ContextPort` 与插件 crate
  文档（防止后来者「顺手」把收缩搬进插件）。

- **装配：`AssemblyServices` 第六个插槽 [`ContextFactory`]**（`Arc<dyn Fn(&ContextServices)
  -> Arc<dyn ContextPort> + Send + Sync>`，**同步**：`ContextPipeline::new` /
  `ContextCompressor::new` / `DualLayerRetriever::new` 都只存字段）。**能力集合
  `ContextServices` 定义在 core**（`core/src/context/assembly.rs`，server 侧 re-export）：
  `config`（`default_top_k` / `learned_rules_top_k`）、`vfs`、`usage_stats`、
  `chat`（`ModelServices.chat` 的同一份 Arc）、`chat_model`、`chat_spec`（压缩窗口联动源）。
  理由与 `TaskServices` 同型：**三方**同时引用（server 消费 / 装配包生产 / 插件读取），
  住在 server 会让插件反向依赖 server（破 D2-1）。**「怎么配」进插件**：`context_window`
  与模型规格联动、`preserve_recent_messages = 6`、`summary_model`、两个 `top_k` 全部收进
  `tianyan_policy_context::pipeline()`——迁移前它们散在 `AgentBuilder::build`（core）与
  `server/src/agent_builder.rs`。`AgentBuilder` 因此**删掉 `retriever` 字段与
  `with_retriever`**（那是插件类型进内核的唯一入口），新增 `with_context_port`；
  `Agent.context_pipeline` 的类型由 `ContextPipeline` 变为 `Arc<dyn ContextPort>`（字段名
  保留，最小化 diff）。三处装配（`app` / `tauri` / `server/tests/common/factory.rs` 的
  `test_assembly()`）同形同源各注入一行 `Arc::new(|s| tianyan_policy_context::pipeline(s))`。

- **★ 未装配降级 = core 的 `NullContextPort`（空直通），且 `start_server*` 加**启动期硬守卫****：
  `load_injectable` 返回空可注入上下文、`compress_for_session` 恒 `None`、
  `extract_marker_summary` 恒 `None`，装配期 `warn!` 指路。**与任务书初稿的偏差**：初稿
  建议「`compress_for_session` 原样返回最后一条消息」——**不能这么做**：`Some(…)` 会被
  `maybe_compress_and_persist` 当作真的发生了压缩，落一个 `compression_marker` 进会话链、
  再收缩工作集段，等于用最后一条用户消息顶替摘要（历史被静默丢弃）。降级只能「什么都不做」。
  **为什么比 T1 严（硬守卫而不是只 `warn!`）**：审批缺失只是少一层策略复核；上下文缺失
  是「整个 system 前缀没了 + 上下文只增不减」，服务起得来但行为已不成立。守卫放在
  `start_server_inner` 入口（与存储同一手法，`has_context_factory()`），失败发生在启动前；
  `create_app`（构造对象图，47 处 lib 单测 + 嵌入式）**不设守卫**、仍允许空直通——两条路径
  严格度不同，判定点分开。各有红/绿判据：
  `test_start_server_requires_context_factory` / `test_missing_context_port_falls_back_to_null_port`
  （删守卫或把兜底改成 panic 即红）。

- **★ builder 的两个窗口测试：断言跟着逻辑走，强度不降**。原
  `core/src/agent/builder.rs` 的两条用例靠
  `agent.context_pipeline.compressor().lock().await.get_status(0).context_window` 穿透具体
  类型断言「chat_spec ↔ 压缩器窗口联动」（`compressor()` 全仓只有这两处用；GUI 的
  `context_window` 是 `server/src/api/chat/handlers.rs:66` 从 config 重新派生，不走管线）。
  管线搬走后 core 只剩 `Arc<dyn ContextPort>`，处理如下：
  1. **联动断言迁到插件**（`plugins/policy-context/src/lib.rs::pipeline_tests`）：
     `build_pipeline(&services).compressor().lock().await.get_status(0).context_window ==
     spec.context_length`（有 spec）与 `== 128_000`（无 spec）——逐字保留原来的精确相等断言。
     它测的就是管线行为，本就该归插件；
  2. **core 侧改测接线**：`test_with_chat_model_spec_injects_spec_into_loop`（`AgentLoop.chat_spec
     == Some(spec)` + 注入的 port 就是装配层给的那一个）与
     `test_no_chat_model_spec_keeps_loop_spec_none`。识别「装配的是哪个 port」用契约里
     **本来就有**的 `extract_marker_summary` 当身份探针（假端口回 marker 字符串）——
     **没有**给 `ContextPort` 加任何只服务测试的方法（契约污染），也**没有**删掉原断言。

- **测试：同源包含，core 计数不变、判别力不减**。core 的单测必须构造**真实**管线：12 个
  `agent_core.rs` 用例锁「压缩落库 → 刷新注入缓存 → 工作集段收缩」的端到端语义，替身会把
  它们抹平成「永远不压缩」。`[dev-dependencies]` 行不通（core 编译两份 → `ContextPort` 出现
  两个不兼容实例，同序 4）。因此照 `storage-sqlite` 先例：`core/src/context/mod.rs` 在
  `#[cfg(test)]` 下用 `#[path]` **同源包含** `plugins/policy-context/src/context/{pipeline.rs,
  compression/mod.rs, retrieval/mod.rs}`，并把模块树**摆成与插件一致的形状**
  （`context::{pipeline, compression, retrieval}`）——被包含文件里的 `crate::context::…`
  于是两边都成立（这是本序相对 T1 的一处手法改进：T1 的 `workflow.rs` 是单文件，没有
  兄弟模块引用问题）。两份**纪律**写进插件 crate 文档与 AGENTS.md：① 这些文件只许
  `tianyan::…` 与 `crate::context::…`；② 它们的测试模块在 core 也会跑一遍（与序 4 先例
  一致：`--lib` 计数不变、插件侧独立再跑一遍）。
  - **测试守恒**：HEAD core `--lib` **1176** = 现在 core **1177**（其中 **65** 条来自
    `#[path]` 同源包含：pipeline 16 / compression 17 / retrieval 31+1）+ 插件 **68**
    （65 条同源重跑 + 3 条新装配门禁）**+1**（core 新增
    `test_missing_context_port_falls_back_to_null_port`）、**−2**（两条窗口测试的 core 版本
    被接线测试替换，窗口断言移入插件）。**零放弃、零降级**。
  - **`ContextPipeline::compressor()` 由 `#[cfg(test)] pub(crate)` 提为 `#[cfg(test)] pub`**：
    同源包含后在 core 构建里它是「未使用的方法」→ `dead_code` 告警（零 warning 门禁会红）。
    提成 `pub` 后可见性图判定为「外部可达」，两边都不告警；公开面影响为零
    （`#[cfg(test)]` 只在测试构建存在）。
  - **删除 `core/src/test_utils::create_test_retriever`**（返回 `DualLayerRetriever`）：
    `test-utils` 是**特性构建**（不是 `cfg(test)`），拿不到同源包含模块，core 也不能反向
    依赖插件。删除依据：全仓**零调用者**（只有定义处 + `retriever_tests.rs` 里的同名局部
    helper，后者自带脚手架、与本函数无关）。**不是放弃测试**，是删死脚手架的公开面。

- **逐行比对证据**（`git diff --no-index`，HEAD 原文件 vs 插件文件）：
  | 文件 | 改动行 | 归类 |
  |---|---|---|
  | `pipeline.rs` | 62 增 / 14 删 | ① `crate::` → `tianyan::`（core 类型）**12 处**（含测试）；② rustfmt 因 `tianyan::` 比 `crate::` 长而重排 import 块；③ `compressor()` 可见性 + 3 行文档；④ **末尾新增 44 行** `impl ContextPort for ContextPipeline`（逐方法委托）；⑤ 测试里一行 import 因变长折行 |
  | `compression/mod.rs` | 9 / 9 | 全部是路径前缀（含 `pub use tianyan::common::token_estimator::{…}` 与测试 import） |
  | `retrieval/retriever.rs` | 7 / 7 | 全部是路径前缀（`UsageStats` / `Result` / `system_paths` / `parse_doc_index` / `ContextNamespace`） |
  | `retrieval/intent.rs` | 2 / 2 | 路径前缀 |
  | `retrieval/loader.rs` | 3 / 3 | 路径前缀（`parse_doc_index` / `render_doc_index`） |
  | `retrieval/types.rs` | 1 / 1 | 路径前缀 |
  | `retrieval/mod.rs` | **0 / 0** | 逐字节相同 |
  | `retrieval/retriever_tests.rs` | 14 / 14 | 路径前缀 + 1 处 rustfmt 折行（`.with_category("agent")`） |
  **`DEFAULT_SUMMARY_PROMPT` / `INCREMENTAL_SUMMARY_PROMPT` 全文、`compression_threshold =
  0.6` / `preserve_recent_messages = 10` / `min_messages_to_compress = 6` / `max_summary_tokens
  = 4000` / `CRITICAL_THRESHOLD_RATIO = 0.8` / `memory_bias = 1.15` /
  `DEFAULT_FRESHNESS_WEIGHT = 0.2` / `score > 0.6` / 意图分类关键词表 / 全部日志文案
  （`"上下文压缩完成"` / `"压缩摘要为空（模型空响应），本次压缩丢弃，下轮重试"` /
  `"意图分析失败，使用默认意图回退"` / `"加载核心提示词失败"` / `"已加载项目指令（AGENTS.md）"`
  / `"Retrieval completed"` …）逐字未动。**

- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**；`tianyan-core --lib` **1177 passed**；`tianyan-policy-context`
  **68 passed**；`tianyan-server --lib` **192 passed / 1 ignored**（189 + 2 条新插槽测试 +
  1 条启动守卫测试）；`--tests` 5 / 14 / 1 / 2 全绿；`tianyan-app` 7 passed；
  `cargo catalog-check` OK；`cargo tree -p tianyan-policy-context --edges normal --depth 1`
  = core / async-trait / chrono / serde / tokio / tracing，**无 `tianyan-server`**。
  （`tianyan-core --lib` 首跑曾有一条 `executor::command::tests::test_background_list_and_unknown`
  偶发失败——shell 后台命令并发用例，与本次改动无关，单跑与重跑均绿。）

- **遗留**：
  1. **`UsageReporter` trait 未抽**（本批裁决，见上）：T5 切 `kernel-api`/`kernel-impl` 时
     与 `WorkingSetPort` 一起做——那时才有第二实现与真实 seam 需求；
  2. `plugins/policy-context` 与 `core` 的**文件级耦合**（`#[path]` 硬编码在
     `core/src/context/mod.rs`）：改插件文件布局须同步，否则 core 测试直接编译失败
     （与序 4 / T1 同型的守卫兼负担）；
  3. `Agent.context_pipeline` 字段名沿用了旧称（类型已是 `Arc<dyn ContextPort>`）：改名会
     扩散到 `agent_core.rs` 的十余处调用，等 T5 切包时与 `WorkingSetPort` 一并整理；
  4. `null` 兜底的**可观测性只到日志**：`create_app` 路径若长期误配，症状是「模型没有
     人设 + 上下文无限增长」。`start_server*` 已有硬守卫覆盖生产形态，`create_app` 侧
     未加运行期指标（47 处单测的可观测面不宜再动，留给期 4 清单收敛时统一）。

**实施记录（期 3-T3：policy-review，2026-10-21）**：两个评审面整体迁入
`plugins/policy-review`（crate `tianyan-policy-review`）——`core/src/executor/judge.rs`
（322 行 = **200 生产行** + 122 测试行）与 `core/src/skills/reviewer.rs`（303 行 =
**275 生产行** + 测试声明）+ `core/src/skills/reviewer_tests.rs`（164 行），合计
**475 生产行搬出 core**。这是期 3 最小的一刀，也是「改 prompt 不重编内核」的最后一块
拼图：此前调一句判定提示词、改一条评分标准、换一次评审模型，都要重编 core（42.4K）
+ server（13.4K）。

- **契约留 core（两处，都是「trait 签名 + 调用点证据」）**：

  | 留 core | 位置 | 为什么 |
  |---|---|---|
  | `TaskReviewer` / `Judgment` / `Verdict` | **新建** `core/src/executor/review.rs`（原 `judge.rs` 删除） | 内核以 `Arc<dyn TaskReviewer>` 消费：`agent/background.rs:280` 字段 / `:364` 注入器、`agent/tool_registry/mod.rs:666`、`AgentBuilder::with_task_reviewer`；`Judgment`/`Verdict` 在 trait 签名里。搬走 ⇒ 内核反向 `use tianyan_policy_review::…`（破 D2-1/D2-3） |
  | `SkillReviewPort` / `SkillReview` / `REVIEWS_PREFIX` | **新建** `core/src/skills/review.rs` | 见下两条 |

  模块改名（`judge` → `review`）不是审美：`judge` 这个名字原本同时覆盖「契约 + 实现」，
  实现走后留一个叫 `judge` 的空壳会持续误导（与 T1 把 `gate.rs` 从 `workflow.rs` 里切
  出来同一条理由）。代价是 4 处 `crate::executor::judge::` 路径改写（`background.rs` 3 处、
  `tool_registry/mod.rs` 1 处），零语义改动。

- **★ 新增 1 个 pub trait `SkillReviewPort`**（`core/src/skills/review.rs`），本批只此
  一个（期 3 至今净增 pub trait = **3**，正好压在反指标线上）。方法面按**真实调用点**
  最小化到 **1 个**：

  | 方法 | 调用点 | 说明 |
  |---|---|---|
  | `latest_review(&self, skill_id) -> Result<Option<SkillReview>>` | `server/src/api/skills/handlers.rs`（技能统计面板「每个技能的最新一条评审」） | 只读；读 `skill/_reviews/<id>.jsonl` 的最后一行 |

  **`review_conversation` 与 `load_reviews` 没有进 trait**：`review_conversation`
  （约 160 行，产生评审的完整逻辑）**零生产调用者**（只有自身测试）——ADR-017 收敛掉
  「记忆提取任务」后，它原本顺路调用的挂点消失，于是**读侧已接线、写侧未接**（GUI 能
  看到评审、却永不产生新评审）。这是「未接线」而非死代码，故保留在插件的
  `SkillReviewer` 具体类型上：**不进契约**（契约里放一个零调用方法等于让未来的实现者
  对着空气实现）、**不删**（删掉等于把技能质量反馈这条路封死）、**不改 mock**。

- **★ `REVIEWS_PREFIX` 必须留 core（一个口径两个消费者）**：它既是插件的落盘路径
  （`tianyan://skill/_reviews/<id>.jsonl`），也是内核技能发现时的过滤条件
  （`skills/manager.rs:43` `if id == crate::skills::REVIEWS_PREFIX` —— 不滤掉的话
  `_reviews` 会被当成一个「技能」列给用户）。与 T1 的 `action_fingerprint`、T2 的
  `extract_marker_summary` 是同一类判断：**产生者与消费者必须同源**。

- **`common/llm_judge.rs` 原样不动**（§B.5 第 4 条，本次实测复核）：6 个消费者里 4 个
  非评审——`vfs/summary/engine.rs:119`（内核摘要解析）、`agent/background.rs:42`
  （任务通知截断）、`agent/tool_registry/evolution_ops.rs:13`（演化综述）、
  `plugins/tools-web/src/web.rs:24`（抓取正文截断）。移进 policy-review 会让内核
  **反向依赖策略插件**；插件只是 `use tianyan::common::llm_judge::…`。

- **`skills/manager.rs` + `SkillManager` 留 core（3a 型业务复用，本批不动）**：
  `server/src/state.rs`（字段 + 构造）与 `server/src/api/skills/services.rs:17/23` 直接持
  具体类型干活（技能列表/详情/`call_skill`）——不是装配接线，是 HTTP 业务直接调实现。
  按 D5 的判据它终将需要 `SkillCatalogPort`，但那会连带 `call_skill` 工具链
  （`agent/tool_registry` → `skills::SkillManager` 单向依赖），超出本批 475 行的范围。

- **★ 与任务书初稿的偏差：插槽是两个（`TaskReviewerFactory` + `SkillReviewFactory`），
  不是「只加一个 `skill_review_factory`」**。理由是硬的：`LlmTaskReviewer` 的**构造点
  不在 server**，而在 core 的 `AgentBuilder::build`（`background_self_review` 为真时
  内联 `LlmTaskReviewer::new(model_service, &chat_model)`）。实现搬走后，builder 若没有
  注入路径，就只能把实现留在 core——那等于没搬。因此第二个插槽是本次搬移的**必要配套**，
  形状与 T1 的 `ApprovalGateFactory` 同款（`Arc<dyn Fn>`、同步、可重复调用）：

  | 插槽 | 形状 | 未装配时 |
  |---|---|---|
  | `TaskReviewerFactory`（第七个） | `Arc<dyn Fn(Arc<dyn ChatService>, &str /*model*/) -> Arc<dyn TaskReviewer>>` | `AgentBuilder` 得到 `None` → `background_self_review` 为真时也不装配自审器（通知直接投递）+ **装配期 `warn!`** |
  | `SkillReviewFactory`（第八个） | `Arc<dyn Fn(Arc<dyn ChatService>, &str, Arc<dyn VirtualFileSystem>) -> Arc<dyn SkillReviewPort>>` | `AppState::create_skill_reviewer` 返回 `None` → 技能统计的 `reviews` 为空数组（**响应结构不变**） |

  「是否自审」（`config.background_self_review`）**仍由内核判**——那是「要不要花这次
  模型调用」的成本开关（内核行为），不是「怎么审」（插件策略）。插件只回答后者。

- **降级语义的取舍（两处都允许降级，都不静默）**：自审缺失与审批缺失同一档（少一层
  质量复核；且该开关默认关闭 = opt-in）；复审读端口缺失在 `GET /api/v1/skills/stats`
  上**不改变可观测形状**（今天「没有评审器」与「有评审器但没有记录」都表现为
  `reviews: []`）。两处都有判据：装配期 `warn!`（自审）与 `server/src/assembly.rs` 的
  插槽测试（默认空 / 注入即用 / 可重复调用）。
  **`create_skill_reviewer` 的签名从 `TianyanResult<Arc<SkillReviewer>>` 变成
  `TianyanResult<Option<Arc<dyn SkillReviewPort>>>`**（仍保留 `Result`：取共享模型服务
  失败时照旧向上抛，与迁移前一致）；`handlers.rs` 在 `None` 时**跳过附加字段**而不是
  造一个空实现——响应结构完全不变，也不会凭空产生一个假实现。

- **逐行比对证据**（`git diff --no-index`，HEAD 原文件 vs 插件文件）：

  | 文件 | 改动行 | 归类 |
  |---|---|---|
  | `judge.rs` | 25 增 / 62 删 | ① 契约块（`Verdict` / `Judgment` / `TaskReviewer`，40 行）移出 → 换 4 行 `use tianyan::executor::review::…`；② `crate::` → `tianyan::`（imports + `run_judge` 里的 `crate::common::types::Message` + 测试 4 处）；③ 模块文档改写（+9 行，说明迁移动机与「逐字未改」清单）；④ 测试：`test_verdict_is_pass`（9 行）留 core，两段分节注释（+7） |
  | `reviewer.rs` | 30 增 / 33 删 | ① `SkillReview` 结构 + `REVIEWS_PREFIX` 常量（28 行）移出 → 换 1 行 `use tianyan::skills::{SkillReview, SkillReviewPort, REVIEWS_PREFIX}`；② `crate::` → `tianyan::`（7 处）；③ 模块文档（+10 行：迁移 + 「未接线遗留」说明）；④ **末尾新增 9 行** `impl SkillReviewPort for SkillReviewer`（1 个方法，委托给同名固有方法，用完全限定语法） |
  | `reviewer_tests.rs` | 6 / 6 | **全部是 `crate::` → `tianyan::`**（5 处 import + 2 处内联路径），2 个用例的函数体逐字未动 |

  **判定提示词全文（`build_task_review_prompt` / `SKILL_REVIEW_PROMPT`）、`VERDICT:`
  扫描顺序（先 `PASS` 再 `NEEDS_CHANGES`）、退出码回退、`REASON:` 缺失文案、
  「自审不可用，默认通过」、空结果短路（`"任务无结果输出"`）、评分标准「4+3+3」、
  截断预算（2000 / 300 / 400）、`max_reviews_per_conversation = 3`、保留最近 `50` 条、
  日志文案（`"技能使用评审完成"` / `"技能评审记录存储失败"` / `"技能评审调用失败"` /
  `"技能评审响应解析失败"` / `"技能评审缺少 score"` / `"LLM 判断调用失败，回退"`）
  逐字未动。**

- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**；`tianyan-core --lib` **1166 passed**（= 基线 1177 − 11 条随迁用例）；
  `tianyan-policy-review` **14 passed**（11 迁移 + 3 装配门禁）；`tianyan-server --lib`
  **194 passed / 1 ignored**（192 + 2 条新插槽测试）、`--tests` 5 / 14 / 1 / 2 全绿；
  `tianyan-app` 7 passed；`cargo catalog-check` OK；
  `cargo tree -p tianyan-policy-review --edges normal --depth 1`
  = core / async-trait / serde / serde_json / tokio / tracing，**无 `tianyan-server`**。
- **测试守恒**：HEAD core 1177 = 现在 core **1166** + 插件 **14**（11 条为迁移，逐个同名
  同断言：judge 9 = 4 条 G4 自审 + 3 条判定解析 + 2 条截断预算；reviewer 2 = 会话证据
  评审 + 无 `call_skill` 空返回；另 3 条为新增**装配门禁**：工厂产出的自审器空结果不花
  调用 / 复审端口从 `_reviews/<id>.jsonl` 取最新且空 VFS 回 `None` / 契约方法委托给同
  一份读取逻辑）**− 1**（`test_verdict_is_pass` 测的是 core 的 `Verdict`，留 core）。
  算式：1177 − 11 = 1166；1166 + 14 = 1180 = 1177 + 3（新增）。**零放弃、零降级**。
- **遗留**：
  1. **`review_conversation` 待接线**（本批最需要留档的一条）：技能评审的**写侧**没有
     生产调用者，面板因此只能显示历史记录、永不更新。接线点应选在**已有模型调用**的
     位置（会话收尾 / 演化任务）以免多一轮成本；接线时它已经在 `SkillReviewer`
     （具体类型）上，不必动契约。
  2. **`skills/manager.rs` + `SkillManager` 仍在 core**（3a 型业务复用）：要走得先抽
     `SkillCatalogPort`（列表 / 详情 / 读取），并连带处理 `call_skill`
     （`agent/tool_registry` → `skills::SkillManager` 单向依赖）。与普查结论「`skills`
     部分可搬」一致，留待期 4。
  3. **插槽增至 8 个**：`AssemblyServices` 正在变成「所有插件能力的平铺表」。T2 的遗留 2
     （`TaskServices` 9 字段）与本条是同一件事的两面——期 4 清单收敛时应按插件分组
     （每个插件一份能力接口），而不是继续往同一个结构上加字段。
  4. `SkillReviewPort` 只有**读**一面；若未来把「产生评审」也接成契约，应新增
     `record_review` 之类的方法而不是把 `review_conversation` 整个塞进 trait（后者会把
     会话解析、`call_skill` 参数形状等内核知识灌进插件的契约面）。

**实施记录（期 2 序 6 补做：provider-llm + provider-embed，2026-10-21）**：
ADR-049 期 2 的序 6（当时跳过、留待最后）在期 3-T3 之后补做——它是期 3-T4
「config 二分」的前置（模型方言 950 行要随 `provider-llm` 走，必须先有那个 crate）。
**本节同时更正一处历史标注**：期 2 行此前写的「期 2 完成」是错的（序 6 未做），
现已改为「期 2 全部完成（序 0–6）」。

规模：`core/src/model/provider/`（2,805 行）+ `services.rs`（232 行）搬出 core，
合计 **3,037 行**（其中生产行约 1,850、测试 58 条）。落地形态是**三个 crate**：

| crate | 内容 | 行数 | 测试 |
|---|---|---|---|
| `plugins/provider-client` | `AsyncOpenAIClient`（`client.rs` 378）+ `RetryableFailure` / `classify_http_status`（`failure.rs` 40）+ crate 文档 | 468 | 9（全部迁移） |
| `plugins/provider-llm` | `chat.rs` 1672 / `vision.rs` 399 / `discovery.rs` 68 / `logging.rs` 92（聊天+视觉装饰）/ `client.rs` 67（newtype）/ `lib.rs` 329（工厂 + 探针 + 装配门禁） | 2,627 | 44（39 迁移 + 5 新增） |
| `plugins/provider-embed` | `embedding.rs` 463 / `logging.rs` 370（装饰 + 用量上报 + 查询缓存）/ `lib.rs` 196（工厂 + 装配门禁） | 1,029 | 13（10 迁移 + 3 新增） |

- **★ 为什么是三个 crate 而不是两个（本序唯一的结构性裁决，理由是硬的）**：

  1. **孤儿规则**：`impl EmbeddingService for AsyncOpenAIClient` 只能写在「类型或 trait
     属于本 crate」的地方。`EmbeddingService` 在 core、`AsyncOpenAIClient` 若在
     `provider-llm`，则 `provider-embed` 里两样都是外部类型——直接 `impl` 被编译器拒绝
     （实测 `E0117`/`E0116`）。而聊天 / 视觉 / 嵌入**共用同一个客户端**。
  2. **一次构造、全路径生效**（`client.rs` 迁移前的注释逐字保留）：UA 单点（ADR-046）、
     方言解析一次（ADR-038）、双预算超时（ADR-044）、动态会话头、`RetryPolicy`。复制一份
     构造逻辑＝两份口径同时漂移（`RepoMapCache` / `action_fingerprint` /
     `extract_marker_summary` 都是同一条判据的产物）。
  3. **D2-2 插件间零互赖**：若让 `provider-embed` 依赖 `provider-llm`（或反过来），改一行
     嵌入实现就要重编聊天插件，插件间出现直接 `use`——两条都是本 ADR 明令禁止的。

  D7 的判据也支持这个切法：**客户端构造**（UA / 超时 / 方言 / 动态头 / 重试）是第三种
  「变化原因」，与「聊天怎么发请求」「嵌入怎么解析 usage」都不同，因此值得独立成 crate。
  反指标复核：`provider-client` 468 行（≥300 ✅）、恰有 **2 个真实消费者**（✅）、公开符号
  3 个（`AsyncOpenAIClient` / `RetryableFailure` / `classify_http_status`，≤15 ✅）。
  `provider-llm` 公开符号 4 个（`services` / `ChatClient` / `LoggedService` /
  `LoggedVlmService`），`provider-embed` 2 个（`services` / `LoggedEmbeddingService`）。

- **★ 两个能力插件各用 newtype + `Deref` 满足孤儿规则，行为零改动**：
  `provider-llm::ChatClient(AsyncOpenAIClient)`、`provider-embed::EmbeddingClient(...)`。
  二者都 `impl Deref<Target = AsyncOpenAIClient>`：**字段与方法访问因此逐字保持**——
  `self.base_url` / `self.http` / `self.headers` / `self.dialect` / `self.retry_policy`
  / `self.send_chat_request(...)` / `self.thinking_param_for(...)` 全部不用改写成
  `self.0.…`（Rust 的字段访问会沿 `Deref` 链自动解引用）。构造语义也**零复制**：
  `ChatClient::from_provider` / `EmbeddingClient::new` 直接转发给
  `AsyncOpenAIClient::{from_provider, new}`。客户端字段因此从 `pub(crate)` 提为 `pub`
  （`service_name` / `client` / `http` / `base_url` / `api_key` / `headers` /
  `retry_policy` / `read_idle` / `first_token` / `dialect` / `model_thinking_params` /
  `dynamic_headers`，全部补了文档——`missing_docs` 是 workspace 级 warn），`thinking_param_for`
  与 `finish_reason_str` 同样提公（跨 crate 调用）。这是本序唯一的公开面扩张，且限于插件。

- **哪些类型留 core（逐条理由）**：

  | 留 core | 位置 | 为什么 |
  |---|---|---|
  | `ChatService` / `EmbeddingService` / `VlmService` / `ServiceDiscovery` | `model/traits.rs`（路径不变） | S 级契约；内核 / 通道层 / 其它插件都以 `Arc<dyn …>` 消费 |
  | 载荷（`ChatCompletionRequest` / `ChatCompletionChunk` / `VisionRequest` / `EmbeddingResponse` …） | `model/types/` | 出现在 S 级签名里，`agent` / `server` / 多个插件共用 |
  | `spec.rs`（739 行） | `model/spec.rs` | 模型规格 + 上下文窗口：内核动态 `max_tokens`、`policy-context` 压缩窗口联动、通道层配置面板三方共用 |
  | `retry.rs`（365 行） | `model/retry.rs` | 请求层策略被**两个** provider 插件共用；「429/5xx/网络可重试」是跨 provider 的统一约定（ADR-014） |
  | `config/model.rs` 全部方言数据（`ProviderDialect` / `DialectPreset` / `ThinkingParam` / `ThinkingField` / `CacheField` / `EmbeddingUsageShape` / `HeaderSource`） | `config/` | 它们是 `TianyanConfig` 的**字段形状**，随内核加载 / 校验 / 热重载；方言 950 行的外移属期 3-T4（config 二分），不在本序 |
  | `EmbeddingUsageSink` / `EmbeddingUsageHandle` | **新建** `model/usage.rs` | 实现方是通道层（`UsageLogEmbeddingSink` 写 `usage_logs`）、消费方是插件（`LoggedEmbeddingService`）→ 契约必须在两者之上 |
  | `Unconfigured{Chat,Embedding,Vlm}Service` | **新建** `model/unconfigured.rs` | 「能力未配置」的兜底有**两条路径**（未注入插件 / 配置缺能力），必须同一句文案与同一错误类别；插件与 server 都要用 |
  | `ProviderLlmServices` / `ProviderEmbedServices` / `ConnectionProbe` | **新建** `model/assembly.rs` | 装配原语：server 消费、装配包生产、插件填充三方共用（与 `TaskServices` / `ContextServices` 同一条纪律：住 server 会让插件反向依赖通道层） |

- **★ `ModelServices`（232 行容器 + 构造器）搬进 `server/src/model_services.rs`**：
  core 内**零生产使用者**（只有 `model/mod.rs` 的导出 + `context/assembly.rs` 一句注释），
  真正消费者是通道层（`create_model_services` / `AppState::shared_model_services`）。
  按 D5 它是装配职责：三种能力今天分属**两个**插件插槽，容器只能活在认识插槽的地方。
  能力解析、客户端池复用、降级服务、`chat_for` 的路由回落、`set_embedding_usage_sink`
  的后置注入、`chat_clients()` 全部逐字保留；`from_config(&ModelsConfig)`（async）变为
  `from_factories(&ModelsConfig, Option<&ProviderLlmFactory>, Option<&ProviderEmbedFactory>)`
  （sync——客户端构造不发请求）。`AppState::new` 因此多两个工厂参数（9 个，加
  `#[allow(clippy::too_many_arguments)]` 并就地注明「期 4 清单收敛时应按插件分组」）。
  「部分模型能力未配置」那条 `warn!`（三个 `*_configured` 字段）随容器留在 server——它是
  唯一同时看得见三项能力的地方，**文案与字段名逐字未动**。

- **★ 连通性探测契约化：选「由工厂造临时客户端」，不选「独立探针插槽」**
  （`POST /api/config/test-connection` 迁移前直接 `tianyan::model::AsyncOpenAIClient::new(...)`
  ——具体类型搬走后那句代码不存在）：
  - 新增 core trait `ConnectionProbe`（1 方法：`probe(endpoint, api_key, model) ->
    Result<Vec<String>>`，**async**），实现 `ProviderConnectionProbe` 在
    `plugins/provider-llm`，随 `ProviderLlmServices::probe` 由工厂产出；
  - **为什么不单开第 11 个插槽**：探针不是「一次性装配产物」，而是「用同一套客户端构造
    语义造临时客户端」的能力——独立插槽会让装配层多一个必填项，也让第二个工厂去重复
    实现那五条语义；挂在能力集合里，插槽数保持 **10**（与任务书一致）；
  - **为什么不是「用已配置的 provider 实例」**：探测用的是用户在配置页**当场输入**的
    endpoint / api_key，此时它们还没落盘，不存在对应实例；
  - 请求形状逐字保留：`timeout = 30` / `first_token_timeout = 300` /
    `system("You are a helpful assistant.")` / `temperature = 0.7` / `max_tokens = 10` /
    `stream = false`，成功返回 `vec![model]`（**不列目录**——列目录是 `ServiceDiscovery`）；
  - **错误分类仍留通道层**：`classify_connection_error` 用 `is_permission` /
    `is_not_found` / `is_timeout` 语义谓词（ADR-014），其单元测试原样保留；
    `test_connection` 改为取 `State<Arc<AppState>>` → `shared_model_services().probe()`。
  - **唯一可观测差异（如实记一条**）：迁移前「客户端构造失败」（例：`http://` 无主机）
    走的是**未经语义分类**的早退分支，文案是「连接失败: 创建客户端失败: …」；现在构造
    发生在探针内部、错误照常经 `classify_connection_error` 的兜底分支，文案变成
    「连接失败: 连接失败: 创建客户端失败: …」（多一层前缀）。该兜底分支对
    `TianyanError::Custom` **本来就有**这个双前缀（迁移前的上游错误文案即如此），且此路径
    已被 handler 的 `http(s)://` 前缀校验挡在前面——属实现细节泄漏，不做「顺手修」以保持
    逐字一致（修它会同时改变上游错误文案的用户可见形状）。

- **`infer_mime_type` 随**消费者**留 core**：它原在 `provider/vision.rs`，但**唯一调用点**
  是 `knowledge/image/analyzer.rs`（知识导入的图片分析），provider 侧从不调用。它跟着
  vision 实现搬进插件就会让内核（knowledge）反向 `use tianyan_provider_llm::…`（D2-1），
  因此移到 `knowledge/image/analyzer.rs`（私有函数，函数体与分支顺序逐字未改）。

- **依赖面证据**：`core` 删除 `async-openai`（`cargo tree -p tianyan-core --edges normal`
  无该项），它只进 `provider-client`/`provider-llm`/`provider-embed`；
  `cargo tree -p tianyan-provider-llm --edges normal --depth 1` 与 `-p tianyan-provider-embed`
  均**无 `tianyan-server`**（两条都是 core / 基底 crate / 通用库）。

- **装配：`AssemblyServices` 加第九、第十个插槽**
  `ProviderLlmFactory` / `ProviderEmbedFactory`（`Arc<dyn Fn(&ModelsConfig) -> Result<…>>`，
  **同步**：客户端构造只建 HTTP 客户端、不发请求），能力集合类型
  `ProviderLlmServices` / `ProviderEmbedServices` **定义在 core**（`model/assembly.rs`）并由
  server re-export。`app` / `tauri` / `server/tests/common/factory.rs::test_assembly()`
  三处各注入一行 `Arc::new(|c| tianyan_provider_llm::services(c))`（同形同源）。
  `initialize_vfs_for_app` 的 `create_model_services` 也改走装配插槽（VFS 只需要嵌入）。

- **★ 插件示例的连带修改（如实记一条）**：`plugins/storage-vector/examples/*`（4 个一次性
  运维脚本）原先 `use tianyan::model::ModelServices` —— 容器搬走后改调两个插件的
  `services()`（`[dev-dependencies]` 加 `tianyan-provider-{llm,embed}`，与
  「storage-vector → storage-sqlite」同类的**只服务 examples 的 dev 边**，生产依赖图不受
  影响）。这同时让两个工厂有了 examples 这个第三类真实消费者。

- **逐行比对证据**（`git diff --no-index`，HEAD 原文件 vs 插件文件）：

  | 文件 | 改动行 | 归类 |
  |---|---|---|
  | `provider-client/src/client.rs` ← `provider/client.rs` | 约 40 增 / 12 删 | ① `crate::` → `tianyan::`（11 处）；② 12 个字段 + 2 个方法 `pub(crate)` → `pub` 并补文档（`missing_docs`）；③ 无逻辑改动 |
  | `provider-client/src/failure.rs` ← `chat.rs` 的两段 | 0 / 0（整块搬移） | `RetryableFailure` 与 `classify_http_status` 逐字搬走，仅 `pub(super)` → `pub` |
  | `provider-llm/src/chat.rs` ← `provider/chat.rs` | 约 30 增 / 30 删 | ① `crate::` → `tianyan::`（18 处）；② `super::client::AsyncOpenAIClient` → `crate::client::ChatClient`；③ 两段语义分类块移出（-26 行）→ 1 行 import；④ `impl ChatService for AsyncOpenAIClient` → `for ChatClient`、`impl AsyncOpenAIClient` → `impl ChatClient`、`AsyncOpenAIClient::finish_reason_str` 保留（内层关联函数）；**请求构造 / SSE 解析 / 思考参数 / reasoning 回传 / 缓存提取一行未动** |
  | `provider-llm/src/vision.rs` | 约 25 增 / 40 删 | 同上 + `infer_mime_type`（15 行）移出到 core（唯一消费者是 knowledge） |
  | `provider-llm/src/discovery.rs` | 4 / 4 | 纯路径前缀 + newtype |
  | `provider-embed/src/embedding.rs` | 约 45 增 / 20 删 | ① 路径前缀；② newtype `EmbeddingClient` + `Deref`（+14 行）；③ 测试构造改 `EmbeddingClient::new`，一处穿透 `client.0.client.embeddings()`（测的正是「上游库类型化路径必然失败」） |
  | `provider-* /logging.rs` ← `provider/middleware.rs` | 拆 + 各约 10 增 / 5 删 | 按能力拆两个文件；用量上报契约提到 core（`model::usage`），其余逐字 |

  **判定提示词 / `apply_thinking_params` 的三档预算（1024/4096/16384）/ SSE 容错口径
  （usage-only 尾块、cost 块、非标准 `finish_reason` 上抛）/ 双预算超时文案（「流式读取
  空闲超时」vs「首 token 超时」）/ `x-opencode-session` 注入与无值兜底 / UA 全局默认 /
  嵌入请求体与 usage 归一（`PromptAndTotal` / `TotalOnly`）/ 查询缓存容量与键 / 日志文案
  逐字未动。**

- **测试守恒**：HEAD `tianyan-core --lib` **1166** = 现在 core **1108**（−58：`chat` 35 /
  `client` 9 / `embedding` 6 / `middleware` 4 / `vision` 4，逐条同名同断言随迁）+
  `provider-client` **9** + `provider-embed` **13** + `provider-llm` **44**
  （58 迁移 + **8 新增**装配门禁：client 0 / embed 3（模型名 + 可注入用量槽位 + 多模态回落）/
  llm 5（能力集合形状 + 无 provider 降级 + 禁用 provider 不进路由池 + 探针构造期失败 +
  方言穿透），server 侧 +3（provider 插槽默认降级 / 注入即用且可重复 / 探针契约带临时凭据）
  + 集成测试 +1（`/config/test-connection` 走契约且参数校验先于请求））。
  算式：**1166 − 58 = 1108**；**1108 + 66 = 1174 = 1166 + 8（新增）**。**零放弃、零降级。**

- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**（`AppState::new` 的 9 参数加了显式 `allow` + 注释）；`tianyan-core --lib`
  **1108 passed**；`tianyan-provider-client` 9 / `tianyan-provider-embed` 13 /
  `tianyan-provider-llm` 44 全绿（含 `chat.rs` 的流式双预算与方言基线、动态会话头、
  reasoning 回传、`middleware` 的查询缓存/用量上报）；`tianyan-server --lib`
  **197 passed / 1 ignored**、`--tests` 6 / 14 / 1 / 2 全绿；`tianyan-app` 7 passed；
  `cargo catalog-check` OK；`cargo tree` 三个 provider crate 均无 `tianyan-server`。

- **遗留**：
  1. **`AssemblyServices` 已 10 个插槽**（本期 +2）。与期 3-T3 遗留 3 是同一件事：期 4
     清单收敛时应按插件分组（每个插件一份能力接口），而不是继续平铺；`AppState::new` 的
     5 个 `Option<Factory>` 参数同理；
  2. **`config/model.rs` 的方言数据（950 行）仍在 core**，它是 `TianyanConfig` 的字段形状；
     期 3-T4「config 二分」把它连同 loader（753 行）一起外移后，改一个方言细节才真正只重编
     插件（本序只做到「改方言的**消费逻辑**不重编内核」）；
  3. **`storage-vector` 的 4 个 examples 又添两条 dev 边**（→ provider-llm / provider-embed），
     与序 3/4 的遗留同型：干净解法是独立的 `tools-ops` bin 包；
  4. `provider-client` 的字段必须 `pub`（跨 crate 访问），公开面比「只有类型」略大；
     若期 4 决定把 provider 三件套并成一个 crate，这些 `pub` 应回退成 `pub(crate)`。

**实施记录（期 3-T4：service-config，2026-10-21）**：配置的**加载 / 持久化 / 迁移 /
校验 / 向导**整段迁入 `service/config`（crate `tianyan-service-config`）——
`core/src/config/migration.rs`（319 生产行）、`validation.rs`（115）、`wizard.rs`（49）
三文件**整体删除**，`config/mod.rs` 的加载/持久化段一并搬走（`load` / `load_from_file` /
`find_config_file` / `config_exists` / `default_config_path` / `generate_default_toml` /
`save_to_file`（段保全）/ `validate`（汇总编排）/ `merge_unknown_keys` / `expand_user_dir`）。

- **★ 本批唯一的设计点：`get_config()` 槽位化**。迁移前 `config::get_config()` 内部
  `load()`（读文件 + 迁移 + 校验）并用 `OnceLock::get_or_init` 保证「首次访问时加载一次」。
  加载器搬走后内核不能 `use tianyan_service_config::…`（D2-1 反向依赖），于是切成
  **槽位 + 注入**：

  | 侧 | 职责 |
  |---|---|
  | 内核 `core/src/config/mod.rs` | `get_config()` 纯读槽位（未注入回退 `TianyanConfig::default()`，保持「无配置文件也能跑」）、`set_config(cfg) -> Result<(), ConfigSlotOccupied>`（只在未设置时生效）、`try_get_config()`、`set_config_error()` / `last_config_error()`（诊断槽位） |
  | 服务层 `service/config` | `load` / `load_from_file` / `find_config_file` / `config_exists` / `default_config_path` / `generate_default_toml` / `save_to_file` / `validate` / `migration` / `validation` / `wizard`，以及**装配期单点** `load_and_install()` |
  | 装配层 `app` / `tauri` | 启动时一行 `load_and_install()`（**先注入再 `init_logging`**——日志级别来自 `[logging]`） |

  两个细节值得留档：① **回退值用第二个 `OnceLock`**（`FALLBACK_CONFIG`）而不是
  `get_or_init` 写回主槽位——否则「先有人读了配置、装配层再注入」会静默失败，是只在
  装配顺序出错时才暴露的陷阱；② `set_config` 的失败类型是零大小的 `ConfigSlotOccupied`
  而不是交还整份配置（`Result<(), TianyanConfig>` 会触发 `clippy::result_large_err`：
  这份配置 1.2KB，而调用方注入失败时用的本来就是自己手里那份）。

  `load_and_install()` 同时保留**两条**失败输出，都是迁移前就有的：`eprintln!`（`app` /
  `tauri` 的兜底：配置破损时 `[logging]` 读不出来、日志订阅者尚未装配，`tracing` 事件
  会被丢弃）+ `tracing::error!` + 诊断槽位（旧 `get_config()` 首次失败路径，含路径上下文）。

- **★ 热更新路径的核对（第二份真相，既有问题，本批照原样保留）**：`server` 的
  `PUT /api/v1/config` 走 `ConfigService::persist_and_reload`：`validate` →
  `save_to_file` → `AppState::update_config`（`Arc<RwLock<TianyanConfig>>`）→ 热重载 Agent。
  它**不**改写内核槽位。核对结论：**迁移前就不是同一份**——`get_config()` 在 server/app/tauri
  里根本没有调用点（全仓唯一生产调用是 `core::init()`，而 `init()` 今天也无人调用），
  `AppState::config` 才是运行期权威。本批不改变这一行为，只在 crate 文档与本节里记明：
  **槽位 = 启动快照（供 `init()` 读日志配置），`AppState::config` = 运行期权威**；
  期 4 引入真正的配置源 seam 时才应合并。`OnceLock` 本不可改写，所以「把新配置写回槽位」
  在槽位方案下**做不到**——若将来需要，得把 `get_config()` 的返回类型从 `&'static` 改掉。

- **为什么结构留 core（不是遗漏）**：`TianyanConfig` 与全部子 Config 是内核与插件的
  **共同构造参数**（`TaskServices.config` / `ContextServices.config` /
  `PluginServices.web_config` / `TaskScheduler.config`），搬走等于让所有插件反向依赖
  服务层；`presets.rs`（`PROVIDER_PRESETS` / `find_preset`）是配置**数据表**，server 的
  配置 API 要展示预设（`resolve_all_preset_hits`），搬进插件会让 server 反向依赖插件；
  各子配置的 `validate`（`AgentConfig::validate` 等）是字段自身的约束，随结构走。

- **逐行比对证据**（`git diff --no-index`，HEAD 原文件 vs 新文件；`-w` 忽略缩进）：

  | 文件 | 差异（增/删） | 归类 |
  |---|---|---|
  | `migration.rs` | 16 / 10 | ① `use crate::{common,config}` → `use tianyan::…` + `use crate::loader::{…}`（4 行）；② `TianyanConfig::find_config_file()` / `load_from_file` / `save_to_file` → 自由函数（4 处调用点，逐字保留参数与错误映射）；③ 模块文档 +7 行（迁移动机 + 「逐字未改」清单）；④ 一处文档断行重排（1 行） |
  | `validation.rs` | 8 / 3 | ① `use crate::…` → `use tianyan::…`（2 处，含测试里 1 处）；② 模块文档 +5 行。**函数体与全部错误文案 0 改动** |
  | `wizard.rs` | 4 / 0 | 仅模块文档 +4 行。类型、字段、`serde` 属性、`success`/`error` 构造**逐字未改**（前端契约不变） |
  | `config/mod.rs` → `loader.rs` | 102 / 90（`-w`） | ① `impl TianyanConfig { … }` 的关联函数降为自由函数（`Self::x()` → `x()`、`self` → `config` 参数、`std::path::` 前缀改 `use` 导入）；② 新增 `load_and_install`（替换旧 `load_config_or_default` + `get_config` 的首次加载闭包）；③ 模块文档 +48 行（三类职责 + 逐字保留清单）；④ `std::path::Path` → `Path` 之类的导入改写 |

  **判定/校验/迁移的全部文案与规则逐字未动**：`配置加载失败：未找到配置文件 ~/.tianyan/tianyan.toml（可用环境变量 TIANYAN_CONFIG 指定其他路径）` /
  `配置加载失败：构建配置失败：…` / `配置加载失败：反序列化配置失败：…` /
  `配置加载失败：{e}` / `配置保存失败：创建目录失败：…`（序列化 / 写入同前缀）/
  `config 加载失败（路径：{location}）：{detail}` / 全部迁移文案（`新数据目录不能为空` …
  `migration: 校验失败：源与目标的文件清单或大小不一致`）/ 段保全合并规则（未建模键保留、
  建模键以内存值为准）/ `TIANYAN_CONFIG` 覆盖与空串回落 / `~` 展开规则。

- **门禁**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets`
  **零 warning**；`tianyan-core --lib` **1080 passed**（= 基线 1108 − 28 条随迁）；
  `tianyan-service-config` **29 passed + 3 集成**（12 loader / 11 migration / 5 validation /
  1 wizard，另 3 条为随迁的 `tests/config_integration.rs`）；`tianyan-server --lib`
  **197 passed / 1 ignored**、`--tests` 6 / 14 / 1 / 2 全绿；`tianyan-app` 7 passed；
  `cargo catalog-check` OK；`cargo tree -p tianyan-service-config --edges normal --depth 1`
  = core / chrono / config / dirs / serde / serde_json / toml / tracing，**无 `tianyan-server`**。

- **测试守恒**：HEAD core `--lib` **1108** = 现在 core **1080** + 本 crate **29**（28 条为
  逐条同名同断言随迁：loader 11（`~` 展开 / 段保全 ×2 / 默认 TOML / 缺文件 / 错误文案 /
  非法 TOML / `TIANYAN_CONFIG` ×3）、migration 11（Windows 上编译的那一组）、validation 5、
  wizard 1；**+1 新增** `test_load_and_install_falls_back_to_default_on_broken_config`——
  此前「加载失败 → 回退默认 + 记诊断」只有一条**条件式**覆盖（`if let Some(err)`），现在
  直接断言）。算式：**1108 − 28 = 1080**；**1080 + 29 = 1109 = 1108 + 1（新增）**。
  另有 `core/tests/config_integration.rs`（3 条集成测试）**整文件随迁** →
  `service/config/tests/config_integration.rs`，core 侧删除（`--lib` 计数不含它）。
  **零放弃、零降级。**

- **★ 与任务书估算的偏差（必须留档）**：普查按「mod.rs 的加载段 270 行全部外移」估
  753 行 → impl 15,314。实测 **core 生产行净减 644**（migration 319 + validation 115 +
  wizard 49 + mod.rs **161**），差额 **109** 全部来自**留在内核的槽位管道**：新槽位段
  97 行（3 个 `OnceLock` + `get_config` / `set_config` / `try_get_config` /
  `set_config_error` / `last_config_error` + `ConfigSlotOccupied` + 文档）替换了旧的
  66 行加载式槽位（`CONFIG` / `CONFIG_LOAD_ERROR` / `load_config_or_default` /
  `config_load_error` / `get_config` / `last_config_error`）。
  → **闸门 B 的 impl 现在是 16,067 − 644 = 15,423（仍超 15K 计 423 行，不是 314）**。
  这一条不是失手，是「槽位 + 注入」这一分层口径的必然代价：内核必须自己保留
  `&'static` 的读取面与注入 API。下一批要补的是 423 行，候选见「待决问题」与期 3 普查的
  `agent/background.rs`（1,300 行，已裁决算 K1，不可外移）之外的余量。

- **遗留**：
  1. **槽位与 `AppState::config` 是两份真相**（见上）：期 4 引入配置源 seam 时合并；
     在那之前，任何「读全局配置」的新代码都要想清楚它要的是启动快照还是运行期权威。
  2. **`storage-vector` 的 4 个 examples 再添一条 dev 边**（→ `tianyan-service-config`），
     与序 3/4/6 的遗留同型：干净解法仍是独立的 `tools-ops` bin 包。
  3. 本 crate 公开符号 30 个（loader 9 / migration 12 / validation 6 / wizard 2 +
     `ConfigSlotOccupied` 不在本 crate）。它不是能力插件（无插槽），消费者有三个
     （`app` / `tauri` / `server`），因此「每插件 crate 公开符号 ≤ 15」这条反指标不适用；
     但若期 4 收紧口径，可把 `perform_migration` / `delete_source_entries` /
     `update_config_data_dir*` / `read_migration_request` 降为 `pub(crate)`（它们只有
     crate 内测试与内部调用）。
  4. `service/config` 的目录形态（`service/` 下按域分 crate）是**新约定**：期 4 若把
     `service-snapshot`（ADR-006 的独立存储语义）也切成 crate，应沿用同一位置规则。

**实施记录（期 3-T5：tool-pipeline，2026-10-21）**：**工具管线的出口侧**整段迁入
`plugins/tool-pipeline`（crate `tianyan-tool-pipeline`）——`agent/tool_registry/observability.rs`
（213 生产行：统计 / Trace / GEPA 执行记录 / 失败规则学习）、`agent/tool_registry/spill.rs`
（181 生产行：ADR-047 超预算输出落盘）+ 其测试文件 `spill_tests.rs`（265 行）三个文件
**整体删除**；主搬 **394 生产行**。**本批还把 `agent/tool_catalog.rs`（96 生产行）一并搬入**
——它单独不成 crate，而 394 达不到闸门 B 需要的 423（见下方规模账），且它是**工具集的对外
呈现**（表头 / 排序 / `|` 转义 / 平台提示词归一化），变化原因与两个监听器同族：
改一句表头不该重编内核。

- **★ 本批唯一的设计点：注册表不再自带任何 post-execute 监听器**。迁移前
  `ToolRegistry::new()` 直接 `push` 两个具体类型的监听器（观测 + 落盘），并因此把
  `ToolObservabilityListener` / `ToolOutputSpillListener` 作为**字段**持在注册表上
  （`with_usage_stats` / `with_trace_collector` / `with_rule_recorder` /
  `with_execution_log` / `with_tool_output` 五个 `with_*` 都是往这两个字段上写状态）。
  实现搬出 core 后这套形态不成立，改为 **接线形状 + 工厂**（契约住 core，实现住插件）：

  | 侧 | 职责 |
  |---|---|
  | core `agent/tool_registry/pipeline.rs` | `ToolPipelineWiring`（5 个 `Option`：`usage_stats` / `trace_collector` / `rule_recorder` / `execution_log` / `tool_output`）+ `ToolPipelineFactory = Arc<dyn Fn(&ToolPipelineWiring) -> Vec<Arc<dyn ToolPostExecuteListener>> + Send + Sync>` |
  | core `agent/builder.rs` | `AgentBuilder::with_tool_pipeline_factory`；`build()` 组装 wiring（值全部来自 builder **自己的**同名字段——同一份 `Arc`，不二次实例化）→ 按工厂返回顺序逐条 `register_post_execute_listener` |
  | 插件 `plugins/tool-pipeline` | `pipeline(&wiring) -> ToolPipeline`（两个**具体类型**：装配期/测试要能看接线结果）+ `ToolPipeline::into_listeners()`（**唯一顺序落点**）+ `pipeline_factory()`（类型擦除入口） |
  | 装配层 `app` / `tauri` / `tests/common/factory.rs` | 各一行 `with_tool_pipeline_factory(tianyan_tool_pipeline::pipeline_factory())` |

  注册表侧**删掉**：两个字段、两个类型的 `mod` 声明、`with_tool_output` / `with_rule_recorder`
  （唯一的调用者就是 wiring），`with_usage_stats` / `with_execution_log` /
  `with_trace_collector` 退化为「保留 `Arc` 供自身查询 / 后台任务 span」——`self_check`
  读技能统计改读 `self.usage_stats`（同一份实例，行为不变）。

- **★ 顺序契约（ADR-047 原文，一字未改）**：ADR-047 明写「落盘为 post-execute 统一出口
  （单点），**注册在观测监听器之后——数据层须见原始结果**」。现在这条契约有**两个**落点，
  都各占一行：
  1. **实现点**（插件 `ToolPipeline::into_listeners`）：
     `vec![Arc::new(self.observability), Arc::new(self.spill)]`——注释写明「观测在前
     （数据层须见原始结果），落盘在后（它是改写者）」，这一行就是契约；
  2. **装配点**（`AgentBuilder::build`）：`for listener in factory(&wiring) { … }` 上方
     ★ 注释写明「工厂返回的顺序即注册顺序」，并指回实现点与锁测试。

  **顺序锁**（行为式，判别力最强）：`spill_tests::test_pipeline_order_observability_sees_original_result`
  ——接上 `ExecutionLog`（真实内存 SQLite）+ 小阈值落盘，跑完整条管线后断言
  **GEPA 数据层里记的是原文**（`result` 含尾部行 `line-49-`、不含 `"spilled"`），而会话侧
  结果**已被改写**（`spilled: true`）。若把 spill 排到观测之前，数据层只会记到「预览对象」，
  本测试立刻变红。它替换了迁移前的 `test_registry_registers_spill_listener_second`
  （后者断言注册表内部字段，注册表不再持有监听器后无处落脚）。另新增
  `test_pipeline_wires_tool_output_budget`：`wiring.tool_output` 真的落到 spill 监听器
  （阈值 / 目录 / 开关），迁移前这条断言寄生在注册表字段上。

- **降级语义（未装配插槽时）**：**一个 post-execute 监听器都不注册**——超预算工具输出
  不再落盘（只受既有 `executor::truncate` 截断限制）、使用统计 / Trace 工具 span /
  GEPA 执行记录 / 失败规则学习全停；`AgentBuilder::build` 打一条 `warn!` 指路
  （ADR-038 可见失败：静默降级的唯一症状是「统计面板再也不涨 + 大输出重新撑爆上下文」）。
  **不 panic、不返回 `Err`**：`create_app`（构造对象图，47 处 lib 单测 + 嵌入式）与集成测试
  靠「不要求装配也能起实例」才能跑，`Err` 等于删入口。与 ADR-047「落盘失败保持原结果
  （best effort）」是同一档语义——缺实现时少一层优化，不是工具调用失败。
  **行为差异清单（仅未装配路径）**：`create_app` 起的实例不再产生 usage stats / 工具
  Trace span / GEPA 执行记录 / 规则条目，也不落盘——`start_server*` 与集成测试的
  `test_assembly()` 都注入真工厂，测的仍是生产形态。

- **逐行比对证据**（`git diff --no-index`，HEAD 原文件 vs 新文件；`-w` 与普通口径结果相同，
  即无「仅缩进」的差异）：增删行数 = 新增/删除。

  | 文件 | 增/删 | 归类 |
  |---|---|---|
  | `observability.rs`（→ 插件） | 23 / 20 | ① `use crate::…` → `use tianyan::…`（7 行，其中 `CallSkillParams` 走 `agent::` 重导出、`TRACE_PARAMS_MAX_BYTES` 走新提公的 `agent::` 公共常量）；② `pub(crate) struct` → `pub struct`（跨 crate 必要）；③ 模块文档 +6 行（迁移动机 + 「与 `ToolRegistry::new` 解耦」的更正）；④ 5 处「由 ToolRegistry::with_* 委托」注释改成「装配期由 `ToolPipelineFactory` 接线」。**四个 `set_*` / `on_post_execute` 的函数体、统计键规则（`skill:` 前缀）、Trace 截断上限、GEPA 记录构造、`FailureKind::Logic`、全部 debug 文案 0 改动** |
  | `spill.rs`（→ 插件） | 12 / 7 | ① 6 行 `use crate::…` → `use tianyan::…`；② `pub(crate) struct` → `pub struct`；③ 模块文档 +5 行；④ `configure` 的注释改调用方。**`SKIP_TOOLS` 白名单、预算判定（`lines <= max_lines && bytes <= max_bytes`）、`truncate_spill_with` 调用参数顺序、spilled 对象 5 个字段、`note` 文案、两条 tracing 文案（info/warn）逐字未改**；阈值下限钳制（`max(1)`）保留在 `configure` 里 |
  | `spill_tests.rs`（→ 插件） | 61 / 27 | ① 导入改写（`crate::` → `tianyan::`，新增 `Database` / `ExecutionLog` / `ToolPipelineWiring`）；② 8 条测试**逐字未动**；③ `test_registry_registers_spill_listener_second` → 行为式顺序锁（见上）；④ 新增 1 条接线测试 |
  | `tool_catalog.rs`（→ 插件） | 21 / 14 | ① `crate::` → `tianyan::`（含测试）；② `[ToolRegistry]` 文档链接改成代码体（不同 crate 无法链接）；③ 模块文档 +6 行 + 更正「权威生成器在 `app/examples/`」（原文写的 `server/examples/` 是 0.5.8 的旧位置）。**渲染表头字符串、排序、`\|` 转义、`normalize_platform_hints` 的两种形态替换、`check` 的 CRLF 归一化逐字未改**（`cargo catalog-check` 仍绿即证明：产物与仓库文件逐字一致） |

- **★ 闸门 B 最终实测（期 3 收口数）**：口径 = **四件套**（`core/src` 生产行按文件归属
  二分：api = trait + 纯类型/契约文件，impl = 其余），生产行 = 到 `#[cfg(test)]` 之前
  （与 `scripts/measure-rebuild.ps1::Get-SourceLines` 同源）。本批 core 侧逐文件增量：

  | core 文件 | api/impl 桶 | 生产行增量 |
  |---|---|---|
  | `agent/tool_registry/observability.rs` | impl | **−213**（整文件迁出） |
  | `agent/tool_registry/spill.rs` | impl | **−181**（整文件迁出） |
  | `agent/tool_catalog.rs` | impl | **−96**（整文件迁出） |
  | `agent/tool_registry/mod.rs` | impl | **−15**（删 2 字段 / 2 `mod` / 2 个 `with_*` / 注册块） |
  | `agent/tool_registry/agent_ops.rs` | impl | 0（`self.observability.usage_stats_ref()` → `self.usage_stats`） |
  | `agent/builder.rs` | impl | **+40**（wiring 组装 + 工厂注入 + 告警） |
  | `agent/tool_registry/pipeline.rs` | **api** | **+36**（`ToolPipelineWiring` + `ToolPipelineFactory`） |
  | `agent/mod.rs` | **api** | **−1**（`tool_catalog` 导出删除，改导出管线契约 5 个符号） |
  | **合计** | | **净 −430**（impl −466 / api +35） |

  → **impl = 15,423 − 466 = 14,957（≤ 15,000 ✅，余 43 行）**；
  **api = 4,552 + 35 = 4,587（≤ 6,000 ✅，余 1,413 行）**。
  交叉校验（用 T4 当时的粗算式 `impl_old − 生产行净减`）：15,423 − 430 = **14,993**，
  同样 ≤ 15K（两者差 36 行，正是**接线契约落在 api 桶**的部分——它本就属于契约预算，
  api 桶余量 1.4K 不缺这 36 行）。
  **闸门 B 至此收口：`api 4,587 ≤ 6K && impl 14,957 ≤ 15K`。**

- **测试守恒**：HEAD core `--lib` **1080** = 现在 core **1066** + 本 crate **14**（观测 2：
  T0-9 内存不缓冲 / GEPA 落库；落盘 10：超限改写 / 预算内零成本 / 白名单 /
  `execute_command` 免落盘 / 失败不改写 / 关闭旁路 / 未配置旁路 / 落盘失败保原结果 /
  顺序锁（重写）/ 阈值钳制；目录 2：平台归一化 / 渲染排序）。
  **+2 新增**（`test_pipeline_factory_is_repeatable_and_ordered`、
  `test_pipeline_wires_tool_output_budget`——注册表不再持有监听器后，这两条只能落在
  管线入口）。算式：**1080 − 14 = 1066**；**1066 + 16 = 1082 = 1080 + 2（新增）**。
  **零放弃、零降级。**（`spill_tests.rs` 整文件随迁，core 侧删除。）

- **任务书的后备候选为何不搬**（实测证据）：`file_ops.rs`（174 生产行）、
  `knowledge_ops.rs`（165）、`evolution_ops.rs`（177）三个文件全是 **`impl ToolContext { … }`**
  （`file_ops.rs:20 impl ToolContext`、`knowledge_ops.rs:13 impl ToolContext`、
  `evolution_ops.rs` 同形）——`ToolContext` 是 K4 装配原语（ADR-049 期 0.5.3），搬它们
  等于把 `ToolContext` 一起搬走，破 D2-1/D2-3。`agent_ops.rs`（1,198）如任务书所述直接依赖
  `Agent` / `AgentLoop` 具体类型，不搬。因此本批选择搬 `tool_catalog.rs`（无 `ToolContext`
  依赖，只读 `ToolRegistry::definitions()` / `presentation()` 两个公开方法）。

- **★ 插槽平铺表的治理建议（留给期 4，不在本批实施）**：`AssemblyServices` 到本批已是
  **11 个平铺字段 + 11 个 `with_*` + 11 个 `pub(crate)` 访问器**，`AppState::new` 的
  `Option<Factory>` 形参也到了 6 个（另有 `build_agent` / `build_agent_or_wizard` 的
  24 个形参）。三条候选路线，按推荐顺序：

  1. **按插件分组**（推荐）：一个插件 crate 自己给出**一份能力结构**，装配层只持有
     `struct Assemblies { tool: ToolAssemblies, storage: StorageAssemblies, policy: PolicyAssemblies, provider: ProviderAssemblies }`。分组维度的判据是**变化原因**（D7 同一把尺子）：
     工具 / 存储 / 策略 / provider 四族各自的字段经常一起变（如 T2 的 `ContextFactory`
     与评审两插槽同族），分成 4 组后 `AppState::new` 的形参从 6 降到 2–3，
     组合根的「用了哪些能力」仍然显式可见。
  2. **清单结构**：`Vec<AssemblyEntry>`，每个插件 crate 提供 `entry()` 返回「名字 + 装配闭包」，
     装配层只拼接（与 D6「清单是有序数据」同构）。优点是可一页查询 + 天然支持
     desktop/headless/minimal 三份清单；缺点是类型擦除再降一层（每个 entry 的输入形状不同，
     需要 `Any` 或统一成 `&AssemblyContext`），`unsafe_code = deny` 下只能走 `Any` 下行转换，
     会引入运行期失败点——**与「缺件在启动期响亮失败」的初衷相抵**，故排在 1 之后。
  3. **合并同类**：把 `TaskReviewerFactory` 与 `SkillReviewFactory`（同属 `policy-review`）、
     `ProviderLlmFactory` 与 `ProviderEmbedFactory`（同属 provider 域）合并成各一个「域工厂」，
     11 个字段降到 8 个。代价是两个消费者（自审 / 技能复审、聊天 / 嵌入）的重建时机不同
     （前者随配置、后者随模型），合并后要么多传参、要么拆返回值——收益最小，仅作兜底。

  三者都不该在期 3 做：本批每加一个插槽都已经付出「三处装配各写一行 + 一处组合根透传」的
  成本，中途改形状会把 T1–T5 的接线全部重写一遍。**期 4 清单收敛时按 1 起步**，
  并把「插槽数量 / `AppState::new` 形参数」记成一条反指标（建议 ≤ 4 / ≤ 3）。

- **遗留**：
  1. **插槽平铺表治理**（见上，期 4 第一件事）；
  2. **`RuleRecorder` 仍在 core**：它是「失败规则学习」的**写侧数据层**（写 VFS
     `agent/_rules/…`），今天的唯一生产调用者是观测监听器（构造点在 `AgentBuilder`），
     读侧是 `policy-context`（检索）与 `task-builtin`（GC 扫描）。搬它可以再省约 330 生产行，
     但这属于「数据层是否随观测一起外移」的边界裁决（与 `ExecutionLog` / `TraceCollector`
     同族），不宜在收口批次顺手做；若期 4 决定拆 `kernel-impl`，应与这三个数据层一起判；
  3. **`tool-pipeline` 与 `tool_catalog` 同 crate 是权宜**：目录渲染与执行出口没有逻辑耦合，
     只是 96 行单独不成 crate（反指标「每 crate ≥ 300 行」）。期 4 若有更强的归属
     （例如与 `builtin_tools.rs` 的元数据表同域），应把它挪走；
  4. **`TRACE_PARAMS_MAX_BYTES` 已提为 `pub`**（`agent::TRACE_PARAMS_MAX_BYTES`）：
     它是内核与插件共用的观测体积预算，若将来 Trace 契约演化出「每 span 预算」的配置项，
     这个常量应随之进配置并在 core 侧单点解析。

---

### 期 4 实施记录（清单收敛，2026-10-23）

**本批目标**：把「装配清单」本身收敛为单一事实源，并把跨插件顺序从注释变成断言。

#### ① 普查推翻了任务书的「三份清单」：实测是 **5 处副本**，但只该有 **2 个函数**

分期表原写 `desktop()` / `headless()` / `minimal()` 三份清单。实测副本是 5 处：

| 位置 | 工具清单 | 9 个策略工厂 | vector | storage |
|---|---|---|---|---|
| `app/src/main.rs` | ✅ | ✅ | ✅ | ✅ |
| `tauri/src/server.rs` | ✅（内联闭包） | ✅ | ✅ | ✅ |
| `server/tests/common/factory.rs::test_assembly()` | — | ✅ | — | ✅ |
| `app/tests/assembly.rs` | ✅ ×2（`plugin_tools()` + 内联工厂） | — | ✅ | ✅ |
| `app/examples/tool_catalog.rs` | ✅ | — | — | — |

**desktop 与 headless 的装配内容完全相同**——差异只在启动入口
（`start_server_with_shutdown_signal_and_assembly` vs `start_server_with_assembly`）、
桌面端的端口探测与窗口生命周期、以及 bin 侧的「搬迁后重启」循环。为「同一份内容」
造两个名字会让人以为它们真的不同，而「改一处必须改两处」正是要消灭的形态。

因此本批落地的是 **2 个函数，且后者是前者的前缀**：

```text
full()      = policies() + plugin_factory() + vector_factory()      ← app bin / tauri
policies()  = 9 个策略 / 基础设施插槽                                ← server 集成测试
              storage / task / approval / context / task_reviewer /
              skill_review / provider_llm / provider_embed / tool_pipeline
```

- **`policies()` 为什么值得一个名字**：`minimal` 的真实需求（集成测试剖面）**确实**
  与生产不同——少两个「环境相关」插槽（动态工具清单在 `create_app` 路径不注册；
  真实 LanceDB 会为每个测试建目录）。但它不该叫 `minimal`：那会诱导读者以为
  「测试少装了几块策略」，而实际上这 9 个策略插槽与生产**完全同形**。
- **为什么不用 `without_*` 从 `full()` 里拿掉两块**：`AssemblyServices` 只有
  `with_*`、没有 `without_*`（「装配了又拿掉」与「本来就没装」在运行期不可区分，
  那是降级路径的语义污染）。前缀关系 `full() = policies() + 2` 是更强的保证：
  新增一个策略插槽只改 `policies()`，两个消费者同时得到。
- **`plugin_factory()` 为什么是 `pub`**：除 `full()` 外还有两个**真实**消费者要用同一份
  清单——`app/examples/tool_catalog.rs`（工具目录的**权威生成器**：目录必须与运行时
  工具集逐字一致，否则插件工具既无文档覆盖、也不受 freshness 门禁保护）与
  `app/tests/assembly.rs`（插件工具全部注册 / 展示意图 / 不撞内置 / 工厂可重复）。
  改造前这两处各自重写了一遍清单。

#### ② 新增的边界认知：**dev-dependency 环是允许的**（与 `#[path]` 陷阱不是一回事）

`server/tests` 要用 `tianyan_assembly::policies()`，而 assembly 依赖 server
（取 `AssemblyServices` 与 11 个插槽类型别名）→ `server --dev--> assembly --> server`。

**Cargo 允许**：dev-dep 不参与**构建**图的环检测，测试目标的依赖图里 server lib
只编一份（与普通 dev-dep 无区别）。这与 core 的「`#[path]` 同源包含 + dev-dep →
同一批源文件编进两个 crate」**完全不同**——那里 `StorageBackend` 会出现两个不兼容
实例（22 处 E0308/E0277），这里 `AssemblyServices` 始终只有 server 那一份定义。

代价（已实测并接受）：server 的测试目标现在连带编译 assembly 的全部依赖（含 LanceDB
链），首次 `cargo test -p tianyan-server` 变慢；换来「9 个工厂只有一份实现」——插槽
变化在**编译期**暴露，而不是等某个装配器运行期缺件（静默降级最贵的那种）。

#### ③ 跨插件顺序：从注释变成断言（期 3-T5 遗留 1 的兑现）

```text
① 工具注册顺序 = 提示词前缀顺序（抖动 → 前缀缓存全白）
   fs(read_file, write_file, apply_edit, apply_patch, glob, list_dir)
     → code(symbol_outline, repo_map, grep) → todo(todo) / goal(goal)
     → web(web_search, web_fetch)
② 任务清单顺序 = 扫描顺序 = 同一轮到期任务的执行顺序
   summary_generation → evolution → garbage_collection → usage_stats_flush
     → [snapshot_gc] → [reminder]
         ▲
         └── splice(1..1)：跨插件位置，只有装配层知道（插件之间零互赖，D2-2）
③ 工具管线监听器顺序（ADR-047「顺序是语义」）：观测 → 落盘
         ▲
         └── 顺序在插件内 `ToolPipeline::into_listeners` 一行可见，装配层不重排
```

- ① `test_tool_order_is_fs_code_todo_web`：**端到端跑真实插件清单**（不 mock），断言前
  11 个工具逐位为 fs(6) → code(3) → todo/goal(2)；web 两个若注册必在末尾且顺序固定。
  用「若注册」而非「必定 13 个」，是为了不让断言退化成 `WebConfig` 校验测试——
  **配置非法时降级为空清单是插件的合法行为**。
- ② `merge_task_lists` 是**唯一**的跨插件拼接点（`task_factory` 里只调它）：
  `test_merge_task_lists_puts_evolution_second` 锁位置，
  `test_merge_task_lists_is_identity_when_evolution_absent` 锁「演化未启用时内置清单
  原样不动」。用 `splice` 而非 `insert` 循环，是为了让这条纪律在一行里可见。
- ③ 顺序留在插件内（本批只做类型擦除包装）：搬进装配层会让顺序脱离被测对象。

**① 的实证价值（当场发生）**：这条断言第一次跑就抓到了**我自己的错误假设**——
`fs` 域内部顺序按源码排列「看起来」是 `glob, list_dir, read_file, write_file, …`，
实测注册顺序是 `read_file, write_file, apply_edit, apply_patch, glob, list_dir`
（与 `ToolPresentation` 表的枚举顺序一致）。这正是「顺序纪律只写在注释里」的典型
风险：**注释写错了没人发现，断言写错了当场红**。

#### ④ 数据

| 指标 | 改造前 | 改造后 |
|---|---|---|
| 清单副本 | **5 处**（见 ① 表） | **2 个函数**（`full()` / `policies()`）+ 1 个工厂（`plugin_factory()`） |
| `app/src/main.rs` | 277 行 | **101 行** |
| `tauri/src/server.rs` | 268 行 | **114 行**（含端口探测 + 3 个测试） |
| `server/tests/common/factory.rs` | 240 行 | **158 行** |
| `assembly/`（新 crate） | — | 生产 347 行 + 测试 215 行（均含文档） |
| 装配器对插件的**直接**依赖 | app 15 个 / tauri 15 个 | app **1** 个（`tool-pipeline`：工具目录渲染口径）+ assembly；tauri **0** 个 + assembly |
| 跨插件顺序断言 | 0（只有注释） | 4 条（工具顺序 ×1、任务位置 ×2、装配可重复 ×1） |

反指标复核：新 crate 347 生产行 ≥300 ✅；真实消费者 3 个（`app` / `tauri` /
`server/tests`）✅，`plugin_factory` 另有 2 个（catalog 生成器 / 装配级测试）——
没有一个是「为对称而造」的。

#### ⑤ 未做（明确遗留）

1. **插槽平铺表治理**（期 3-T5 遗留 1 曾标为「期 4 第一件事」）：本批**未做**。它要动
   `AssemblyServices`（11 字段 + 11 `with_*` + 11 访问器）、`AppState::new` 形参
   （6 个 `Option<Factory>`）、`build_agent*`（24 形参），风险与收益都和「清单收敛」
   不同量级。本批已把**漂移风险**消除（唯一实现 + 顺序断言），而平铺表的问题是
   **可读性 / 形参数**——按「改一批、提交一批」拆成独立批次（**期 4b**），判据沿用
   下方三条候选路线（按插件分组优先）与反指标（插槽数 / 形参数）。
2. **启动编排的重复**：`app/src/main.rs` 的「`load_and_install` → `init_logging` →
   `panic_hook::install`」与 `tauri/src/lib.rs` 里的一套（有各自的 `init_logging` 函数）
   仍是两份。它属**启动编排**而非装配清单（桌面端还要窗口 / 托盘 / 单实例 / 更新器），
   收敛需先把「桌面端独有」与「两端共有」切开，留待后续。
3. **`policies()` 与 `full()` 的字段级断言**：`AssemblyServices` 的访问器是
   `pub(crate)`，本 crate 无法直接断言「`policies()` 缺的正是那两个插槽」。现有断言是
   行为级（server 侧插槽测试已覆盖「未装配 → 空清单 / Noop 兜底」），本批不造第二套。

---

## 分期与闸门

| 期 | 目标 | 动作要点 | 闸门（硬数） |
|---|---|---|---|
| **0** | 地基与基线（零行为改动）✅ **已完成** | incremental 实验（**3× 加速 / 185MB 每会话**，建议开启 + GC 3 天/20GB）；`scripts/measure-rebuild.ps1`；`contracts.md`（34 trait 分级 + common 二分）；`microkernel-baseline.md` | 三场景基线齐全（**均 100% 全量重编**）；lint/unit 绿；无行为改动 ✅ |
| **0.5** | 工具契约与注册形状 + 装配器二分（**工具侧已完成**） | ✅ 元数据表承载执行入口 + 协议工具独立成表（28 分支 match → 一次查表，内核零工具名字面量）；✅ `ToolContext` 承载六个工具域（`impl ToolContext` + 注册表 `Deref`），依赖边界实测：工具域 executor 引用仅 16 处（随插件搬走）。待办：`thin-channel` + `assembly-manifest`；`AppState` 20 字段契约化 | 工具目录 freshness ✅；工具行为零变化 ✅（1431 单测全绿）；clippy 零警告 ✅ |
| **1** | 工具插件试点（**装配器二分 + tools-fs 骨架已完成**） | ✅ `tianyan-app` bin 包（**清单住 bin**）+ `ToolPluginFactory` 类型擦除 + `create_app_with_plugins` / `start_server_with_plugins*`；✅ `plugins/tools-fs` 独立 crate（`executor::fs` 实现整体迁入）。待续：**整域迁移**——`read_file` 目录模式反向依赖 `executor::fs::execute_list_dir`，故 fs 域必须 6 工具一起搬（read/write/edit/patch/glob/list_dir）才能删 core 侧 | 闸门 A **首测 512 行 ✅**（判据 ≤3.5K）；core 侧暂留（双路径过渡，重名注册自动跳过） |
| **2** | 适配器与任务插件（**期 2 全部完成（序 0–6）**） | 迁移顺序（按 自包含度×收益）：**0 清 core 死依赖** → 1 `tools-todo`+`tools-goal` → 2 `tools-web` → 3 `storage-vector` → 4 `storage-sqlite` → 5 `task-*`（2 crate）→ **6 `provider-llm`+`provider-embed`（+ 共享基底 `provider-client`，2026-10-21 补做）**。详见下方「期 2 普查结论」与「实施记录（期 2 序 6）」 | 各领域 ≤ 2K；全量 release 不劣化 >10% |
| **3** | 策略分离（最难一刀） | 抽 `kernel-impl`（loop/cancel/rollback/stream_forward/working_set）；**`context`→`policy-context`（T2 ✅ 2026-10-21，见下方实施记录；`assembler.rs` 按普查修订留 kernel-impl）**；**审批→`policy-approval`（T1 ✅ 2026-10-21）**；`role_learning`→已删除（零消费者，2026-10-21 裁决）；**`judge` + `skills/reviewer`→`policy-review`（T3 ✅ 2026-10-21，475 生产行；`TaskReviewer` / `SkillReviewPort` 契约留 core）**；**配置加载/持久化/迁移/校验/向导→`service-config`（T4 ✅ 2026-10-21，644 生产行净减；结构与预设表留 core，`get_config` 槽位化）** | **闸门 B**：api ≤ 6K && impl ≤ 15K——**✅ 期 3 末实测 `api 4,587 / impl 14,957`**（期 3-T5 收口后；口径 = 四件套文件归属。`agent/background.rs` 1,300 行经裁决算 K1，不可外移）；改策略不重编内核 ✅；S 级零变更 ✅ |
| **4** | 清单收敛与发行版（**清单收敛已完成 = 4a**；插槽治理 = 4b 未做；发行版收尾待 1.0 前） | ✅ `tianyan-assembly`：11 个插件工厂的**唯一实现** + `full()` / `policies()` 双清单（前缀关系）+ 跨插件顺序断言 4 条（详见下方「期 4 实施记录」）；⏳ 插槽平铺表治理（见 ⑤ 遗留 1）；⏳ 安装验收 / `latest.json` / 签名 | 装配清单 1 页可查 ✅（`assembly/src/lib.rs` crate 文档三张图）；headless 可构建 ✅（`app` bin 即 headless）；**原「desktop/headless/minimal 三份清单」被实证推翻**（desktop ≡ headless，minimal = `policies()`——见 期 4 记录 ①） |

**反指标**（防拆过头）：每 crate ≥ 300 行且至少 2 个真实消费者；每期净增 pub trait ≤ 3；
每插件 crate 公开符号 ≤ 15；全量构建不劣化 >10%。

---

## 后果

### 正面

- 改工具插件：64,328 → **约 3,300 行**（降 95%）；改内核机制：插件零重编（约 14,600 行，降 77%）。
- 依赖面收窄：`lancedb → datafusion` 只进 `storage-vector`，`tree-sitter ×6` 只进 `tools-code`，
  工具/策略插件的编译链完全不碰这两条最重的链。
- 装配可审计：一页清单回答"天演由什么组成"；缺件在启动期失败而非运行期。
- 演进解耦：调 prompt、加工具、换 provider 不再撼动循环。

### 负面 / 代价

- 迁移期存在双路径（旧 `pub use` 与插件路径并存），需按版本周期清理。
- 拆 crate 会把部分 `pub(crate)` 提升为 `pub`，API 面有污染风险（用"每插件公开 ≤15 符号"约束）。
- 装配器字段契约化是实打实的改动（20 字段 + 80–100 访问点），不是文档工作。
- crate 数量从 4 增至约 26，链接次数与发布管理复杂度上升（用反指标兜底）。

### 风险与止损

| 风险 | 止损 |
|---|---|
| 基线/判据错误 → "达标"但无改善 | 期 0 强制先测真基线；判据用 **64,328**（源码口径）而非 33K/98,953（文件口径） |
| incremental 磁盘膨胀复发（历史 179GB 事故） | GC 脚本 + 上限 20GB + CI 永不开启 |
| 契约冻结过早（坏设计冻死） | 期 3 结束才冻结；此前用两类真实消费者压测 |
| 注册顺序不稳定 → 前缀缓存击穿 | 清单强制有序 + 跨插件顺序断言 |
| 装配器不薄 → 收益腰斩 | D5 二分提前；闸门 A 先验结构再验数字 |
| 收益被 LTO 吞掉 | 反指标"全量不劣化 >10%"；不达标退回"只做 incremental + 期 0.5" |

---

## 替代方案与否决

| 方案 | 否决理由 |
|---|---|
| 保持单 crate，只优化 incremental / 编译参数 | 治标：编译单元仍是 56.5K，改一行工具无法降到千行级；incremental 只能减少 codegen，仍需整 crate 类型检查 |
| 运行时插件（dylib 热插拔） | Rust 无稳定 ABI + `unsafe_code = deny`；复杂度 >> 收益 |
| WASM 沙箱插件 | 当前无"第三方不可信扩展"需求；留作远期 |
| 插件市场（可下载第三方包） | 维持 REJECTED #6 否决；本 ADR 的"插件"是随发行版编译的 crate，不是用户可安装的包 |
| 按"代码类型"分层（tools/ models/ utils/） | 与"变化原因"正交：同一层里既有 S 级契约也有高频策略，边界会切错 |
| 一次性全量迁移 | 无数据闸门，风险不可控；本 ADR 每期一个边界 + 可回退 |

---

## 与既有 ADR 的关系

| ADR | 关系 |
|---|---|
| 001 VFS 双层摘要 | **不动**。插件化只动代码边界，不新建任何存储抽象 |
| 002 StructuredMessage / 027 时序链 | 进 kernel-api（K2），冻结 |
| 003 组件工具化 | K4 装配原语 = 既有 `ToolRegistry` 收拢，不新建 Manager |
| 006 快照独立存储 | `service-snapshot` 插件保留其独立存储语义 |
| 018 会话权威 SQLite | `SessionStore` 属内核机制（K3），不外移 |
| 030 统一循环框架 | K1 = 既有 `AgentLoop`；边界显式化而非重写 |
| 035 会话工作集 | 进内核（K1'），写侧收口语义不变 |
| 038 provider 方言单点 | 方言数据留在 `provider-llm` 内部，仍是单点 |
| 043 工具对连续性 | 属组装层规范化，留内核 |
| 047 工具输出 spill | 留注册表层（机制），不下放插件 |
| 049-3-T5 工具管线出口（观测监听器 + 落盘 + 目录渲染） | 本批把它从「留注册表层」细分为「**契约 + 管线阶段留 core（K1），实现外移插件**」（`ToolPostExecuteListener` / `ToolPipelineWiring` / `ToolPipelineFactory` 住 core，两个监听器住 `plugins/tool-pipeline`）；**顺序契约（观测先、落盘后）逐字保留**并双落点（实现点一行 + 装配点一行）+ 行为式测试锁 |

## 待决问题（不阻塞期 0/0.5）

1. **采样参数缺口**（2026-10-20 发现）：`temperature` / `top_p` 在 core 是 `Option<f32>` 且恒为 `None`
   → 请求不带这两个参数，走服务端默认；前端硬编码的 `temperature: 0.7` 在 server 侧被丢弃（死参数）。
   用户决定：**等微内核完成后统一处理**（届时归入 `provider-llm` 插件的模型规格配置）。
2. ~~配置加载（3.3K）是否外移为 `service-config`~~ —— **已裁决并实施**（期 3-T4，
   2026-10-21）：只外移**加载/持久化/迁移/校验/向导**（753 估算 / 644 实测净减），
   配置**结构**与**预设表**留 core（内核与插件的共同构造参数）；`get_config()` 改为
   「槽位 + 注入」，见「实施记录（期 3-T4）」。**新遗留**：槽位与 `AppState::config`
   两份真相待期 4 合并。
3. 期 3 需重新做一次行数账（`agent` 22,994 行中 10.5K 是测试，真正待判源码 12.5K）。
4. ~~闸门 B 仍差 423 行~~ —— **已收口**（期 3-T5，2026-10-21）：工具管线出口（观测监听器 213 + 输出落盘 181 + 工具目录 96 生产行）迁入 `plugins/tool-pipeline`，core 侧净减 430 生产行 → **impl 14,957 / api 4,587**，两项均过闸门。**新遗留**：① `AssemblyServices` 的 **11 个平铺插槽**需要治理（按插件分组起步，见「实施记录（期 3-T5）」末节）；② `RuleRecorder`（约 330 生产行）作为「失败规则学习的写侧数据层」仍留 core，是否随观测一起外移需与 `ExecutionLog` / `TraceCollector` 一并裁决；③ `tool_catalog` 与 `tool-pipeline` 同 crate 是权宜（96 行单独不成 crate）。

---

## 参考

- 实测数据与实施清单：`.scratch/microkernel-design/roadmap.md`
- 最终态架构图（A–G，含账本与量化）：`.scratch/microkernel-design/final-architecture.html`
- 度量脚本：`.scratch/microkernel-design/measure-rebuild.ps1`（期 0 迁入 `scripts/`）
- 原则来源：`.scratch/microkernel-design/index.html`（设计稿 v0.1）
- 评审技能：`architecture-review-html-doc`（字号 ≥12、业务流程图、SVG 可放大、渲染自检）
