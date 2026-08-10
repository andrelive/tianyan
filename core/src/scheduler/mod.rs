//! 统一任务调度模块。
//!
//! 本模块提供基于 cron 表达式的任务调度功能，管理所有后台任务。
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
