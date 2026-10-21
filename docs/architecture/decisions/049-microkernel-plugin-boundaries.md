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
| 6 | `provider-llm` + `provider-embed`（3185，最难） | 前置：`ModelServices` 容器外移 assembly；连通性探测改走契约 |

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

---

## 分期与闸门

| 期 | 目标 | 动作要点 | 闸门（硬数） |
|---|---|---|---|
| **0** | 地基与基线（零行为改动）✅ **已完成** | incremental 实验（**3× 加速 / 185MB 每会话**，建议开启 + GC 3 天/20GB）；`scripts/measure-rebuild.ps1`；`contracts.md`（34 trait 分级 + common 二分）；`microkernel-baseline.md` | 三场景基线齐全（**均 100% 全量重编**）；lint/unit 绿；无行为改动 ✅ |
| **0.5** | 工具契约与注册形状 + 装配器二分（**工具侧已完成**） | ✅ 元数据表承载执行入口 + 协议工具独立成表（28 分支 match → 一次查表，内核零工具名字面量）；✅ `ToolContext` 承载六个工具域（`impl ToolContext` + 注册表 `Deref`），依赖边界实测：工具域 executor 引用仅 16 处（随插件搬走）。待办：`thin-channel` + `assembly-manifest`；`AppState` 20 字段契约化 | 工具目录 freshness ✅；工具行为零变化 ✅（1431 单测全绿）；clippy 零警告 ✅ |
| **1** | 工具插件试点（**装配器二分 + tools-fs 骨架已完成**） | ✅ `tianyan-app` bin 包（**清单住 bin**）+ `ToolPluginFactory` 类型擦除 + `create_app_with_plugins` / `start_server_with_plugins*`；✅ `plugins/tools-fs` 独立 crate（`executor::fs` 实现整体迁入）。待续：**整域迁移**——`read_file` 目录模式反向依赖 `executor::fs::execute_list_dir`，故 fs 域必须 6 工具一起搬（read/write/edit/patch/glob/list_dir）才能删 core 侧 | 闸门 A **首测 512 行 ✅**（判据 ≤3.5K）；core 侧暂留（双路径过渡，重名注册自动跳过） |
| **2** | 适配器与任务插件（**清单已普查，按序推进**） | 迁移顺序（按 自包含度×收益）：**0 清 core 死依赖** → 1 `tools-todo`+`tools-goal` → 2 `tools-web` → 3 `storage-vector` → 4 `storage-sqlite` → 5 `task-*`（2 crate）→ 6 `provider-llm`+`provider-embed`。详见下方「期 2 普查结论」 | 各领域 ≤ 2K；全量 release 不劣化 >10% |
| **3** | 策略分离（最难一刀） | 抽 `kernel-impl`（loop/cancel/rollback/stream_forward/working_set）；`context`→`policy-context`；审批→`policy-approval`；`role_learning`→`policy-learning`；`judge`→`policy-review` | **闸门 B**：api ≤ 6K && impl ≤ 15K（或配置外移后 16.1K）；改策略不重编内核；S 级零变更 |
| **4** | 清单收敛与发行版 | 清单完整化、跨插件顺序断言、desktop/headless/minimal 三份清单 | 装配清单 1 页可查；headless 可构建 |

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

## 待决问题（不阻塞期 0/0.5）

1. **采样参数缺口**（2026-10-20 发现）：`temperature` / `top_p` 在 core 是 `Option<f32>` 且恒为 `None`
   → 请求不带这两个参数，走服务端默认；前端硬编码的 `temperature: 0.7` 在 server 侧被丢弃（死参数）。
   用户决定：**等微内核完成后统一处理**（届时归入 `provider-llm` 插件的模型规格配置）。
2. 配置加载（3.3K）是否外移为 `service-config` —— 期 3 决定。
3. 期 3 需重新做一次行数账（`agent` 22,994 行中 10.5K 是测试，真正待判源码 12.5K）。

---

## 参考

- 实测数据与实施清单：`.scratch/microkernel-design/roadmap.md`
- 最终态架构图（A–G，含账本与量化）：`.scratch/microkernel-design/final-architecture.html`
- 度量脚本：`.scratch/microkernel-design/measure-rebuild.ps1`（期 0 迁入 `scripts/`）
- 原则来源：`.scratch/microkernel-design/index.html`（设计稿 v0.1）
- 评审技能：`architecture-review-html-doc`（字号 ≥12、业务流程图、SVG 可放大、渲染自检）
