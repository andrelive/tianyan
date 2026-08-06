# REFACTOR_LOG

core 模块系统性架构重构日志。约束：公开 API 签名与行为完全兼容；每步修改后运行测试无回归。

## 基线（迭代 0）

- 状态：`cargo test -p tianyan-core --lib` = 536 通过 / 0 失败；clippy 零警告
- 发现的问题点：
  1. `agent/loop.rs` 的 `handle_llm_response` 有 9 个参数、140 行函数体，承担 4 个职责（token 累加 / ask_user 检查 / 消息持久化 / 工具执行组装），且 assistant 与 tool 消息的持久化样板完全重复（`message_to_structured` + 更新 parent_id + 容错写入）
  2. `TokenUsage` 累加逻辑在 loop.rs 手写 3 行，agent_core.rs 另有类似手写累加
  3. 大文件（>600 行）共 7 个：lancedb.rs 704 / snapshot 636 / loop.rs 629 / approval.rs 608 / image.rs 593 / agent types.rs 577 / tool_registry mod.rs 567

## 迭代 1：AgentLoop 回合上下文内聚 + TokenUsage 累加去重

### 改动

**1. `common/types/token.rs`：新增 `TokenUsage::accumulate`**
- 新增公开方法 `accumulate(&mut self, other: &TokenUsage)`，消除 loop.rs 中手写 3 行累加
- 纯新增方法，不改变任何既有签名

**2. `agent/loop.rs`：提取 `TurnContext` + `persist_message` helper**
- 新增私有结构体 `TurnContext<'a>`，持有回合级可变上下文（session_id / current_parent_id / total_tokens / messages / stream_sender / turn）
- `handle_llm_response` 参数从 9 个减到 3 个（assistant_msg / turn_usage / ctx），删除 `#[allow(clippy::too_many_arguments)]` 及 279 行旧的"合并反而降低可读性"注释——该结论已过时：9 参数意味着函数承担多职责，拆出上下文结构体后每个职责只有一个数据入口
- 提取 `persist_message` helper：合并 assistant 与 tool 消息持久化的重复样板（`message_to_structured` → 更新 parent_id → `add_structured_message` 容错写入）。日志文本从"持久化 assistant/tool result 消息失败"统一为"持久化消息失败"——仅日志细节，错误被吞、流程继续的行为不变

### 验证

- `cargo test -p tianyan-core --lib`：536 通过 / 0 失败（含 loop 8 个测试 + token 2 个测试 + accumulate 新测试）
- clippy 零警告；fmt 干净

### 决策理由

- 9 参数函数是"数据入口混乱"的信号：调用点需要记住参数顺序，改动易错。TurnContext 把回合级状态收拢为一个对象，后续新增回合级状态（如流式缓冲）无需再改签名
- 持久化样板重复是真实的复制粘贴维护陷阱：此前若在 assistant 分支修复 bug，tool 分支会漏改。合并后只有一处实现
- 日志文本统一是可接受的差异：持久化失败本就只 warn 不中断，文本不属于 API 契约

## 迭代 2：工具执行器样板去重（参数解析 + 安全违规包装）

### 改动

**`agent/tool_registry/executors.rs`：提取 `parse_params` + `safety_violation`**
- 新增模块私有泛型函数 `parse_params<T: DeserializeOwned>(arguments: &str)`：13 个工具的参数解析从 3 行样板（`serde_json::from_str` + map_err "tool: 参数无效"）收敛为 1 行 `parse_params(arguments)?`，类型注解驱动泛型推断
- 新增模块私有泛型函数 `safety_violation<T, E: Display>(result)`：5 处链式安全策略检查的 map_err 包装收敛为 `safety_violation(...)?`
- 保留 3 处直接构造（"操作需要用户确认"变体 + 重写失败分支）：消息结构不同，强行统一反而引入条件分支

### 验证

- `cargo test -p tianyan-core --lib`：537 通过 / 0 失败；clippy 零警告；`cargo check --workspace` 通过

### 决策理由

- 参数解析与安全违规包装是 14 个工具执行器中最高频的重复样板，错误消息文本完全一致，提取后行为零差异
- 泛型 + 类型注解驱动的调用方式保持调用点可读性，不引入 trait 或宏的复杂度
- 遵循 error.rs 已有的 `not_found` 统一构造先例——"统一入口 + 统一消息"是项目既定的错误处理模式

## 迭代 3：审批工作流模块拆分（executor/approval.rs → approval/ 目录）

### 改动

**`executor/approval.rs`（664 行）拆分为 `executor/approval/` 三文件模块**
- `approval/types.rs`：审批领域数据类型——`is_user_confirmation`、RiskLevel、ApprovalDecision、ApprovalRequest/Response/Record、AutoApprovalRule、ActionPattern、ApprovalCondition、ApprovalWorkflowConfig、ApprovalStatusSnapshot
- `approval/workflow.rs`：ApprovalWorkflow 状态机 + 全部实现（自动规则匹配、人工审批通道、审计记录、pending 管理）
- `approval/mod.rs`：`pub use types::*` + `pub use workflow::ApprovalWorkflow`，公开路径 `executor::approval::Xxx` 完全不变
- `approval_tests.rs` 移入目录；tests 挂载点从 approval.rs 变为 mod.rs
- 附带：`check_auto_approval` 从私有提升为 `pub(super)`（仅审批模块内可见，外部 API 不变）——模块拆分后 tests 不再是同文件子模块，无法访问私有方法，最小可见性提升

### 验证

- `cargo test -p tianyan-core --lib`：537 通过 / 0 失败（审批 16 个测试全部通过）；clippy 零警告；`cargo check --workspace` 通过

### 决策理由

- 单一文件同时承载 11 个类型定义 + 350 行状态机逻辑，类型读者与逻辑读者互相干扰。拆分后"类型"与"工作流"各归其位，符合单一职责
- 模块拆分（文件级重组 + re-export）不触碰任何签名与行为，是零风险的内聚性改进路径
- `pub(super)` 比 `pub(crate)` 更克制：可见性只扩大到需要的边界（审批模块 + 其测试），避免无谓扩散

## 迭代 4：图像处理模块拆分（knowledge/image.rs → image/ 目录）

### 改动

**`knowledge/image.rs`（656 行）拆分为 `knowledge/image/` 四文件模块**
- `image/types.rs`：图像领域数据类型——ImageFormatType、ExifMetadata、ProcessedImage、ImageAnalysis、ImageType、UnifiedTextRepresentation、ImageProcessorConfig
- `image/processor.rs`：ImageProcessor（格式转换、压缩、缩略图、EXIF 提取）
- `image/analyzer.rs`：ImageAnalyzer + 私有 analyze_image_base64（VLM 描述生成、统一文本、视觉嵌入）
- `image/mod.rs`：re-export 9 个公开类型 + `#[path = "tests.rs"] mod tests`
- 公开路径 `knowledge::Xxx` 完全不变（`knowledge::image` 本就是私有模块）

### 验证

- `cargo test -p tianyan-core --lib`：537 通过 / 0 失败（图像 5 个测试通过）；clippy 零警告；`cargo check --workspace` 通过

### 决策理由

- 原文件同时承载 7 个数据类型 + 两类处理逻辑（本地图像处理与 VLM 分析），读者需要跨 300 行理解上下文才能定位目标。拆分后 processor 与 analyzer 各自独立可读
- 类型层是共享词汇，先于处理逻辑定义，拆分后 processor/analyzer 均从 `super::types` 导入，依赖方向单向清晰

### 经验教训

- PowerShell 切分行时 `'str' + $array` 是字符串连接而非数组拼接，会把整个文件压成一行——应使用 `@('str') + $array` 或 `Write-Output`
- 切分内联测试模块时注意 `mod tests {` 包装：`#[path] mod tests;` 引用的文件不应自带包装（否则嵌套导致 `use super::*` 失效）；从 git 恢复原始文件重新切分是最可靠的回退手段

## 迭代 5：LanceDB 向量存储拆分（vector/lancedb.rs → lancedb/ 目录）

### 改动

**`vfs/vector/lancedb.rs`（798 行）拆分为 `vfs/vector/lancedb/` 三文件模块**
- `lancedb/batch.rs`：RecordBatch ↔ VectorPoint 转换辅助（append_vector / extract_vector / extract_base_fields / batch_to_point / vector_column_name / batch_to_results，109 行），全部为 `pub(super)` 纯函数
- `lancedb/mod.rs`：LanceDbVectorStore 存储生命周期（建表、CRUD、检索、RRF 融合）+ 依赖 self 字段的 `point_to_batch` 保留在本文件
- `lancedb/tests.rs`：291 行测试经 `#[path]` 挂载
- 公开路径 `vfs::LanceDbVectorStore` 完全不变

### 验证

- `cargo test -p tianyan-core --lib`：537 通过 / 0 失败（lancedb 测试全过）；clippy 零警告；`cargo check --workspace` 通过

### 决策理由

- arrow 列构建/提取是纯数据转换细节，与存储生命周期（表、检索、融合）无耦合，独立成文件后主文件聚焦"存储语义"，读者不再需要跳过 130 行 arrow 样板定位检索逻辑
- `point_to_batch` 依赖 `self.schema`/`self.embedding_dim`，留在主文件避免引入 4 个参数的裸函数——按依赖关系而非主题一刀切

### 经验教训

- PowerShell `$arr[a..b]` 切片是 **end-INCLUSIVE**（与 Rust/其他语言不同）——切分边界多包含一行，导致 impl 闭合 `}` 重复/缺失的连锁问题；验证手段是编译错误定位 + 对照 git 原始文件逐行核对
- 每次切分后运行 `cargo check` 前先数花括号：`Select-String` 输出关键边界行 + `Get-Content | Select-Object -Last N` 核对文件尾

## 迭代 6：最终验收（停止迭代）

### 验收结果

- `cargo test --workspace`：全部通过（core 537 + server 等，0 失败）
- `cargo clippy --workspace`：零警告
- `cargo fmt --all -- --check`：干净
- `cargo check --workspace`：通过

### 本轮评估与停止理由

对剩余大文件逐一评估后判定无实质改进空间：
- `snapshot/mod.rs`（636 行）：单一 SnapshotManager 类型 + 私有辅助，无多职责混杂
- `scheduler/task_scheduler.rs`（556 行）：单一调度器，类型/实现分离已清晰
- `context/compression/mod.rs`（554 行）：单一 ContextCompressor，策略类型即配置
- `agent/types.rs`（577 行）：纯类型定义集中，无逻辑可拆

这些文件是"大但结构合理"——为拆分而拆分只会增加文件导航成本，不提升内聚性。

### 全会话总结（5 轮实质改进）

| 迭代 | 改进 | 类型 |
|------|------|------|
| 1 | AgentLoop TurnContext（9 参数→3）+ persist_message 去重 + TokenUsage::accumulate | 内聚 + DRY |
| 2 | executors 参数解析/安全包装样板去重（parse_params/safety_violation） | DRY |
| 3 | approval.rs 664 行 → types/workflow/mod 三文件 | 单一职责 |
| 4 | image.rs 656 行 → types/processor/analyzer 四文件 | 单一职责 |
| 5 | lancedb.rs 798 行 → batch/mod/tests 三文件 | 单一职责 |

**净效果**：-2432 行大文件重构为模块化目录；公开 API 零变化；测试 537 全绿（+1 新测试 accumulate）；零回归。

## Wave 1a：断开 common↔config 循环依赖（P0-3）

### 改动

**1. `common/logging.rs`：LoggingConfig 移入（与 init_logging 同文件）**
- 结构体（7 字段）+ 默认值函数 + `Default` impl + `validate()` + 2 个配置测试从 `config/logging.rs` 原样迁入
- serde 属性、字段名、默认值逐字节保留（与 HEAD 版本 diff 验证一致），TOML 配置兼容性零变化
- 删除 `use crate::config::LoggingConfig;` —— `core/src/common/` 内不再有任何 `use crate::config`（验收 1 ✓）
- `validate()` 返回类型写为 `std::result::Result<(), String>`：common 的 `Result<T>` 别名只有一个泛型参数（`Result<T, TianyanError>`），被导入后遮蔽 std Result；全限定路径保持原签名语义，`TianyanConfig::validate` 的 `?` 链（String 错误）不变

**2. `config/mod.rs`：改为 re-export**
- 删除 `mod logging;`，`pub use logging::LoggingConfig;` → `pub use crate::common::logging::LoggingConfig;`
- 方向 config→common 合法，`tianyan::config::LoggingConfig` 路径对下游保持可用（API 兼容）
- 删除空壳文件 `config/logging.rs`（全部内容已迁移）

**3. 外部调用点更新**
- `tauri/src/lib.rs` 本地 init_logging：`&tianyan::config::LoggingConfig` → `&tianyan::common::logging::LoggingConfig`
- `server/tests/common/factory.rs`：`LoggingConfig` 从 `tianyan::config` 批量导入中拆出，独立 `use tianyan::common::logging::LoggingConfig;`（struct literal 字段不变）
- `server/src/main.rs` 无需改动（仅用 `config.logging` 字段 + `init_logging`，无类型路径引用）

### 验证

- `grep "use crate::config" core/src/common/`：空
- `cargo check -p tianyan-core`：本任务文件零错误（残留错误全部位于另一 Wave 1b 的 in-flight 文件 `observability/usage_stats.rs` / `vfs/backend/sqlite.rs`）
- HEAD 基线（stash 隔离验证）：`cargo check -p tianyan-core` 干净通过（12.35s），证明 Wave 1b 未落地前基线本绿
- 结构体块与 `git show HEAD:core/src/config/logging.rs` 逐字节一致（regex 提取比较）
- **受阻项**：`cargo test -p tianyan-core --lib` 与 `cargo check --workspace` 无法在共享工作树跑通 —— 同一工作树上有并行 Wave 1b 的半成品（`sqlite_db.rs` 迁移 + `usage_stats.rs` 编译错误），且对方 worker 正在运行构建持有 cargo 锁。协调者合并两个 Wave 后需复跑全量测试（config round-trip 测试 `test_config_serialization` 等随本改动无逻辑变化）

### 决策理由

- 类型归属 common：`init_logging` 的入参类型与行为同属"通用基础设施"，移动后 common 自成一体（不再依赖 config），config 侧单向 re-export 保留下游兼容
- `validate()` 留在结构体旁而非 config：方法是类型的一部分，拆开会产生"类型在 A、行为在 B"的割裂；返回 String 错误与整条配置校验链一致
- 仅更新显式引用 `config::LoggingConfig` 的 2 处外部调用点：re-export 已保证 `tianyan::config::LoggingConfig` 继续有效，但显式路径更清晰，且为未来彻底移除 re-export 留出余地

### 经验教训

- **共享工作树 + 并行 Wave 的互操作**：`git stash` 验证基线会连其他 worker 的 in-flight 改动一起暂存；stash pop 遇对方实时编辑会失败。恢复策略：逐文件 `git restore --source=stash@{0} --worktree`（PowerShell 下 `--source=stash@{0}` 必须整体加引号，否则 `{0}` 被误解析为 switch），索引侧纯重命名靠 `git mv`/索引状态复核，勿直接 stash drop
- 临时 worktree 验证隔离改动时，worktree 路径差异会使共享 target 全量冷编译（lancedb/datafusion 数十分钟级）——此路不可行；且其残留指纹会拖慢并行 worker 的构建。隔离验证应优先静态 diff + 等对方合并后跑全量
- 超时杀进程时须先按 CommandLine 核对归属（`Win32_Process.CommandLine`），只杀自己的 cargo/rustc 树，避免误伤并行 worker 的构建


## 迭代 7：断开依赖环（vfs↔observability、context↔observability）+ 错误类型泄漏修复（Wave 1a）

### 改动

**1. SqliteDb 下沉到 `vfs::backend`（断开 `vfs → observability` 环）**
- `observability/sqlite_db.rs` 整体移入 `vfs/backend/sqlite_db.rs`（git mv，内容不变）
- `vfs/backend/mod.rs` 注册 `pub mod sqlite_db;`；`observability/mod.rs` 删除 sqlite_db 模块声明与 `pub use SqliteDb` re-export（不保留隐藏依赖）
- 引用更新：`vfs/backend/sqlite.rs` 顶部 `super::sqlite_db::SqliteDb`（tests 模块内为绝对路径 `crate::vfs::backend::sqlite_db::SqliteDb`）；`usage_stats.rs` 改 `crate::vfs::backend::sqlite_db::SqliteDb`；server 的 `state.rs` / `lib.rs` 改 `tianyan::vfs::backend::sqlite_db::SqliteDb`

**2. rusqlite::Error → TianyanError（usage_stats 错误类型泄漏修复）**
- `UsageStats::new` / `flush` / `shutdown` 返回类型改为 `Result<_, TianyanError>`，rusqlite 错误经模块私有 `sqlite_error()` 映射为 `TianyanError::Custom("observability: {e}")`
- server `state.rs` 的 `UsageStats::new(...).map_err(...)` 简化为 `?`（new 已返回 TianyanError）
- `SqliteDb` 内部方法保持 rusqlite::Error 不变（边界处包装即可，属既有约定）

**3. RetrievalTrace 家族下沉到 `common::types::retrieval_trace`（断开 `context ↔ observability` 环）**
- `RetrievalStep` / `RetrievalStepType` / `RetrievalTrace`（含 impl）从 `context/retrieval/types.rs` 移入新文件 `common/types/retrieval_trace.rs`，字段名/顺序原样保留（`retrieval_traces.trace_json` 持久化契约 + `gui-vite/src/lib/types.ts` 镜像冻结）
- `context/retrieval/types.rs` 保留 `pub use` 兼容既有引用路径；`usage_stats.rs` 改 `use crate::common::types::retrieval_trace::RetrievalTrace`（依赖方向变为 observability → common，不再反向依赖 context）

### 验证

- `cargo check --workspace`：通过（core/server/tauri/mcp 全部编译）
- `cargo test -p tianyan-core --lib`：**542 通过 / 0 失败**（基线 537 + 5 个新测试）
  - 新增：serde round-trip 字节一致性（锁定 trace_json 契约）、step_type snake_case 序列化契约、`UsageStats::new` 返回 TianyanError 类型断言、坏 DB 路径打开失败、未初始化 schema 时 `flush` 错误映射为 `TianyanError::Custom`
- `grep "use crate::observability" core/src/vfs/` = 空；`grep "use crate::context" core/src/observability/` = 空
- rustfmt：本次改动文件全部格式干净（仅并行波次的 `config/mod.rs` 存在非本任务 fmt diff，未触碰）

### 决策理由

- 共享基础设施放在被依赖方（vfs）而非依赖方（observability），消除 `vfs → observability` 反向依赖；observability 不保留 re-export，避免旧路径残留掩盖真实依赖
- RetrievalTrace 是纯数据契约，放 `common::types` 使其对生产方（context）与持久化方（observability）同为下游，`context ↔ observability` 环自然消除
- 错误映射统一到 `TianyanError::Custom` 并带模块前缀，符合 AGENTS.md「所有错误用 TianyanError、消息带模块前缀」约束；SqliteDb 内部保持 rusqlite::Error 是因其它经 map_err 包装、改内部类型属无收益的扩散性变更

### 经验教训

- **并发波次风险**：任务执行期间工作区存在其他并行修改（LoggingConfig 迁移）；中途一次外部 git 恢复操作回滚了已完成的 `git mv` 与部分文件编辑（usage_stats 部分编辑保留、types.rs/common-types 注册被回滚）。应对：每步编辑后立即用 `git diff` / `Select-String` 核对磁盘真相，被回滚的编辑重新应用，最终以 `git diff` 全量核对为准
- `super::` 在 `#[cfg(test)] mod tests` 内指向 tests 的父模块（`sqlite`），而非 `backend` —— tests 模块内引用兄弟模块需绝对路径，`cargo check`（不含 cfg(test)）无法发现此类错误，必须跑 `cargo test` 编译
- 首次 `cargo test -p tianyan-core --lib` 需冷编译 lance/datafusion 依赖链（debug profile），耗时可能超过 10 分钟，期间与并行波次的 cargo 构建互相阻塞，需放大超时
