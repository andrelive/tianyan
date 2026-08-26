//! 定时智能体任务类型。

use serde::{Deserialize, Serialize};

/// 定时智能体任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledAgentTask {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// Cron 表达式。语义：秒字段为 */N = 每 N 秒（如 */30 * * * * *）；
    /// 分字段为 */N = 每 N 分钟（如 0 */30 * * * *）；分/时字段为具体数字且其余
    /// 为 * = 每天该时刻（如 0 30 9 * * * = 每天 09:30）。
    pub cron: String,
    /// 工作目录（agent 执行命令/读写文件的根）。
    pub workspace: String,
    /// 给智能体的指令（到点后让它在这个工作区做的事）。
    pub prompt: String,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间（epoch 秒）。
    pub created_at: i64,
    /// 上次执行时间（epoch 秒；未执行过为 None）。
    pub last_run_at: Option<i64>,
    /// 下次触发时间（epoch 秒；首次创建时计算）。
    pub next_run_at: Option<i64>,
    /// 最近一次执行结果摘要。
    pub last_result: Option<String>,
}

impl ScheduledAgentTask {
    /// 创建新任务（首次 next_run 由 manager 计算）。
    pub fn new(name: String, cron: String, workspace: String, prompt: String) -> Self {
        Self {
            id: format!("sched-task-{}", uuid::Uuid::new_v4()),
            name,
            cron,
            workspace,
            prompt,
            enabled: true,
            created_at: chrono::Utc::now().timestamp(),
            last_run_at: None,
            next_run_at: None,
            last_result: None,
        }
    }
}

/// 创建定时任务请求。
#[derive(Debug, Clone, Deserialize)]
pub struct CreateScheduledTaskRequest {
    /// 任务显示名称。
    pub name: String,
    /// Cron 表达式（*/N 秒、*/N 分钟、每天 H:M）。
    pub cron: String,
    /// 工作目录（agent 执行工作的根）。
    pub workspace: String,
    /// 给智能体的指令。
    pub prompt: String,
}
