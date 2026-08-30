//! 定时智能体任务（Scheduled Agent Tasks）。
//!
//! 用户可指令智能体建立一个定时任务：在指定周期（cron）调用智能体在指定
//! 工作区完成某项工作。任务定义持久化到 {data_dir}/scheduled_agent_tasks.json，
//! 由 ScheduledAgentTaskManager 后台循环按时触发并调用 agent 执行。

pub mod handlers;
pub mod manager;
pub mod routes;
pub mod tool;
pub mod types;

pub use manager::{ResultSinkBridge, ScheduledAgentTaskManager, SchedulerRegistrar, TaskRegistrar, TaskResultSink};
pub use types::ScheduledAgentTask;
