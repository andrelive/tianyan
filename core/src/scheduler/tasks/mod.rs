//! 后台任务模块。
//!
//! 本模块提供后台任务的**实现**。
//!
//! # 任务列表
//!
//! - [`EvolutionTask`][]: 自演化任务（ADR-017；记忆/技能/规则/组织形态统一演化，
//!   替代已移除的 memory_extraction / rule_extraction）。
//!
//! # 其余内置任务已出 core（ADR-049 期 2 序 5a）
//!
//! `SummaryTask` / `GcTask` / `SnapshotGcTask` / `ReminderTask` /
//! `UsageStatsFlushTask` 已整体迁入 `plugins/task-builtin`（crate
//! `tianyan-task-builtin`），连同各自的内联测试。core 因此不再编译这 5 份实现
//! （约 1.7K 行源码 + 1.3K 行测试）：改任务策略 / 新增内置任务只重编插件 + 装配包。
//!
//! **契约与机制仍在本模块的上一层**（`crate::scheduler`）：`TaskHandler`（执行契约）、
//! `TaskContext`（运行时世界）、`TaskScheduler`（间隔扫描 + 补跑）、
//! `TaskRegistration` / `TaskServices`（装配原语）。任务插件只依赖这些，不依赖 core
//! 的任何具体任务类型。

mod evolution_task;

pub use evolution_task::{EvolutionReviewExecutor, EvolutionTask};
