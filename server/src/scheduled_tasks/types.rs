//! 定时智能体任务类型。

use serde::{Deserialize, Serialize};

/// 定时智能体任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledAgentTask {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// 执行间隔（秒）：距上次执行 ≥ 此值即在下个扫描周期执行（ADR-024 间隔制；
    /// 宕机补跑——服务未运行期间超期的任务在重启后自动执行一次）。
    /// 旧 JSON 无此字段 → default 0 → load 时从 cron 换算。
    #[serde(default)]
    pub interval_secs: u64,
    /// 旧 cron 表达式（ADR-024 迁移遗留；load 时换算为 interval_secs 后不再使用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
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
    /// 下次预计执行时间（epoch 秒；展示用近似值 = last_run_at + interval）。
    pub next_run_at: Option<i64>,
    /// 最近一次执行结果摘要。
    pub last_result: Option<String>,
}

impl ScheduledAgentTask {
    /// 创建新任务。
    pub fn new(name: String, interval_secs: u64, workspace: String, prompt: String) -> Self {
        Self {
            id: format!("sched-task-{}", uuid::Uuid::new_v4()),
            name,
            interval_secs,
            cron: None,
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
    /// 执行间隔（秒），如 1800 = 每 30 分钟、86400 = 每天（距上次执行 ≥24h 即触发）。
    pub interval_secs: u64,
    /// 工作目录（agent 执行命令/读写文件的根）。
    pub workspace: String,
    /// 给智能体的指令。
    pub prompt: String,
}
