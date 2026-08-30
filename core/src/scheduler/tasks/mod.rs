//! 后台任务模块。
//!
//! 本模块提供所有后台任务的实现。
//!
//! # 任务列表
//!
//! - [`SummaryTask`][]: 摘要生成任务，扫描 VFS 并生成摘要。
//! - [`EvolutionTask`][]: 自演化任务（ADR-017；记忆/技能/规则/组织形态统一演化，
//!   替代已移除的 memory_extraction / rule_extraction）。
//! - [`GcTask`][]: Garbage Collection 任务，扫描过期规则。
//! - [`SnapshotGcTask`][]: 快照垃圾回收任务，清理孤儿快照对象。
//! - [`UsageStatsFlushTask`][]: 使用统计刷盘任务，内存计数器批量落库。

mod evolution_task;
mod gc_task;
mod reminder_task;
mod snapshot_gc_task;
mod summary_task;
mod usage_stats_flush_task;

pub use evolution_task::{EvolutionReviewExecutor, EvolutionTask};
pub use gc_task::GcTask;
pub use reminder_task::ReminderTask;
pub use snapshot_gc_task::SnapshotGcTask;
pub use summary_task::SummaryTask;
pub use usage_stats_flush_task::UsageStatsFlushTask;
