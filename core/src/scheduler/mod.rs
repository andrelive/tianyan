//! 统一任务调度模块。
//!
//! 本模块提供基于 cron 表达式的任务调度功能，管理所有后台任务。
//!
//! ## 与 [`crate::agent::background`]（agent 后台任务）的边界
//!
//! 本模块是**系统级周期任务**（cron 驱动、纯后台执行、无唤醒语义）；
//! agent 后台任务（delegate background / 后台命令）是**交互驱动的一次性任务**
//! （完成/失败唤醒主 agent，ADR-013）。持久化载体亦不同：本模块任务状态经
//! VFS（[`TaskStateStore`]），agent 后台任务经 SQLite（background.rs `with_db`）。
//! 两类任务不共享状态机与载体，扩展时在各边界内进行。
//!
//! # 架构
//!
//! - [`TaskScheduler`][]: 统一任务调度器，管理所有后台任务。
//! - [`TaskHandler`][]: 任务处理 trait，所有后台任务必须实现。
//! - [`TaskContext`][]: 任务上下文，提供任务执行时访问核心服务。
//! - [`TaskDefinition`][]: 任务定义，包含任务的元数据和处理。
//! - [`TaskPriority`][]: 任务优先级枚举。
//! - [`TaskResult`][]: 任务执行结果。

mod task_scheduler;
mod task_state;
pub mod tasks;

pub use task_scheduler::{
    TaskContext, TaskDefinition, TaskHandler, TaskPriority, TaskResult, TaskScheduler,
};
pub use task_state::TaskStateStore;
