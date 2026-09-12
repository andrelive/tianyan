# REFACTOR_LOG

> **仅历史**（2026-08 迭代快照）：本文档记录 2026-08 期间 core 模块的系统性重构过程，
> **不代表当前代码状态**（其中的文件路径、测试数量等已演进——如 `tool_registry/executors.rs` 已拆分、
> 测试数 577 → 1177）。现状以代码与 `docs/architecture/` 为准；新的重构记录写入对应 ADR。

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
## Wave 1b（T4）：SummaryEngine 删除死参数（P0-4）

### 改动
- `vfs/summary/engine.rs`：`SummaryEngine::new` 4 参数 → 2 参数（删除 `_embedding_service` / `_embedding_model_name` 两个从未使用的死参数）；`use crate::model::{ChatService, EmbeddingService}` → 仅 ChatService —— vfs→model 依赖边收窄（残留 ChatService 为摘要生成所需，非环）
- 调用点同步：`server/src/state.rs`（删除 embedding_model 解析 + 简化调用）、`knowledge/ingestor/mod.rs`（删除 2 参数）、`gc_task.rs` 测试（删除 embedding 变量与 import）
- engine.rs 7 处测试调用同步简化，删除 7 处 `let emb` 声明

### 验证
- `cargo test -p tianyan-core --lib`：542 通过 / 0 失败；零 warning
- `cargo check --workspace`：通过
- grep `_embedding_service|_embedding_model_name`（SummaryEngine 相关）= 0（test_utils 的 `_embedding_service` 字段为 TestVfs 自有字段，非本次范围）

### 决策理由
- 死参数是"设计声明与实际不符"的直接证据（traits.rs 声称 VFS 经 EmbeddingProvider 反转，engine 却携带未用依赖）；删除后构造 API 反映真实依赖

## Wave 2（T7）：knowledge/ingestor token 估算口径统一（P2-3b）

### 改动
- `core/src/knowledge/ingestor/mod.rs`：删除本地 `count_tokens`（`text.len() / 4` 字节口径）私有函数，3 处调用点全部替换为统一的 `estimate_tokens`（`crate::common::token_estimator::estimate_tokens`，字符类启发式：中文 1.5 字符/token、英文 0.75 词/token）：
  - `ingest()` 的 `tokens_processed` 统计
  - `process_document()` 元数据 `total_tokens`
  - `process_image()` 元数据 `total_tokens`
- import 选择 `crate::common::token_estimator::estimate_tokens`（非 `context::compression` re-export）：ingestor 现有 import 均为 `crate::common::` 风格，且 common 是叶模块，最小依赖；context 不反向依赖 knowledge，两者均可选，取最小改动
- 测试：新增 `test_token_estimation_matches_unified_caliber` —— 中文文本 `estimate_tokens` > 0 且 < `len() / 4`（证明旧字节口径被更合理的字符类口径替代），并断言与全局统一估算器输出一致

### 外部行为变化（必须注意）
- `ingest` API 返回的 `tokens_processed` 数值口径改变：从字节数/4 变为字符类估算。对纯中文文本，新口径约 0.67 字符/token vs 旧口径 0.75 字符/token（UTF-8 中文 3 字节/字符），数值略降；中英文混合文本方向不定。server 层不消费该字段（`server/src/api/knowledge/services.rs` 仅读 `document_id`），无需 server 适配

### 验证
- `cargo test -p tianyan-core --lib`：**543 通过 / 0 失败**（基线 542 + 1 个新测试），零 warning
- grep `count_tokens|\.len() / 4` `core/src/knowledge/` = 空

### 决策理由
- 字节数/4 是第三套估算口径（且中文字节/字符比 3:1 使统计失真），统一到全系统唯一的 `estimate_tokens` 后，ingestor 报告的 token 统计与其他模块（context/retrieval）口径一致，用户看到的数值可信
- 无精确数值断言测试需迁移：ingestor 现有测试（config/infer_category/hash）均不触及 token 数值，故只补口径正确性断言而非改期望值

## Wave 2（T6）：SummaryTask 修复字节/token 误判 bug（P0-5a）

### 改动
- `scheduler/tasks/summary_task.rs` `process_uri`：短内容判断由 `detail_content.len() < ABSTRACT_TOKEN_LIMIT * 4`（字节数，400 字节阈值）改为 `estimate_tokens(&detail_content) < ABSTRACT_TOKEN_LIMIT`（token 数，100 token 阈值），语义与 L0 Abstract 约 100 token 的定位对齐
- 新增 `use crate::common::token_estimator::estimate_tokens;`（T5 统一后的唯一估算入口）
- 回归测试 4 个（复用 `test_utils::MockVfs` + `MockChatService` + 真实 `SummaryEngine` 包装 mock chat 的既有模式，与 gc_task 测试同构）：
  - 短中文（96 汉字 ≈ 288 字节 / 64 token）→ 短路径：Abstract == 原文，且 mock 无期望（被调用即 panic）证明未调 LLM
  - 长中文（312 汉字 ≈ 936 字节 / 208 token）→ LLM 路径：Abstract/Overview 均为 mock 输出且 ≠ 原文
  - 长中文不写全文进 L0：Abstract 必须为摘要输出
  - **边界回归（RED 核心）**：144 汉字（432 字节 ≥ 400 旧阈值，但 96 token < 100）→ 必须仍走短路径

### 验证
- RED：先加测试，`cargo test -p tianyan-core --lib scheduler::tasks::summary_task` 边界测试失败——`left: "模拟摘要" / right: <原文 144 字>`，证明字节判断把 <100 token 的中文误送 LLM 摘要
- GREEN：修复后同一命令 8 passed / 0 failed；全量 `cargo test -p tianyan-core --lib` = **547 passed / 0 failed / 1 ignored**（基线 542 + 本任务 4 新测试 + 并行波次 1 新测试；ignored 为既有 session::manager 忽略项）
- `grep -n "ABSTRACT_TOKEN_LIMIT \* 4\|\.len() <" core/src/scheduler/tasks/summary_task.rs` = 空
- rustfmt：本次改动文件干净；clippy：summary_task.rs 零警告（余下 5 个警告均为其他模块既有/并行波次产物）

### 决策理由
- 字节数 ≠ token 数：中文 3 字节/字符使 400 字节 ≈ 133 汉字 ≈ 89 token，阈值两侧都存在误判（token 密集 ASCII 短字节长 token 直接全文入 L0；134–149 汉字字节超限但 token 不足被误送 LLM），统一用 `estimate_tokens` 后判断口径与 L0 语义一致
- 边界回归测试用「字节超旧阈值 + token 低于阈值」双前置断言构造分歧输入，使测试在 bug 存在时必然失败、修复后必然通过，且不依赖 mockall verify（默认 times 范围 0..=MAX，短路径下未调用不报错）
## Wave 2（T8）：P0-5b 显式索引标记 —— 决策：SKIP

### 决策
不引入"写入方显式标记已索引"机制。

### 理由
1. T6 已修复正确性 bug（字节/token 误判），L0 摘要污染问题消除，无失败行为支撑新机制
2. 显式索引标记 = 新功能面（VFS metadata 契约 + SummaryTask 扫描逻辑 + 前端/API 可能受影响），收益（写入方知晓索引时机）不构成当前架构缺陷
3. 现有 SummaryTask 兜底扫描（5 分钟间隔）在数据量增长前是可接受的延迟；若未来成为瓶颈，再引入写入触发队列（届时作为独立 ADR 决策）

### 验证
- 决策已记录；无代码改动

## Wave 3（T12）：统一 extract_build_errors 双实现（P1-8）

### 改动
- `core/src/executor/output_parse.rs`（新建）：单一 `pub(crate) fn extract_build_errors(stdout, stderr) -> Vec<String>`，合并两套语义：
  - **大小写不敏感匹配** `error:` / `error[`（原 `verification.rs` 行为，`to_lowercase` 后 contains）
  - **每行 trim 首尾空白**（原 `actions.rs` 行为）
  - **单行截断 200 字符**（原 `verification.rs` 行为；改为 `chars().take(200)` 按 char 截断——原实现的 `line[..line.len().min(200)]` 字节切片在 200 落在多字节 UTF-8 字符中间时会 **panic**，统一版顺带修复该隐患）
  - **50 条上限 + 截断标记**（原 `actions.rs` 行为；原实现 `errors.len() > 50` 在 push 之后判断，实际放行 51 条 + 标记，统一版收敛为严格 50 条 + 标记）
- `executor/actions.rs`：删除本地 `extract_build_errors`（原 448-463），`execute_verify_build` 调用点改为委托 `crate::executor::output_parse::extract_build_errors`
- `executor/verification.rs`：删除本地 `extract_build_errors`（原 117-128），`verify_build` 的 pattern 提取改为委托统一函数；原 4 个提取测试迁移至 output_parse.rs（stdout/stderr/clean/case-insensitive 行为不变）
- `executor/mod.rs`：注册 `mod output_parse;`
- 测试：output_parse.rs 共 7 个（4 个迁移 + 3 个新增：大写 `ERROR:` 匹配、`error[E0308]` 匹配、51 行 → 50 条 + 截断标记、>200 字符行截断、trim 行为、无错误空结果、stdout/stderr 来源）

### 外部行为变化（必须注意）
- `run_tests`/`verify_build` 工具返回 JSON 的 `errors` 字段内容可能变化：
  - 匹配更全：大写 `ERROR:` / `Error[` 等此前 `actions.rs` 大小写敏感版漏掉的行现在会进入 `errors`（`verify_build` 的 `error_count` 可能上升）
  - 行内容变短：统一后每行 trim + 200 字符截断，此前 `actions.rs` 版不截断的长行会被裁剪
  - 上限从实际 51+标记 收紧为 50+标记
- 已核实 server 层无依赖该 JSON 格式的解析代码，前端仅展示用，可接受

### 验证
- RED：新建 output_parse.rs 仅含测试时，`cargo test -p tianyan-core --lib executor::output_parse` 编译失败——7 处 `E0425: cannot find function extract_build_errors`（编译器提示到 verification 的私有同名函数，证明统一函数尚不存在）
- GREEN：实现后同一命令 7 passed / 0 failed；executor 模块 44 passed / 0 failed
- 全量 `cargo test -p tianyan-core --lib`：**556 passed / 0 failed / 1 ignored**（基线 547 + 本任务净增 3 测试 + 并行任务 6 个；ignored 为既有 session::manager 忽略项）
- grep `fn extract_build_errors` `core/src/executor/` = 仅 output_parse.rs 1 处定义；rustfmt 4 个改动文件干净
- clippy 全库暂不可跑：并行任务对 `agent/loop.rs` 的在途改动（E0106）阻塞编译，非本次改动引入

### 决策理由
- 同一模式匹配双实现是"输出解析契约"分裂：`run_tests` 返回的 `errors` 与 `verify_build` 的 `pattern_errors` 对同一输出可能给出不同列表，LLM 看到错误列表与判定依据不一致。统一后单一实现保证两处一致
- 语义合并取向：匹配取 verification 版（大小写不敏感更全）、输出取 actions 版（trim + 上限 + 标记，防 LLM 上下文被超长/超量输出淹没）——两版各自的"强项"保留
- 上限 off-by-one（51 vs 50）与字节切片 panic 属实现缺陷，测试锁定任务描述语义（50 条上限），修复顺带完成


## Wave 3（T9）：清理 Agent 结构体死字段（P1-2）

### 改动
- `core/src/agent/agent_core.rs`：`Agent` 结构体删除 5 个"为未来协调器 API 预留"的从未被读取字段：`config` / `model_service` / `vfs` / `skill_executor` / `skill_registry`（证据：全仓 `agent.(config|model_service|vfs|skill_executor|skill_registry)` 与 agent 模块内 `self.<字段>` 均无读取点；`Agent::new` 的 12 参数同步减为 7 参数）。删除 `#[allow(dead_code)]` 及 `#[allow(clippy::too_many_arguments)]`（7 参数低于 clippy 阈值 8），同步清理 4 个随之失效的 import（`AgentConfig` / `ChatService` / `VirtualFileSystem` / `SkillExecutor`+`SkillRegistry`）
- 保留字段（有真实读取点）：`default_model`（coordinator.rs:165/274、agent_core.rs:275）、`context_pipeline` / `metrics` / `skill_learning_engine` / `state` / `agent_loop` / `session_manager` / `snapshot_manager`
- `core/src/agent/builder.rs`：`with_skill_registry` 是唯一变死的 with_* 方法（其值只流向被删的 Agent 字段，删后 builder 私有字段无人读取 → 编译器 dead_code），删除方法 + `AgentBuilder.skill_registry` 字段 + `build()` 内局部 `skill_registry`；`with_config` / `with_model_service` / `with_vfs` / `with_skill_executor` 保留（build() 内仍读取喂给 AgentLoop/ToolRegistry/ContextPipeline）
- `server/src/agent_builder.rs`：删除 `.with_skill_registry(...)` 调用；`build_agent` / `build_agent_or_wizard` 删除 `skill_registry` 参数（已无消费方）
- `server/src/state.rs`：两处 `build_agent_or_wizard(...)` 调用删除对应实参；`skill_registry` 字段与访问器保留（skills API 仍使用）

### 验证
- `cargo check -p tianyan-core`：0 错误（仅 1 个既有无关 warning：`executor/output_parse.rs::extract_build_errors`，并行波次产物，非本任务文件）
- `cargo check --workspace`：通过
- `cargo test -p tianyan-core --lib`：**556 passed / 0 failed / 1 ignored**（基线 547 + 并行波次新增 9；knowledge ingestor 单个用例首轮偶发失败，隔离重跑通过，并行波次改动所致非本任务回归）
- `grep allow(dead_code) core/src/agent/agent_core.rs` = 空

### 决策理由
- 以编译器为准删除：先删字段跑 check，无使用点报错即确认死字段；`with_skill_registry` 因唯一消费者（Agent.skill_registry）被删而变死，连带 server 参数链一并清理（保留会触发 dead_code warning）
- 不删仍被内部读取的 with_* 方法（config/model_service/vfs/skill_executor 在 build() 中继续喂给 AgentLoop/ToolRegistry/ContextPipeline/SkillLearningEngine）
- 未重构 AgentCoordinator 实现逻辑（T10 范围）

## Wave 3（T13）：消除 ingestor 摘要失败双层回退（P1-10）

### 改动
- `core/src/knowledge/ingestor/mod.rs` `generate_summaries()`：删除 abstract/overview 各自的 `unwrap_or_else` 截断回退（`chars().take(500)` / `take(2000)`），改为 `?` 直接传播错误；摘要失败的回退仅保留在 `ingest()` 外层一处（截断 500 字符 abstract + overview 全文 + warning）
- 删除后行为变化：失败时 abstract 仍截断为 500 字符（不变），overview 从 `take(2000)` 变为完整内容（与回退语义一致：无摘要可用时保留全文），且新增 warning 提示（此前内层吞错导致无 warning，用户无从得知摘要失败）
- 测试：新增 `test_ingest_summary_failure_single_layer_fallback` —— `MockChatService` 的 `chat_completion` 返回错误模拟摘要失败，走完整 `ingest()` 流程断言：恰好 1 条 warning（含"摘要生成失败，使用截断回退"）、abstract 落库 500 字符、overview 落库为完整内容、不 panic

### 验证
- RED：加测试后运行 `cargo test -p tianyan-core --lib knowledge::ingestor` 失败——`assertion left: 0 right: 1`（warnings 为 0，证明旧代码内层吞错无 warning）
- GREEN：删除内层回退后同命令 5 passed / 0 failed；全量 `cargo test -p tianyan-core --lib` = **556 passed / 0 failed / 1 ignored**
- grep `take(2000)` = 空；`take(500)` 仅剩 2 处（`:185` 外层摘要失败回退 + `:192` 禁用摘要配置路径，均为 ingest() 单一决策点）；摘要相关的 `unwrap_or_else` = 0（`:156` 为分类推断，非摘要路径）
- rustfmt：本次改动文件干净（含顺手修正该文件既有的 `SummaryEngine::new` 多行格式）

### 决策理由
- 双层回退是"同一决策（摘要失败怎么办）在两处实现"的典型冗余：内层吞错使外层永不触发，warning 永不产生，失败行为不可观测且推理困难。收敛为单层后，失败路径只有一个入口（ingest() 的 match），控制流与可观测性（warning）同时成立


## Wave 3（T11）：AgentLoop 流式路径回归测试 + 共享轮次脚手架去重（P1-4）

### 改动（仅 `core/src/agent/loop.rs`）

**1. 流式回归测试（RED 先行，5 个新测试）** —— 此前 `run_stream` 零覆盖，是真实风险源：
- `test_run_stream_accumulates_multi_chunk_content`：3 chunk 文本 → 最终消息内容 = 拼接；同时断言 stream_sender 收到的 answer_delta 顺序与发送一致
- `test_run_stream_accumulates_tool_call_deltas`：tool_call 分片（id/name 首 delta + arguments 分两次）→ 两轮循环后 messages[1] 中 ToolCall 的 id/name/arguments 完整（`{"city":"北京"}`）
- `test_run_stream_mid_stream_error_returns_custom`：第 2 个 chunk 后 Err → 返回 `TianyanError::Custom`（含 agent_loop 前缀 + LLM 调用失败 + 流式接收中断）
- `test_run_stream_usage_from_final_chunk`：usage 仅出现在最后 chunk → Answer.total_tokens 三字段（100/50/150）正确
- `test_run_stream_answer_delta_order_preserved`：A/B/C/D 顺序与发送一致
- 测试基建复用既有 `MockChatService`（mockall 已 mock `chat_completion_stream`，无新 mock）；新增 helper：`stream_chunk`（构造 chunk）、`stream_mock`（重放 chunk 序列）、`collect_answer_deltas`（收集中断后收集 Answer delta）

**2. `run` / `run_stream` 轮次脚手架去重（GREEN）**
- 提取私有 `run_turns<F>`：统一两路径的轮次状态初始化（current_parent_id / total_tokens）、TurnContext 构建、`handle_llm_response` 调用与早返回、最大轮数错误。`step` 闭包负责每轮"请求构造 → 响应获取 → 统一为 (Message, Option<TokenUsage>)"，两路径各保留差异部分（with_stream 标记 / chunk 累积）
- 私有类型别名 `TurnStep<'a>` 承载 step 返回的 boxed future；HRTB 约束 `for<'a> FnMut(&'a AgentLoop, &'a str, Option<&'a StreamEventSender>, Vec<Message>) -> TurnStep<'a>` 使闭包不捕获借用（self/model/sender 均作为入参传入），避免异步闭包借用冲突
- 净删 ~60 行重复脚手架；`handle_llm_response`、`persist_message` 未动；公开 API 签名零变化

### 验证

- RED：5 个测试先全绿（现有实现行为正确，回归锁定）；随后临时变异 `accumulated_content.push_str(content)` → `accumulated_content = content.clone()`（只保留最后 chunk），`test_run_stream_accumulates_multi_chunk_content` 失败——`left: "！" / right: "你好，世界！"`，证明测试能捕获累积逻辑回归；恢复后重新全绿
- GREEN：去重后 `cargo test -p tianyan-core --lib test_run_stream` = 5 passed；全量 `cargo test -p tianyan-core --lib` = **556 passed / 0 failed / 1 ignored**（基线 547 + 本任务 5 新测试 + 并行波次新测试；ignored 为既有 session::manager 忽略项）
- clippy：loop.rs 零警告（crate 余下 5 个警告均在 observability / scheduler / session，既有）；fmt 干净

### 决策理由
- 流式路径此前零测试且累积/错误语义是前端打字机效果的契约（chunk 顺序、usage 末 chunk、中途错误前缀），必须用回归测试锁定
- `run`/`run_stream` 的重复不只在请求构造：轮次状态初始化、TurnContext 构建、早返回、最大轮数错误各出现两份，提取为 `run_turns` 后单轮脚手架只有一份；step 闭包用 HRTB 入参传递而非捕获，规避异步闭包借用冲突（若 HRTB 方案编译失败，备选方案为仅提取 `build_request` 并保留双循环）
- 行为零变化：错误消息文本、chunk 顺序、累积逻辑逐字保持；`run_stream` 中新增的 `stream_sender` 缺失守卫（`ok_or_else`）在既有调用下不可达，仅为满足 Option 入参契约

## Wave 3（T10）：提取 run_agent_turn 消除编排逻辑重复（P1-3）

### 问题
`Agent::process_message`（coordinator.rs，AgentCoordinator trait 实现）与 `Agent::handle_clarification_response`（agent_core.rs，impl Agent）两条编排路径各约 110 行、逐段重复同一编排：`persist_user_message` → `prepare_context` → `agent_loop.run` → Answer/NeedsClarification/Err 三分支组装 AgentResponse → `update_agent_metrics`，连错误处理都相同。行为漂移风险：改一条忘另一条。

### 共享/差异段对比（基于实际代码）
| 段 | process_message | handle_clarification_response | 处理 |
|---|---|---|---|
| `start = Instant::now()` | 函数入口 | 函数入口 | 共享（`start` 由调用方传入，测量窗口不变） |
| model 解析 | `model.unwrap_or(&default_model)` | 恒 `&default_model` | 差异（调用方） |
| load_and_build_state | 有 | 无（trait 侧已加载） | 差异（调用方） |
| capture_workspace_snapshot | 有 | 无 | 差异（extras） |
| pending_clarification 检查/清理 | 无 | 有 + 早返回 | 差异（extras） |
| persist_user_message | 有 | 有 | **共享** |
| 审批 confirm_pending_approval | 无 | 有 | 差异（extras） |
| prepare_context | 有 | 有 | **共享** |
| parent_id 计算点 | prepare 之后（=当前用户消息） | persist 之前（=上一消息） | 统一为 prepare 之后（见决策理由） |
| agent_loop.run | 有（model 参数） | 有（default_model） | **共享**（model 作入参） |
| 三分支 match（含 record_execution / token_usage / processing_time_ms / 错误消息） | 相同 | 相同 | **共享** |
| update_agent_metrics | 有 | 有 | **共享** |
| maybe_compress_and_persist | 有 | 无 | 差异（extras） |
| learn_skills 后台 spawn | 有 | 无 | 差异（extras） |

### 改动
- `core/src/agent/agent_core.rs`：在 `impl Agent` 新增 `pub(crate) async fn run_agent_turn(&self, state, session_id, query, model, start) -> Result<AgentResponse>`，承载共享骨架（persist → prepare → parent_id → run → 三分支 → update_agent_metrics，约 85 行）
- `coordinator.rs::process_message`：保留 extras（model 解析、快照、压缩、技能学习 spawn），编排压缩至 **40 行**（含签名）
- `agent_core.rs::handle_clarification_response`：保留 extras（pending 检查/清理、审批确认），编排压缩至 **≤40 行**（含签名）；删去自身 parent_id 预计算（由共享骨架统一）
- `coordinator.rs`：删除随提取变死的 `use crate::common::types::TokenUsage` 导入
- `process_message_stream` 未动（流式路径不在范围）

### 验证
- RED（特性锁定）：澄清路径此前零覆盖，先补 2 个集成测试（MockChatService + 真实 ContextPipeline/ToolRegistry 构造完整 Agent）：`test_process_message_returns_clarification`（ask_user → AgentResponse::clarification，问题文本一致）、`test_clarification_response_returns_answer`（回答追问 → Answer，pending 清理）。重构前运行通过（行为锁定）
- GREEN：提取后同 2 测试通过；全量 `cargo test -p tianyan-core --lib` = **558 passed / 0 failed / 1 ignored**（基线 556 + 本任务 2 新测试）
- clippy：本次改动文件零警告（crate 余下 5 个警告均在 observability / scheduler / session，既有）；fmt 干净；`cargo check -p tianyan-core` 通过

### 决策理由
- 共享骨架置于 agent_core.rs `impl Agent`（`handle_clarification_response` 所在文件），coordinator 侧经 `self` 调用，同类型跨 impl 块无新抽象
- `parent_id` 计算点两条路径原本不一致（process：prepare 之后=当前用户消息；clarify：persist 之前=上一消息，回复链跳过用户回答）。统一为 process_message 语义（回复挂接在用户回答之下）——process 路径行为零变化；澄清路径的持久化回复链被修正为 回答→回复（此前全代码库无任何消费者读取 parent_id 链，无测试断言，仅 JSONL 元数据）
- 审批确认相对 persist 的顺序互换（原 persist→confirm，现 confirm→persist）：两子系统（VFS 会话 / 审批注册表）互不可观测，无行为影响
- `start` 由调用方传入而非骨架内部新建：process_message 原测量窗口含 load_and_build_state + 快照捕获，保持 processing_time_ms 语义逐字不变

## Wave 4（T17）：删除 session/manager.rs 残留设计注释

- `core/src/session/manager.rs`：删除 load_session 尾部失效注释（"注意：元数据存储在向量库中，暂时不读取 / 如果需要，可以通过 vector_storage().get_point() 获取"，约 :135-136）——SessionManager 无 vector_storage 引用，误导维护者；纯注释清理、代码零改动，`cargo check -p tianyan-core` 通过

## Wave 4（T14）：拆分 executor/actions.rs（P1-7，纯移动）

### 改动

`core/src/executor/actions.rs`（597 行）单文件混合三组职责（SecurityPolicy 安全策略 :36-312 + 命令执行 :314-400 + 输出解析 :400-450），拆分为三个子模块，公开 API 路径零变化：

- **新建 `executor/security.rs`（280 行）**：移入 `SecurityPolicy` 结构体 + 全部 impl（`from_config` / `transform_command` / `check_path` / `check_file_size` / `check_command` / `has_shell_metacharacters` / `check_file_write`）+ `Default` impl。原 actions.rs 测试模块中无 SecurityPolicy 专属测试（3 个测试为 read/write/command），故无测试随迁；行为覆盖由 `tool_registry/executors_tests.rs`（构造 `SecurityPolicy` 字面量 + 各 check 方法）保持
- **新建 `executor/command.rs`（132 行）**：移入 `DEFAULT_COMMAND_TIMEOUT_SECS`、`read_reader_to_vec`、`extract_command_base`、`execute_command_action` + 原 `test_execute_command` 测试。`extract_command_base` / `DEFAULT_COMMAND_TIMEOUT_SECS` 因 security.rs 引用改为 `pub(super)`（原为模块私有，可见域从"本文件"扩为"executor 子树"，同 crate 内不构成公开 API）
- **`executor/output_parse.rs`（T12 已建）**：追加 `count_test_passed` / `extract_test_failures`（`pub(crate)`，与原 `extract_build_errors` 可见域一致）；模块文档更新为覆盖 build/lint/test 三类输出
- **`executor/actions.rs`（150 行）**：保留 5 个公开 `execute_*`（read/write/search_code/run_tests/verify_build）+ 2 个 `pub use` 重导出（`command::execute_command_action`、`security::SecurityPolicy`）+ read/write 2 个测试。保留 `pub use` 而非改 mod.rs 的原因：`verification.rs` 硬引用 `crate::executor::actions::execute_command_action`，actions.rs 重导出使该内部路径与 `executor/mod.rs` 的 `pub use actions::{...}` 均原样可用
- **`executor/mod.rs`**：仅追加 `mod command;` / `mod security;` 声明，re-export 块逐字未动

### 验证

- `cargo check -p tianyan-core` 通过（余下 1 个 `unused-qualifications` warning 位于 scheduler，属既有）
- `cargo test -p tianyan-core --lib` = **559 passed / 0 failed / 1 ignored**（基线 558 + 其他波次新增 1；executor 全模块含 `actions::tests` 2 个、`command::tests` 1 个、`output_parse::tests` 7 个均绿）
- `cargo fmt --all` 干净

### 决策理由

- 纯移动：除 import 路径、mod 声明与两个 `pub(super)` 可见域调整外，函数体/错误消息/测试逐字未动；`chrono::Utc` 等全限定引用原样保留
- 薄委托层选 `pub use` 而非转发函数：`execute_run_tests` / `execute_verify_build` 本就调用 `execute_command_action`，转发层会引入无意义包装；`pub use` 让 `crate::executor::actions::execute_command_action`（verification.rs 依赖）与 `crate::executor::SecurityPolicy`（builder/agent_core/loop/tool_registry 依赖）两条既有路径零改动
- 输出解析函数统一收拢进 output_parse.rs：与 T12 的 `extract_build_errors` 同属"命令输出 → 结构化结果"职责，后续测试输出解析测试可直接挂载该模块

## Wave 4（T15）：拆分 agent/tool_registry/executors.rs（P2-6，纯移动）

### 改动

`core/src/agent/tool_registry/executors.rs`（611 行，15 个 `pub(crate)` 工具函数 + 2 个模块级 helper + 39 个测试）按工具域拆分为 4 个 `pub(crate)` 模块，`ToolRegistry` 公开 API 与 mod.rs 的 dispatch match 零变化（mod.rs 仍直接调用 `self.execute_*` 方法，方法名/签名原样保留）：

- **新建 `tool_registry/file_ops.rs`（156 行）**：`execute_read_file` / `execute_write_file` / `execute_vfs_read` / `execute_vfs_list`（文件读写 + VFS 条目浏览，含 write_file 的安全检查链与审批工作流门控）
- **新建 `tool_registry/code_ops.rs`（67 行）**：`execute_search_code` / `execute_run_tests` / `execute_verify_build`（代码搜索 + 测试运行 + 构建验证，含 verification_gate 语义验证回退）
- **新建 `tool_registry/knowledge_ops.rs`（196 行）**：`execute_search_knowledge` / `execute_knowledge_ingest` / `ingest_single_file` / `ingest_directory`（VFS 语义检索 + 文件/目录知识摄入；`ingest_*` 两辅助方法随 `execute_knowledge_ingest` 同驻，保持文件/目录分流依赖内聚）
- **新建 `tool_registry/agent_ops.rs`（247 行）**：`execute_execute_command` / `execute_call_skill` / `execute_ask_user` / `execute_self_check` / `execute_delegate_to_agent`（命令执行 + 技能调用 + 追问 + 指标自检 + 子 Agent 委托，含 execute_command 的安全策略重写与审批门控；delegate_to_agent 的 `BoxFuture` 异步递归打破原样保留）
- **`tool_registry/mod.rs`**：`mod executors;` 替换为 4 个 `mod` 声明；`parse_params` / `safety_violation` 两个模块级 helper 上移为 mod.rs 私有函数（私有项对子模块可见，各新模块经 `use super::{parse_params, safety_violation}` 引用，可见域不变）；既有 `vfs_content_field` 同样被 file_ops / knowledge_ops 经 `super::` 复用
- **删除** `executors.rs` / `executors_tests.rs`（mod.rs 直接调用方法，无薄委托层）
- **测试随迁**：`executors_tests.rs` 的 39 个测试按工具域切分为 4 个 `#[path]` 挂载测试文件（`file_ops_tests.rs` 15 / `code_ops_tests.rs` 5 / `knowledge_ops_tests.rs` 5 / `agent_ops_tests.rs` 14），每个测试文件不自带 `mod tests` 包装；`default_strict_policy` / `file_policy` / `read_file_args` / `write_file_args` / `EchoSkillHandler` / `chat_response` 等测试 helper 随所属测试文件复制

### 验证

- `cargo check -p tianyan-core` 通过（2 个既有 warning 位于 executor / scheduler，属其他波次任务改动）
- `cargo test -p tianyan-core --lib` = **559 passed / 0 failed / 1 ignored**；`cargo test --lib tool_registry` = 44 passed（mod.rs 5 + file_ops 15 + code_ops 5 + knowledge_ops 5 + agent_ops 14），拆分前后计数一致
- 4 个新生产文件 + 4 个测试文件 `rustfmt --check` 零差异

### 决策理由

- 纯移动：函数体、错误消息文本、安全检查/审批调用顺序逐字未动；仅 import 路径与 mod 声明变化。`execute_delegate_to_agent` 保留内联 `serde_json::from_str`（不换用 `parse_params`，避免借机改动）
- helper 上移 mod.rs 而非保留薄 `executors.rs`：mod.rs 通过方法直调新模块，无调用方引用 executors 路径，薄文件会沦为"仅两个 helper"的空壳；mod.rs 私有函数对子模块可见（Rust 可见性规则：私有项对后代模块可见），各模块 `use super::` 引用即可，无需改 `pub` 可见域
- 分组以任务清单为基准、按实际依赖微调：`ingest_single_file` / `ingest_directory` 非工具入口但被 `execute_knowledge_ingest` 直接调用，必须同驻 knowledge_ops；`execute_command` 的审批门控与 `execute_write_file` 同型但归入 agent_ops（命令/委托/追问同属"与外部交互"域），两处样板互不引用，无拆散依赖

## Wave 4（T16）：TaskResult 错误从 String 改为 TianyanError（P1-9）

### 问题
`scheduler/task_scheduler.rs` 的 `TaskResult { error: Option<String> }` 用 String 传递任务错误，违反 AGENTS.md「所有错误用 TianyanError、消息带模块前缀」；错误不可结构化处理（无法调用 `is_not_found()` 等谓词、无法携带结构化信息）。

### 改动
- `core/src/scheduler/task_scheduler.rs`：
  - `TaskResult.error` 字段类型 `Option<String>` → `Option<TianyanError>`；`TaskResult::failed(error: impl Into<String>)` → `failed(error: impl Into<TianyanError>)`
  - `TaskResult` derive 从 `Debug, Clone` 收窄为 `Debug`——`TianyanError` 含 `Io(io::Error)` 非 Clone 变体，无法派生 Clone；全代码库无任何 TaskResult 克隆点（`execute_task` 按值返回、调度循环丢弃结果），去掉 Clone 零影响
  - `register_task` 的重复 ID 错误改为使用已导入的 `TianyanError::Custom`（消除全限定路径冗余）
  - `execute_task` 失败日志（`任务执行失败：{} - {}`）不变：`TianyanError` derive `Error`（thiserror），`{}` Display 继续可用
- 4 个任务调用点改为 `TaskResult::failed(TianyanError::Custom(format!("模块前缀: 详情")))`，错误消息文本保持（仅补模块前缀）：
  - `tasks/summary_task.rs`：「摘要生成任务：扫描失败：{}」
  - `tasks/memory_task.rs`：「记忆提取任务：扫描会话失败：{}」
  - `tasks/rule_task.rs`：「规则提炼任务：扫描失败：{}」
  - `tasks/gc_task.rs`：「GC 任务：扫描失败：{}」（gc_task 已有 `TianyanError` 导入，其余用全限定路径）
- 未改：`TaskContext` 结构、调度循环逻辑（interval/register/start）、无新错误类型（沿用 `TianyanError::Custom`）

### 契约保持
- **`TaskStatus` serde JSON 契约零变化**：`TaskStatus`（task_scheduler.rs:155）本就**不含 error 字段**（仅 id/name/priority/cron_expression/run_count/last_run_ago_secs），`scheduler.snapshot() → Vec<TaskStatus>` 的 `/api/v1/scheduler/status` JSON 字节不变；server `insights/handlers.rs::get_scheduler_status_handler` 无需改动
- 新增测试 `test_task_status_json_contract`：直接序列化 TaskStatus 并与 `json!` 字面量全等断言（含 `"priority": "Normal"`），并断言输出无 error 键——锁定 API 契约
- e2e（`server/tests/e2e_tests.rs:121-128`）仅断言 `tasks` 数组长度，与字段无关

### 验证
- `cargo test -p tianyan-core --lib` = **559 passed / 0 failed / 1 ignored**（基线 558 + 本任务 1 新测试 `test_task_status_json_contract`；`test_task_result` 原地更新断言 TianyanError 字符串化）
- `cargo check --workspace` 通过（含 tianyan-server/tianyan-tauri/tianyan-mcp）
- fmt：scheduler 5 文件干净；clippy：改动文件零新警告（crate 余下 5 个警告均在 observability / gc_task:212,214（既有）/ session，与基线一致）

### 决策理由
- 错误结构化是「错误统一 TianyanError」约束的一部分：后续任务失败可用 `is_not_found()` 等谓词区分错误类别
- `impl Into<TianyanError>` 保留宽松构造点：TianyanError 无 `From<String>`，调用点显式构造 `Custom(String)` 强制带模块前缀（AGENTS.md 要求），避免裸字符串再次渗入
- 去掉 TaskResult 的 Clone 而非给 TianyanError 补 Clone：io::Error 非 Clone，补 Clone 需包装 Io 变体（改动共享错误类型、超出任务范围）；且 TaskResult 无克隆消费点
- 错误消息文本逐字保持（原「GC 扫描失败：{}」→「GC 任务：扫描失败：{}」等），仅补模块前缀符合「错误消息文本尽量保持」

## Wave 5（T19）：会话截断常量单点定义（P1-6）

### 问题
会话历史截断常量散落两处、口径不同：
- `agent/session_state.rs`（内存态 `trim_conversation`）：`MAX_CONVERSATION_MESSAGES = 100`（裁剪触发阈值）+ `KEEP_RECENT_MESSAGES = 50`（保留条数）
- `session/manager.rs`（持久态 `load_session_from_vfs` 加载截断）：`MAX_SESSION_MESSAGES = 100`（安全上限）

同一领域概念（会话历史生命周期）横跨两模块，各管一段，无统一契约，调参需改两处。

### 改动
- `core/src/session/mod.rs`：定义统一常量（模块公开面）：
  - `pub const MAX_SESSION_MESSAGES: usize = 100;` —— 持久态安全上限（`manager.rs` 加载截断）+ 内存态裁剪触发阈值（`session_state.rs` trim 触发，原 `MAX_CONVERSATION_MESSAGES` 100 并入此常量）
  - `pub const KEEP_RECENT_MESSAGES: usize = 50;` —— 内存态裁剪后保留的最近消息条数
- `core/src/agent/session_state.rs`：删除本地 `MAX_CONVERSATION_MESSAGES` / `KEEP_RECENT_MESSAGES` 定义，改为 `use crate::session::{KEEP_RECENT_MESSAGES, MAX_SESSION_MESSAGES}`；`trim_conversation` 内 `MAX_CONVERSATION_MESSAGES` 引用替换为 `MAX_SESSION_MESSAGES`（值不变 100）
- `core/src/session/manager.rs`：删除本地 `MAX_SESSION_MESSAGES` 定义，改为 `use super::{types::Session, MAX_SESSION_MESSAGES}` 引用统一常量
- 截断逻辑（`trim_conversation` / `load_session_from_vfs` 裁剪段）零改动，仅常量来源变化

### 契约保持
- **常量数值零变化**：100（触发/加载上限）/ 50（保留条数）/ 100（加载上限）
- 行为差异（`session_state.rs` 裁剪触发 100 vs 保留 50；`manager.rs` 加载上限 100）**未合并**——本任务只做单点化，不做口径合并

### 验证
- `cargo test -p tianyan-core --lib` = **559 passed / 0 failed / 1 ignored**（基线 559，零新增测试；`session_state.rs::test_session_state_cleanup` 断言 `<= KEEP_RECENT_MESSAGES` 经 `use super::*` 解析到统一常量）
- grep 验收：`MAX_SESSION_MESSAGES` 定义 1 处（`session/mod.rs`），`KEEP_RECENT_MESSAGES` 定义 1 处（`session/mod.rs`），`MAX_CONVERSATION_MESSAGES` 已不存在（生产代码零定义）

### 决策理由
- 常量定义在 `session/mod.rs`（领域模块公开面）而非 manager.rs：会话历史生命周期属于 session 领域，`agent/session_state.rs` 引用 `crate::session::*` 依赖方向合理（agent → session），避免 agent 常量被 session 反向依赖
- 100-vs-50 的触发/保留差是既有产品行为（压缩标记机制依赖保留窗口），数值一律不动，待产品决策
- 后续如需调参，改 `session/mod.rs` 一处即可全局生效

## Wave 5（T21）：删除 gc_task 投机代码（文档漂移检测 + 质量报告回写，P2-5）

### 问题
`core/src/scheduler/tasks/gc_task.rs` 含三块投机代码，与 GC 核心职责（规则归档 / 记忆 TTL 清理）无关：
- `DocDriftReport` struct + `scan_documentation_drift`：硬编码 `docs` / `core/src` 相对路径扫描文档模块引用（依赖进程工作目录，VFS 之外直接 `std::fs` 访问项目源码目录），结果只用于 warn 日志，无任何实际消费者
- `write_quality_report`：向 VFS `knowledge/quality/domain-grades.md` 写入全 "-" 占位的模板 Markdown（无真实数据），仅空转消耗 VFS 写入
- 全仓 grep 确认零外部消费者（仅 gc_task.rs 内部调用自身），server / gui / e2e 均不引用

### 改动
- 删除 `DocDriftReport` struct（6 行）与第二个 `impl GcTask` 块（`scan_documentation_drift` 65 行 + `write_quality_report` 47 行 + 文档注释，120 行）
- `execute()`：删除 drift 调用（含"不阻塞 GC 主流程"注释）、质量报告回写调用、`drift_msg` 拼接分支；`GC 扫描完成{}` 日志去掉 `{}` 占位与 `drift_msg` 参数；`stale_rules` / `cleaned_memory` 统计字段与 `TaskResult::success(rules + mem)` 保留
- 移除随删变死的 `ContentLevel` 导入（仅 `write_quality_report` 使用）
- 共删除 143 行（496 → 353）；GC 核心 `scan_learned_rules` / `scan_memory` 与 `TaskHandler` 注册（`garbage_collection`）逐字未动

### 测试
- gc_task 测试模块本就只覆盖 `scan_learned_rules` / `scan_memory`（5 个测试），无 drift/report 测试；测试基建（`make_context` / MockVfs）与被删函数无关，零调整
- `cargo test -p tianyan-core --lib` = **559 passed / 0 failed / 1 ignored**（基线 559 持平）；`cargo check --workspace` 通过（server 的 scheduler status 仅消费 `run_count` 等 TaskStatus 字段，不受影响）
- 验收：`grep "DocDriftReport|scan_documentation_drift|write_quality_report" core/src/` = 空

### 决策理由
- 零消费者 + cwd 依赖（进程启动目录不在 `docs` 时静默空转或 warn）+ 全占位报告 = 纯投机代码，删除零行为影响
- 质量报告是"GC 任务自动生成"的既有空壳，无数据来源（规则/记忆计数在内存中已有，但报告从未接线），保留只会误导维护者以为有质量评级功能

## Wave 5（T20）：配置加载失败从静默降级改为可观测（P2-2）

### 问题
`config/mod.rs` 的 `get_config()` 加载失败仅 `tracing::warn!` 后返回 `TianyanConfig::default()`——生产环境配置损坏时**静默用默认配置运行**，无任何显式错误暴露。`get_config()` 有 3 个真实外部调用点（core `lib.rs:init` + config 内部 + server `lib.rs`），签名不可改。

### 改动（最小化方案：保持签名，暴露错误）
- `core/src/config/mod.rs`：新增 `static CONFIG_LOAD_ERROR: OnceLock<TianyanError>`（进程级单例，与 `CONFIG` 同生命周期）
- `get_config()` 失败路径（`load_config_or_default` 返回错误时）：`tracing::warn!` → **`tracing::error!`**（日志文本含路径与错误详情），错误写入 `CONFIG_LOAD_ERROR`，仍返回默认配置（行为零变化）
- 新增公开方法 `pub fn last_config_error() -> Option<&'static TianyanError>`：调用方可查询首次加载错误；**本任务不改 server**，仅提供查询入口（server 若接入 `config_load_error` 语义，建议挂到既有 `/api/v1/config/status`）
- 提取两个私有函数便于测试（`CONFIG` 为进程级单例、OnceLock 不可重置）：
  - `load_config_or_default()`：加载配置；失败时构造带路径上下文的错误并回退默认配置
  - `config_load_error(path, detail)`：构造错误消息（模块前缀 + 配置路径 + 详情；文件缺失时路径回退到预期配置路径 `default_config_path`）
- 修正 `get_config` 过时文档：原 `# Panics`（"配置加载失败时 panic"）与实现不符（从不 panic），改为记录 `error` 日志 + 回退默认 + `last_config_error` 可查询

### 测试
新增 3 个测试（`config::tests`）：
- `test_config_load_error_contains_path_and_detail`：纯函数验证错误消息格式（模块前缀 + 路径 + 详情；路径未知时"默认搜索路径"占位）
- `test_load_invalid_toml_fails`：temp 目录写非法 TOML → `load_from_file` 返回 Err（损坏配置触发源）
- `test_get_config_once_semantics_and_error_query`：lib 测试二进制中唯一调用 `get_config()` 的测试（grep 确认），验证 OnceLock 单次初始化语义（`ptr::eq` 同一实例）+ 首次加载失败时 `last_config_error()` 为 `Some` 且含"config 加载失败"前缀与"路径"上下文

**测试限制**：`CONFIG` / `CONFIG_LOAD_ERROR` 为进程级 OnceLock，无 `take()` 无法重置，且测试并行执行——因此"损坏配置 → `get_config()` 返回默认 + `last_config_error()` 为 Some"的完整链路无法在单进程内确定性重放（首个调用前无法注入环境）。采用等价拆解：错误构造纯函数 + `load_from_file` 损坏文件失败 + 首次调用语义（本机无任何配置文件，实际运行中 Some 分支被真实触发并通过断言）。

### 验证
- `cargo test -p tianyan-core --lib` = **577 passed / 0 failed / 1 ignored**（基线 559；新增 3 个 config 测试，其余 15 个为并行任务 T21 等新增）；`config::` 过滤 44 个全绿
- `cargo check --workspace` 通过；`cargo fmt --all -- --check` 干净
- clippy：`cargo clippy -p tianyan-core -- -D warnings` 当前 6 个错误全部位于并行任务在途文件（`executor/security.rs` x2 / `observability/mod.rs` x2 / `session/manager.rs` / `skills/executor.rs`），**零个位于 `config/mod.rs`**——本文件 clippy 干净

### 决策理由（为何最小化而非重构）
- 调用点 >3 个（core lib.rs + config 内部 + server lib.rs），改签名或返回类型会破坏调用方；保持 `get_config() -> &'static TianyanConfig` 签名与返回行为零变化是硬约束
- 不引入 `Result<&'static TianyanConfig>` 双 API：`last_config_error()` 查询入口已覆盖"可观测"诉求（错误日志 + 可查询），server 诊断面（`/api/v1/config/status`）后续消费即可，无需新依赖或错误类型
- 错误用既有 `TianyanError::Custom`，遵循"模块前缀：详情"约定（`config 加载失败（路径：…）：…`），不新增变体

## Wave 5（T18）：统一两套路径沙箱实现（P1-5，安全敏感）

### 问题

两套独立的路径校验实现，语义不同，同一条路径在不同入口（技能 vs 工具）可能得到不同判定：

| 维度 | `skills/executor.rs::validate_path` | `SecurityPolicy::check_path` |
|------|-------------------------------------|------------------------------|
| 列表语义 | allowlist：空列表 = 放行全部 | blacklist 优先 + allowlist（非空时） |
| 路径规范化 | `canonicalize` → 失败回退 `cwd.join(path)` → **剥离 `\\?\` verbatim 前缀** | `canonicalize` → 失败回退原值（相对路径不解析） |
| 目录规范化 | 逐目录 `canonicalize` + 剥离前缀 | 直接用配置原始值 `starts_with`（依赖调用方预规范化，Windows 上 raw 配置目录因 verbatim 前缀失配静默失效） |
| 错误前缀 | `操作不被允许：...` | `executor: 安全策略违规：...` |

### 改动

- **`core/src/executor/security.rs`（共享 helper 落地处）**：
  - 新增 `pub(crate) fn normalize_path_for_check(p: &Path) -> PathBuf`——自 `skills/executor.rs` 原样移入（`\\?\UNC\` → `\\`、`\\?\` 剥离），成为两套沙箱唯一的规范化实现
  - 新增 `pub(crate) enum PathCheckOutcome { Allowed, Blocked(PathBuf), NotAllowed }`——`Blocked` 携带命中的黑名单目录，供调用方格式化各自的错误消息
  - 新增 `pub(crate) fn check_path_rules(path, allowed: &[PathBuf], blocked: &[PathBuf]) -> PathCheckOutcome`——统一判定：**黑名单绝对优先**（命中任一黑名单目录 → `Blocked`，与 `check_path` 既有顺序一致），白名单非空且不在其中 → `NotAllowed`，其余（含白名单为空）→ `Allowed`；路径与目录均规范化后做组件级 `starts_with` 前缀匹配（天然拒绝前缀兄弟目录）
  - 私有 `resolve_canonical` / `normalize_dir_for_check`：canonicalize 失败时路径回退 `cwd.join(path)`（与 `validate_path` 既有行为一致，保留相对路径语义）、目录回退原值
  - `SecurityPolicy::check_path` 改调 `check_path_rules`，**签名、blocked 绝对优先语义、两条错误消息逐字不变**（`Blocked` 携带的目录即配置原始值，`(禁止：{})` 展示不变）；Permissive 提前放行保留在方法内
- **`core/src/skills/executor.rs`**：删除本地 `normalize_for_compare` 与手写判定循环；`validate_path`（`pub(crate)` 签名不变）改调 `check_path_rules(path, allowed_paths, &[])`，错误消息逐字不变——4 个 handlers 调用点（file_read / file_write / file_list / file_delete）零改动；skills 测试模块补 `use crate::executor::security::normalize_path_for_check` 引用被移函数
- **`core/src/executor/mod.rs`**：`mod security` → `pub(crate) mod security`（技能模块跨模块访问共享 helper 所需；`pub(crate)` 对外部 crate 不可见，公开 API 零变化）

### 优先级决策

以现有两套实现的行为为准：`check_path` 先查黑名单 → **黑名单绝对优先**；技能侧无黑名单概念，`validate_path` 固定传 `blocked = &[]`，保留"空白名单 = 放行"的 allowlist 语义，同时获得可选黑名单能力。

### 取舍说明（合并代价）

1. **错误消息无法合并**：两入口消息前缀不同（`操作不被允许` vs `executor: 安全策略违规`），且 approval 指纹/前端展示依赖后者逐字保持 → helper 返回结构化 `PathCheckOutcome` 而非 `Result<TianyanError>`，消息由各调用方格式化，**两套消息逐字保持**。grep 验收：黑名单/白名单错误消息文本各只出现 1 处（`executor/security.rs`），判定逻辑 1 处（`check_path_rules`）
2. **`validate_path` 的"无法解析路径"分支（canonicalize 与 current_dir 双双失败）随委托消失**：该分支依赖 `current_dir()` 失败（cwd 被删除等不可达场景），无测试覆盖；委托后此场景回退原值路径 → 走"不在允许的操作范围内"。可达路径上的所有错误消息不变
3. **`check_path` 对 Windows raw 配置目录的行为修复（安全正向）**：旧实现 `canonical.starts_with(blocked原始值)` 在 Windows 上因 `\\?\` verbatim 前缀组件失配，未预规范化的配置目录（用户原始配置/相对路径）静默不生效；统一后两侧均剥离前缀 + 目录规范化，黑名单/白名单按配置意图生效。方向全部为"更严格地执行既有配置"（黑名单更有效、白名单按 resolved 路径匹配），非 Windows 平台 normalize 为恒等变换零影响；file_ops_tests 的 `file_policy` 预规范化目录的既有测试全部保持绿

### 验证

- RED：新增 15 个行为锁定测试先行编译失败（`check_path_rules` / `PathCheckOutcome` / `normalize_path_for_check` 不存在）
- GREEN：`cargo test -p tianyan-core --lib` = **577 passed / 0 failed / 1 ignored**（基线 559 + 本任务 15 新测试；剩余 3 为并行 Wave 5 任务测试；skills `validate_path` 5 测试 + `normalize` 1 测试全绿，security.rs 新 15 测试全绿）
- 新测试覆盖：allowlist（内放行/外拒绝/前缀兄弟拒绝/空列表放行/绝对路径匹配）+ blacklist（拒绝/嵌套子路径拒绝/**黑名单优先于白名单**/未命中黑名单时白名单放行）+ `SecurityPolicy::check_path` 行为锁定（黑名单消息含 `(禁止：{})`、白名单消息、空列表放行、黑名单优先级、Permissive 跳过）+ verbatim 前缀剥离
- `cargo check --workspace` 通过；改动 3 文件 `rustfmt --check` 零差异；clippy 仅剩基线既有警告（`validate_path` 的 `ptr_arg` 为任务锁定签名，属既有警告随行号迁移）
## Wave 7（Loop 2）：clippy 零警告达成（19 → 0）

### 改动
- `core/src/observability/mod.rs`：`total / count` 手动除零判断 → `checked_div().unwrap_or(0)`（manual_checked_div）；`sort_by` → `sort_by_key(Reverse)`（sort_by_key）
- `core/src/session/manager.rs`：`sort_by` → `sort_by_key(Reverse)`（sort_by_key）
- `core/src/agent/tool_registry/mod.rs`：`TestConflictingTool` 从 `mod tests` 之后移入测试模块内（items_after_test_module）
- `tauri/src/lib.rs`：顶部新增 `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`（与 core/lib.rs 模式一致，覆盖整个 crate 含 server.rs 测试）；`mod tests` 移到文件末尾（items_after_test_module）；`tokio::time::sleep` → `sleep`（unnecessary_qualification）

### 验证
- `cargo clippy --workspace --all-targets`：0 warnings（重构起点 22 → Loop 1 后 19 → 本轮 0）
- `cargo test -p tianyan-core --lib`：577 passed；`cargo test -p tianyan-tauri --lib`：7 passed；`cargo test -p tianyan-server --lib`：36 passed
- 纯 lint 清理，零行为变化（豁免属性仅作用于 cfg(test)）

### 决策理由
- 项目 REFACTOR_LOG 迭代 6 曾声明"clippy 零警告"标准，19 个遗留（observability/session/tool_registry/tauri）为最后卫生缺口；expect 豁免采用与 core 相同的 crate 级 cfg_attr(test) 模式，避免逐测试加 allow 的噪声
## Wave 6：文档同步（T22/T23 后补日志）

### 改动（提交 deeec6f9 + 919e2474，此前未记日志）
- `docs/architecture/module-map.md`：模块索引同步重构后路径（sqlite_db/retrieval_trace/token_estimator/security.rs/file_ops.rs/approval/ 目录化）
- `docs/module-relationships.md`：依赖矩阵更新（3 环消除后的新边）、§6 偏差表按代码核验、§7 新增"决策 5：依赖环消除（ADR-007）"
- `docs/architecture/principles.md`：新增原则"共享基础设施归属被依赖方"
- 新建 `docs/architecture/decisions/007-core-dependency-cycle-removal.md`（ADR-007：下沉决策 + SKIP 决策 + 行为变化记录）
- `docs/module-descriptions.md`：模块描述同步；`001/005` ADR 与 system-architecture.md 路径修正
- `docs/architecture/core-module-architecture.html`：顶部标注"重构前快照，以 ADR-007 为准"

### 验证
- 文档断言与代码路径 grep 抽查一致（T22 报告列 12 项核对表）；无代码改动

## Wave 7（Loop 2）补记：Oracle 终审 fmt 回归修复

### 背景
Oracle 批判性终审（Loop 2）发现 Wave 7 的 clippy 修复引入 3 处 fmt 回归：`tool_registry/mod.rs` TestConflictingTool::execute 签名移入测试模块后缩进加深（98→102 字符超 100 上限）未拆行；`tauri/src/lib.rs` mod tests 移文件尾遗留 2 处多余空行。判定 NOT-COMPLETE（按项目自身验收定义，fmt 干净属验收项）。

### 修复
- 运行 `cargo fmt --all`：3 处格式自动修复（签名拆行 + 空行清理）
- 复验：`cargo fmt --all -- --check` exit 0；`cargo test -p tianyan-core --lib` 577 passed；clippy 0 warnings

## Wave 8（性能优化）：全局禁用 doctest

### 背景与测量（先测量后优化）
用户报告 `cargo test` 性能差。实测分解（Windows, warm）：
- **测试运行不是瓶颈**：全部测试（577 单测 + 集成 + e2e）运行 <3s
- 普通测试目标编译（`--lib --tests`）：72.1s（冷）
- **doctest 冷编译 59.7s**（仅 core；全 workspace 共 14 个示例：core 9、server 3、mcp 2、tauri 0，其中 9 有效 + 5 ignored），运行仅 ~8s
- 真实开发循环（改 1 个源文件后）：core lib 15.9s；core doctest 10.7s；workspace `--lib --tests` 38.5s
- 全量 `cargo test --workspace`（warm 混合冷目标）：378.8s

### nextest 调研结论（不引入）
- 收益集中在**运行**阶段（本项目 <3s → 收益 ≤2s），不解决编译瓶颈
- 多 binary 并行对 5-crate workspace 有结构性收益（官方基准 1.4×–3.4×），但本项目运行时间占比过小
- 零迁移成本（构建产物与 cargo test 共享），作为未来 CI 选项保留

### 改动
- 4 个 crate 的 `Cargo.toml` `[lib]` 段加 `doctest = false`（core/server/tauri/mcp，各附一行理由注释）
- `scripts/test.ps1` 无需改动（本就无 `--doc` 调用）
- 零 Rust 源码改动

### 行为变化（记录）
- 常规 `cargo test` / `cargo test --workspace` 不再构建与运行 doctest（14 个示例不再执行）
- `cargo test --doc` 显式请求仍可运行 doctest（Cargo 语义：`doctest` 字段控制"默认"测试，显式目标选择覆盖——已实测验证）
- `cargo doc` 文档生成不受影响（示例仍渲染为代码块）

### 验证
- 常规 `cargo test -p tianyan-core`：577 passed / 0 failed / 1 ignored，输出无 `Doc-tests` 段
- `cargo test --workspace`：改 1 文件场景 41.1s（此前 ~55-60s 含 doctest 重编）；clean core 后全量 133.3s
- clippy 0 warnings；`cargo check --workspace` 通过；git diff 仅 4 个 Cargo.toml +8 行

### 决策理由
- 14 个示例编译成本 ~60s（冷）/ 每次改动 ~11s（热），运行仅 ~8s——编译:运行 > 7:1，性价比极低
- 示例均为模块级文档代码块（coordinator.rs / config / context），无 API 契约测试价值
- 保留 `--doc` 显式能力，未来引入重要 API 示例时可选择性恢复

## D 轮波次（0.3.16）：文档固化 + 结构重构（2026-09-12）

### 背景
三路深度审查（架构一致性 / 代码债务 / 文档体系）+ 机械扫描清单落在
`.scratch/audit-report-2026-09-12.md`（D1 文档固化 6 份、D2 结构技术债 12 项）。
批次 A（P1 正确性，`4505668`）、B（门禁恢复，`8ecb226`）、C（仓库卫生 + clippy 清零，`afba701`）
已完成，本轮清 D1 + D2。

### D1 文档固化（新增 8 份，全部经代码核实）
| 文档 | 覆盖 | 规模 |
|---|---|---|
| `docs/architecture/context-pipeline.md` | 组装顺序（含 `project_instructions`）+ 压缩（阈值 0.6/临界 0.8/保留 10/最小 6、**轮末单次检查**、`prompt_side_tokens` 单点、user 锚定）+ 量化 SQL + 故障模式 | 399 行 |
| `docs/architecture/event-protocol.md` | 通道模型 + 4 类事件字段表 + `StreamChunkType` 7 成员 + 订阅/快照恢复 + 可靠性分层 + 与 ADR 的 10 处不一致核实 | 347 行 |
| `docs/architecture/model-provider-notes.md` | Provider 矩阵 + reasoning 回传契约 + 思考强度×语言实测 + 前缀缓存 + 上游异常 | — |
| `docs/architecture/task-runtime.md` | 三类任务 + 并发排队 + 3 天 TTL + 唤醒 + 取消/回退 + FTS 边界 + 面板数据流 | 190 行 |
| `docs/operations/troubleshooting.md` | 日志位置 + 三类高频故障 + DB 取证 + QA 纪律 + PowerShell 陷阱 | 378 行 |
| `docs/operations/data-health-check.md` | 10 表 + 18 条巡检 SQL（EXPLAIN 全通过）+ 解读处置 | 283 行 |
| `docs/operations/release-msi.md` | 版本落点 + 打包流程 + CI 链 + 已知坑 + 验收清单 | 172 行 |
| 索引 | `module-map.md`（专题导航 + ADR 016/017/019 补齐）、`AGENTS.md`（导航 + PowerShell/cargo 陷阱 + 有界事件总线）、`development.md`（知识固化判据 + 测试口径） | — |

### D2 结构重构（逐项）
| # | 项 | 处置 | 验证 |
|---|---|---|---|
| B1 | 超长函数 4 个 | `spawn_background` 308→65、`run_stream` 226→46、`run_turns` 222→105、委托入口 235→95（各提取私有 helper） | 1182 测试全绿；字符串字面量集合比对（旧版全部保留） |
| B3 | 截断重复 | 删 `truncate_trace_params`，统一 `common::truncate` 单点 | 编译 + 测试 |
| B5 | 事件通道无界 + Lagged 静默 | `EventBus` 有界 4096 + `try_send` 丢弃计数 + 首次/每 100 次告警；`Lagged` 显式告警（含 skipped） | 新回归测试 + 反向验证 |
| B6 | 统计刷盘逐行 INSERT | 语句 prepare 一次复用（逐行是语义要求：每次调用一行） | 编译 + 测试 |
| B7 | VFS 前缀查询不一致 | `list_directory` `LIKE` → `substr + length`；`delete_entry` 长度改 SQL 侧 `length()` | 2 新回归测试 + 反向验证 |
| B8 | `total_stat` 丢弃传入 SQL | WHERE 子句单点构造（分组/总计共用） | 新回归测试 + 反向验证 |
| B4a | rusqlite 类型外泄 | `SqliteDb::open/open_in_memory/init_all_schemas` → `TianyanError`；门面去重复前缀 | 编译 + 测试 |
| B11 | 残留 | `agent/role_store.rs` 兼容层删除；`/tasks/stream` 保留为兼容入口（见下） | 编译 + 测试 |
| ADR 对齐 | `command_output` 无节流（ADR-028 声明 100ms 合并） | `drain_output` 时间窗合并 + 收尾 flush（含提前 break 路径） | 新回归测试 + 反向验证 |
| B10/B12 | clippy 残留 / 仓库卫生 | 批次 C 已清零，本轮复核 `-D warnings` 通过 | clippy |

### 决策记录（明确不修 + 理由）
| 项 | 决定 | 理由 |
|---|---|---|
| B4b `Database::lock()` 裸连接（54 处） | 不修 | 站点集中在 `db/*` 仓储、`vfs/backend/sqlite.rs`、`session/store.rs`、`agent/background.rs`——都是"该直接执行 SQL 的存储实现层"；再包一层违背「已有链路不叠加抽象」。真正可修的抽象泄漏（类型出现在公开签名）已修（B4a） |
| B9 启动期重复 clone 配置 | 不修 | 启动期一次性、非热路径（毫秒级），改动需引入局部变量/借用重构，收益≈0 风险非零 |
| B6 `skill_calls` 无 TTL | 不修 | 该表是演化/统计的长期输入，删历史行会改变指标口径；如要限制应立独立 ADR（数据保留策略） |
| `/tasks/stream` 端点 | 保留 | 它已是 `GET /events` 的兼容入口（README 已注明"兼容入口；推荐用 /events"）；删除收益小、外部脚本风险大 |
| `agent_ops_tests.rs` helper"未使用" | 无需动作 | clippy `-D warnings` 全绿说明该项已过时 |

### 验证（门禁全绿）
- `cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets -- -D warnings` 0 报错
- core **1182** / server 146 / mcp 16 / tauri 9；前端 vitest **392** / eslint 0 problems / prettier 全符合
- `scripts/gen-tool-catalog.ps1 -Check` → 工具目录与代码一致
- 反向验证（5 个新测试均有判别力）：VFS 前缀 ×2、usage WHERE 口径 ×1、事件总线背压 ×1、命令输出节流 ×1
- 文档质量：`data-health-check.md` 的 18 条 SQL 全部 EXPLAIN 通过（修正 1 条 DELETE 别名语法）

### 交付
`Tianyan_0.3.16_x64_zh-CN.msi` / `Tianyan_0.3.16_x64_en-US.msi`
