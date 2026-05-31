//! 后台任务模块。
//!
//! 本模块提供所有后台任务的实现。
//!
//! # 任务列表
//!
//! - [`SummaryTask`][]: 摘要生成任务，扫描 VFS 并生成摘要。
//! - [`MemoryTask`][]: 记忆提取任务，扫描会话并提取记忆。
//! - [`GcTask`][]: Garbage Collection 任务，扫描过期规则。

mod gc_task;
mod memory_task;
mod rule_recorder;
mod rule_suggester;
mod rule_task;
mod summary_task;

pub use gc_task::GcTask;
pub use memory_task::MemoryTask;
pub use rule_task::RuleTask;
pub use summary_task::SummaryTask;
