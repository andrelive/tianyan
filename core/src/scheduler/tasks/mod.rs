//! 后台任务模块。
//!
//! 本模块提供所有后台任务的实现。
//!
//! # 任务列表
//!
//! - [`SummaryTask`][]: 摘要生成任务，扫描 VFS 并生成摘要。
//! - [`MemoryTask`][]: 记忆提取任务，扫描会话并提取记忆。
//! - [`GcTask`][]: Garbage Collection 任务，扫描过期规则。
//! - [`SnapshotGcTask`][]: 快照垃圾回收任务，清理孤儿快照对象。

mod gc_task;
mod memory_task;
pub mod rule_recorder;
mod rule_suggester;
mod rule_task;
mod snapshot_gc_task;
mod summary_task;

pub use gc_task::GcTask;
pub use memory_task::MemoryTask;
pub use rule_recorder::RuleRecorder;
pub use rule_task::RuleTask;
pub use snapshot_gc_task::SnapshotGcTask;
pub use summary_task::SummaryTask;
