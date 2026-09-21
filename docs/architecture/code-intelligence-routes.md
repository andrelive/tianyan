# 代码智能路线决策（grep / tree-sitter / LSP / code graph）

> 状态：**已决策（2026-09-21）**——LSP 选定 **C 彻底移除**（已实施，工具数 34 → 33）；
> `repo_map` **立项并落地**（工具数 33 → 34；阈值二次校准与「精确语义」复评见 §10）。
> 原始数据与对照分析保留如下。数据截至 2026-09-21，数据源为应用 SQLite
> （`{data_dir}/tianyan.db` 的 `executions` / `usage_logs` / `vfs_entries` 表）。
> 本文档记录四条"让助手看懂代码"的技术路线的能力／成本对照，以及推荐形态。

## 1. 问题

助手要做两件事时都需要"看懂代码"：

1. **建立代码地图**：这个仓库有什么、在哪、怎么分层；
2. **评估重构**：改动会影响谁、哪里是耦合热点。

四条路线可选：文本检索（grep）、语法解析（tree-sitter）、语义查询（LSP）、
结构图（code graph / repo map）。取舍维度：**依赖成本**（要不要装东西）、
**精度**、**token 效率**、**CPU／常驻开销**。

## 2. 四条路线对照

| 路线 | 代表实现 | 外部依赖 | 精度 | token 效率 | CPU |
|------|----------|----------|------|-----------|-----|
| 文本检索 | `grep` / `glob` | 无 | 低（文本匹配） | 中（有 2000 字符截断层保护） | 低（实测平均 437ms） |
| 语法解析 | `symbol_outline`（tree-sitter） | 无（grammar 编入二进制） | 单文件语法级 | 中（实测平均 1,875 字符） | 低（按需，秒级内） |
| 语义按需 | `lsp`（LSP 客户端） | **需装语言服务器** | 高（真实语义） | **当前偏低**：结果透传、无截断 | **高且常驻**（见 §4） |
| 结构图 | repo map / code graph | tree-sitter 级=无；精确级=需索引器 | 近似 → 精确 | **高**（压缩子图） | 低（构建短促，可缓存） |

## 3. 本机实证数据（2026-09-21）

`executions` 表共 **17,599** 条工具调用：

| 工具 | 次数 | 说明 |
|------|------|------|
| `execute_command` | 5,664 | |
| `read_file` | 3,935 | |
| `grep` | 2,715 | 成功率 99% |
| `apply_edit` / `apply_patch` | 1,501 / 1,149 | 合计 2,650 次编辑 |
| `run_tests` / `verify_build` | 190 / 43 | 编译器级验证 |
| `symbol_outline` | **60** | 零依赖的语法级大纲 |
| **`lsp`** | **2（0.011%）** | **两次全部失败** |

### 3.1 `lsp` 的两次调用（唯一历史记录，6.3 天前）

```
goToDefinition  {"file_path":"core/src/vfs/vector/lancedb/mod.rs","line":305,"character":15}
  → tool: 执行失败：executor: lsp: 服务器 rust-analyzer 不可用：连接已关闭，
    安装提示：rustup component add rust-analyzer
hover           同上参数 → 同样失败
```

**结论：LSP 能力在本机从未成功执行过一次。**

### 3.2 隐性成本（代码级证据）

| 事实 | 位置 | 后果 |
|------|------|------|
| 服务器只在 `lsp` 工具路径启动 | `lsp/diagnostics.rs:153`（`ensure_server`）唯一调用方是 `:223` 的 `query`，而 `query` 只被 `execute_lsp` 调用 | 编辑／验证路径**永不启动**服务器 |
| 编辑后诊断只"读"不"启" | `tool_registry/file_ops.rs:179`（`attach_lsp_diagnostics` 只查 store） | **2,312 次编辑结果带 `diagnostics` 字段，100% 是空数组**（纯 token 噪音） |
| 服务器池无空闲回收 | `lsp/diagnostics.rs` 的 `servers` 只在"客户端已死"时逐出 | `LspClient::drop`（`lsp/client.rs:413`）才会 `start_kill`，但池持 `Arc` 永不 drop ⇒ **调用成功后服务器常驻到天演退出** |
| 结果透传无截断 | `tool_registry/lsp_ops.rs` 直接返回服务器原始 JSON | 所有自研工具受 2000 字符截断层（`executor/truncate.rs`）保护，**LSP 路径不受** |
| 失败会生成知识条目 | 见 `tianyan://agent/learned/rule-20260915-011229`（本次 `search_vfs` 命中） | 失败尝试 → 规则学习 → 知识库噪音 + 向量索引 + 嵌入消耗 |

### 3.3 反向数据：检索需求主要由 grep 满足

`grep` 2,715 次（99% 成功、平均 437ms）vs `symbol_outline` 60 次 vs `lsp` 2 次。
说明模型当前的检索需求绝大多数是"找字符串／找段代码"，**尚未表现出对全局结构的刚性依赖**。

### 3.4 嵌入链路健康（对照组）

`usage_logs`：`bailian` / `text-embedding-v4` 共 **1,035 次**、365,092 token；
闭环验证（调用前的 1,034 次 / 14:46 → 调用后 1,035 次 / 16:03）确认 query
embedding 实时生效；LanceDB 持续写入。**VFS 检索链路无静默失败。**

## 4. 为什么"LSP 省 token"在当前实现下不成立

按操作分：

| `lsp` 操作 | 返回体量 | 对比 grep/tree-sitter |
|-----------|---------|----------------------|
| `goToDefinition` / `goToImplementation` | 少数位置（约 200–400 字符） | ✅ 确实省（grep 需多轮试探 + `read_file`） |
| `hover` | 类型／文档 | ✅ 独门能力（grep 原理上做不到） |
| `findReferences` | **所有引用点**（可几十上百条） | ⚠️ 可能很大，且无截断 |
| `documentSymbol` | **完整符号树**（range/detail/children） | ⚠️ 比 `symbol_outline` 更啰嗦 |
| `workspaceSymbol` | 全工作区匹配 | ⚠️ 最大 |

要让 LSP 真正划算，必须同时补三件事（缺一不可）：

1. **结果裁剪/结构化**：`references`/`documentSymbol` 走统一截断层 + 字段精简；
2. **服务器生命周期**：空闲 N 分钟关闭／会话结束关闭（否则 CPU 常驻）；
3. **降级引导**：未装服务器时明确指示改用 `symbol_outline`/`grep`（而非失败一次就放弃）。

## 5. repo map / code graph 的现成方案调研

| 方案 | 语言/形态 | 可复用性 | 结论 |
|------|-----------|---------|------|
| **aider `repomap.py`** | Python | 思路成熟：tree-sitter 抽符号 + PageRank 排序 + 磁盘缓存 + token 预算（`--map-tokens`） | **思路可直接移植**；无 Rust 现成实现，需自写 |
| **stack-graphs**（GitHub） | Rust 库 | 精确跨文件名字解析（code navigation 用）；grammar 覆盖有限、构建成本高、API 陡 | 精度诱人但过重，不适合首版 |
| **rust-analyzer 库化**（`ra_ap_*`） | Rust 库 | 进程内语义查询，**无服务器常驻**；但把一大票 crate 编进天演，二进物体积与构建时间显著上升 | 备选：真需要精确语义时再评估 |
| **SCIP / LSIF**（Sourcegraph） | 索引格式 + 索引器 | 精确，但需外部索引器与 CI 式构建 | 对"通用助手"定位过重 |
| **universal-ctags** | 外部二进制 | 符号索引（无引用图） | 需外部依赖，且能力弱于已有 tree-sitter |

**结论**：没有"拿来即用"的 Rust crate；最务实是**移植 aider 思路并复用天演已有的
tree-sitter 基础设施**（`core/src/executor/symbols.rs` 已支持 Rust/TS(.tsx)/JS/Python/Go）。

## 6. 推荐形态

| 形态 | 做法 | 理由 |
|------|------|------|
| **A. `repo_map` 工具（推荐）** | 新增工具，**按需调用**（模型看到工具自行拉取，与 `symbol_outline` 60 次的成功先例一致） | 零会话启动成本、零无关开销；token 只在真正需要时消耗 |
| **B. 会话开场"仓库名片"（可选）** | 极小摘要（仓库类型 + 顶层模块 + 入口文件 + **已有 ADR/文档清单**），可缓存到会话 | 对"摸清架构"最有效的是**指向文档**，而非符号清单 |
| **C. 重构验证靠编译器** | 影响面靠 `verify_build`（`cargo check`）+ `run_tests` | 名字匹配级的图会漏（宏/重导出/动态分发）；**编译器给的是精确影响面**，图只做侦察 |
| **明确不做** | 会话开始自动构建/注入全仓库索引 | 为无关会话白烧 CPU 与 token（rust-analyzer 病） |

**架构 ≠ 图**：架构包含分层意图与边界约定（如 `AGENTS.md` 的硬约束、ADR 例外），
这些只在文档里；图能补的是"文档说的分层是否被真正遵守"这类结构核查。

## 7. `repo_map` 落地方案（已实施，工具数 33 → 34）

> **实测（2026-09-21，本仓库）**：416 文件 / 5753 符号 / 冷扫描 ~2.7s、缓存命中 114ms、默认预算 4096 字节 → 80 条。首版排序被「通用名」淹没（前 20 行全是 `path(1100)` / `name(1099)` 这类 getter/trait 方法）；经三层修正（方法不进地图 / 通用名过滤 / 通用方法名过滤 + kind 权重 + 扣除定义处计数）后，顶层为 `Module content(787)` / `Module vfs(711)` / `Enum TianyanError(254)` / `Struct TianyanUri(233)` 等真实构件——即「按需 + 零常驻」路线确实能产出可用的架构地图。

**工具形态**（待评审）：

```jsonc
repo_map: {
  "path": "core/src",      // 可选：限定子树（缺省会话工作目录）
  "focus": "session",      // 可选：关键字，命中符号优先排序
  "max_tokens": 1500       // 可选：骨架 token 预算（缺省 1000，硬上限 2000）
}
```

**算法**：tree-sitter 扫描 → 全局定义表（file:line + kind + name）→ 名字引用计数
（跨文件文本级计数，不做语义解析）→ `focus` 命中加权 → 排序取 Top N → 输出骨架。

**缓存与失效**：键 = 仓库根 + `(相对路径, mtime, size)` 集合哈希；内存 LRU
（不落盘、不进 VFS）；失效时只重扫变更文件。构建延迟目标：≤ 2s（本仓库规模）。

**边界与约束**：

- **AGENTS.md 硬约束**：这是**运行时结构缓存**（与 LSP 服务器池、snapshot 同类），
  **不是第二存储、不是第二个检索管道**，不得写入 VFS 内容或独立数据库；
- 语言覆盖沿用 `symbols.rs` 现有 5 种，不新增 grammar；
- 输出必须走统一截断层（`executor/truncate.rs`），与自研工具同口径；
- 近似性必须在工具描述中显式声明（"名字匹配级近似，精确引用请改后跑 `verify_build`"）。

**验收标准（判别力测试）**：① 二次调用命中缓存不重扫（用 mtime 计数断言）；
② 修改单个文件后仅该文件重扫；③ 超预算时按排序截断并置 `truncated`；
④ 非法 `path` 明确报错；⑤ 无任何外部进程/依赖。

## 8. 待拍板事项

| 事项 | 选项 | 备注 |
|------|------|------|
| LSP | **B 显式可选**（配置开关，默认不注册工具）／**C 彻底移除**（34→33 工具、删 `lsp_types` 依赖） | 数据支持 C（零成功、持续成本）；B 保留后路 |
| `repo_map` | **已立项并落地**（工具数 33 → 34） | 阈值二次校准与「精确语义」复评见 §10（2026-09-21 二次评估） |

## 9. 附：结论速记

- 四条路线的排序不是"精度越高越好"，而是**按需 + 零常驻**优先；
- LSP 的精度优势真实存在，但**当前实现把它的 token 优势抵消了**（无截断）且**CPU 成本被放大**（常驻无回收）；
- 重构场景的精确影响面**编译器免费提供**，不需要额外的图；
- 图的价值场景是"大仓库 + 跨文件追踪 + 耦合热点侦察"，本仓库规模下边际收益有限。

## 10. repo_map 阈值校准与「精确语义」复评（2026-09-21 二次评估）

> 探针：`core/examples/repo_map_probe.rs`（评估工具，不参与产品逻辑；用法见 §10.5）。
> 本仓库实测：**416 文件 / 5758 定义 / 4023 个名字 / 8781 个引用键**（`include_tests=false`）。

### 10.1 结论：重名度阈值 `MAX_SHARED_DEFINITIONS` 6 → **8**（已落地）

固定其余过滤，只改重名度阈值（探针 §C）：

| 阈值 | 保留条目 | top-10（`名称(引用数,kind)`） |
|------|---------|------------------------------|
| 5 / 6（原） | 3242 / 3246 | content vfs state model model store session message **manager manager** |
| **8（新）** | 3261 | **config config uri** content vfs state model model store **error** |
| 10 / 12 / 20 | 3293 / 3300 / 3322 | 与阈值 8 **完全一致**（已收敛） |

**根因**：`config`（定义 7 次 / 被引用 **1028** 次）、`uri`（7 / **925**）、`error`（7 / **472**）
是本仓库最核心的三个模块，却因定义次数恰好 **7** 被阈值 6 判为「通用名」剔除。它们的 7 次定义
只是「`mod` 声明 + 模块文件」结构的必然结果，**不是**「互不相关的同名方法」——而后者才是该阈值
要挡的东西（`name` 1099 次 / `path` 1102 次定义、`tests` 174 次、`impl` 457 次）。

**取 8 的依据**：定义次数是长尾分布（4023 个名字中 90% 只定义 1 次，定义 ≥6 次的仅 48 个 = 1.2%）；
阈值 8 / 10 / 12 / 20 的 top-10 完全一致 ⇒ **8 是拐点**，再放宽只会放入低引用的同名 `mod`
（`types` 27 次引用 / `handlers` 5 / `services` 2）——无收益。

**同步改动**：`core/src/executor/repo_map.rs`（常量 + 依据注释）；`repo_map_tests.rs` 增边界判别力断言
（定义 `MAX−1` 次的核心模块**保留**、`MAX` 次被剔除）。

### 10.2 其余阈值复评（未改）

| 项 | 复评结论 |
|----|---------|
| `is_generic_method_name`（短 / 全小写 / 无下划线 / ≤6） | **有效**。被它砍掉的 62 个 Function 条目里，前几名 `entry` 543 / `msg` 489 / `task` 310 / `cfg` 284 的「引用数」**全部来自局部变量与标准库方法**（`.entry()` / `.len()` 等），不是构件。副作用：顺带砍掉本仓库少量真实小函数（`scan` 6 / `render` 5 / `init` 5），引用数太低不影响排序。 |
| `MAX_ENTRIES = 600` | **死配置**。预算硬上限 16 KB，实测最多 308 行（平均 ~53 字节/行），永远到不了 600。列为可选清理项。 |
| `DEFAULT_MAX_BYTES = 4096`（≈1000 token） | 合理。实测 4096 → 83 条 / 4.2 KB；8192 → 162 条；16384 → 308 条（对应 `max_tokens` 1000 / 2000 / 4000）。 |
| 预算双层钳制（`clamp(256, 16 KB)`） | 无问题（`resolve_budget(Some(0)) = 256` 有测试锚）。 |

### 10.3 「精确语义」路线（`ra_ap_*`）实测**否决**

§5 曾把 rust-analyzer 库化列为「真需要精确语义时再评估」。本次在临时 crate 中引入
`ra_ap_ide = "=0.0.349"`（当日 latest 0.0.352）实测：

| 维度 | 实测结果 |
|------|---------|
| 依赖规模 | **+181 个包**（`cargo tree` 636 行；`ra_ap_*` 家族 28 个 crate，含 proc-macro）；本仓库 `Cargo.lock` 现为 1011 包 |
| MSRV | latest **0.0.352 要求 rustc 1.98**（本机 1.97.1）⇒ 只能用旧快照（0.0.349 要求 1.95/1.98 混杂） |
| 构建 | **三轮尝试全部失败**：① `ra-ap-rustc_lexer` —— `unicode-ident 1.0.26`（Unicode 18）与 `unicode-properties 0.1.4` 的 const 断言冲突（须 pin 到满足其 `^1.0.22` 的版本才过）；② `ra_ap_span` —— `salsa 0.28.4` 移除 `plumbing::HashEqLike::hash`（E0407 / E0782，pin `salsa 0.28.2` 才过）；③ `ra_ap_base_db` —— `salsa-macros 0.28.2` 生成代码与 RA 代码不匹配（E0046 `missing hash in implementation`） |
| 语言覆盖 | **仅 Rust**（天演 5 语言：rust / ts / tsx / js / py / go） |
| API 稳定性 | crate 自述「Automatically published version of the package …」——**内部 crate 的自动发布快照**，无 API 稳定性承诺，发布节奏约每周一版（0.0.349 → 0.0.352 四周） |
| 额外前置 | 真要用语义查询需 `ra_ap_load-cargo` 加载项目（`cargo metadata` + sysroot / `rust-src`），在「用户任意仓库」场景失败面大 |

**结论：不投入。** 重评触发条件（三者同时满足）：① `repo_map` 高频真实使用后，
「同名混淆 / 引用度失真」被证明是**实际瓶颈**；② 该瓶颈主要发生在 Rust 仓库场景；
③ 上游修好 crates.io 侧的可构建性（或天演愿意 pin 一串传递依赖并随 RA 周更维护）。

### 10.4 对症的中间路线（**未实施**，备选）

现存三个局限的根因是**同一个**——符号名**未限定命名空间**：

1. **重名阈值误伤**（§10.1）：`config` / `uri` / `error` 被当通用名；
2. **同名声明引用数虚高**：`Module manager` 的 4 行各记 `261` 次——这 261 是 4 个不同模块引用的**合计**；
3. **重复条目占预算**：默认 83 行里 **16 个重名组、26 行冗余（31%）**（`manager` ×4、`roles` ×4、
   `error` ×3、`ApiError` ×3、`chat` ×3、`Session` ×3、`TokenUsage` ×3、`embedding` ×3、`config` ×2 …）。

**限定名（qualified name）路线**可同时消除三者：定义侧用**语法容器链**限定
（Rust `impl` / `mod`、TS `class` / `namespace`、Python `class`、Go receiver——`symbols.rs`
已用 `impl` 祖先链判定 `SymbolKind::Method`，基础设施在），得到 `Foo::path`、`session::manager`
这类唯一名 ⇒ 不再需要粗糙的重名度过滤、引用数按限定名分账、重复条目自然消失。

**代价与边界**：引用侧仍是文本级（`self.path` 与 `x.path` 不做类型推断），`path` 的引用数
只能按名字空间**近似分账**；跨语言同名（`ApiError` 在 core = Struct / gui = Class / server = Enum）
不可合并。成本估：中等（每语言容器规则 + 限定名装配 + 引用计数改造 + 测试）。

**建议：先不实施。** 理由与 §10.2 同类——`repo_map` 至今**零真实调用**
（`executions` 表 `tool_name='repo_map'` 计数 = 0；工具当日落地，且运行中会话的工具清单是
启动时快照，新工具需新会话可见）。应先取得真实使用数据（是否被调用、输出是否够用、
`focus` / `path` 是否真被用），再决定是否为精度付费。§10.1 的阈值校准是**数据已证实的缺陷修复**，
与使用量无关，故先行落地。

### 10.5 附：探针用法

```powershell
cargo run -p tianyan-core --example repo_map_probe -- <仓库根> [--include-tests]
# A 基础统计 / B 定义次数分布 / C 阈值敏感度 / D 重名度误伤候选
# E 通用名误伤候选 / F 预算覆盖度 / G 不过滤 top40 / H 默认地图全文
```
