//! 统一任务调度模块（间隔 + 补跑模型，ADR-024）。
//!
//! 本模块提供基于**执行间隔**的任务调度：单一扫描循环每 tick 顺序检查全部
//! 任务，「距上次执行 ≥ 间隔」即立即执行。个人 PC 场景下服务不常驻——
//! last_run 持久化使宕机错过的任务在重启后自动补跑（单次补跑语义）。
//!
//! ## 与 [`crate::agent::background`]（agent 后台任务）的边界
//!
//! 本模块是**系统级周期任务**（间隔驱动、纯后台执行、无唤醒语义）；
//! agent 后台任务（delegate background / 后台命令）是**交互驱动的一次性任务**
//! （完成/失败唤醒主 agent，ADR-013）。持久化载体亦不同：本模块 last_run 经
//! scheduler_state.json，agent 后台任务经 SQLite（background.rs `with_db`）。
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
//! - [`TaskRegistration`][]: 一条任务注册（装配原语：任务插件交给调度器的清单项）。
//! - [`TaskServices`][]: 任务装配时注入的能力集合（统计/快照/会话/通知/模型/工作集）。
//! - [`EvolutionReviewExecutor`][]: 演化综述执行器契约（server 构造实现，经
//!   [`TaskServices`] 注入演化任务插件）。
//!
//! # 具体任务实现不在本模块（ADR-049 期 2 序 5a / 5b）
//!
//! 5 个内置任务在 `plugins/task-builtin`，演化任务在 `plugins/task-evolution`
//! （`core/src/scheduler/tasks/` 已整目录删除）。本模块只剩**机制 + 装配原语**：
//! 间隔扫描与补跑、执行契约、注册机制、能力集合——改任务策略／新增任务不重编 core。

mod evolution_executor;
mod task_assembly;
mod task_scheduler;
mod task_state;

pub use evolution_executor::EvolutionReviewExecutor;
pub use task_assembly::{TaskRegistration, TaskServices};
pub use task_scheduler::{
    interval_from_cron, next_due_at, TaskContext, TaskDefinition, TaskHandler, TaskPriority,
    TaskResult, TaskScheduler,
};
pub use task_state::TaskStateStore;
