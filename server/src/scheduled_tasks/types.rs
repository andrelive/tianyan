//! 定时智能体任务类型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 定时智能体任务（**持久化模型**）。
///
/// 时间字段是 epoch 秒 —— 这是 `scheduled_agent_tasks.json` 的落盘格式，**刻意不改它**：
/// 换成 RFC3339 会让旧版本读到新文件时整个反序列化失败，升级就变成不可逆的
/// （回退要手工改 JSON）。对外契约见 [`ScheduledTaskResponse`]（统一 RFC3339）。
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
            created_at: Utc::now().timestamp(),
            last_run_at: None,
            next_run_at: None,
            last_result: None,
        }
    }
}

/// 定时任务 API 响应（**对外契约：时间统一 RFC3339**）。
///
/// 产品其余接口（sessions / skills / memory / goals / todos）一律 RFC3339，只有这里
/// 原本输出 epoch 秒，前端因此得为它单独维护一条解析分支。统一放在这一层做：
/// **落盘格式不动，只改对外表示** —— 既消掉了「两套时间格式」，又保住可回退。
#[derive(Debug, Clone, Serialize)]
pub struct ScheduledTaskResponse {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// 执行间隔（秒）。
    pub interval_secs: u64,
    /// 工作目录。
    pub workspace: String,
    /// 给智能体的指令。
    pub prompt: String,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间（RFC3339）。
    pub created_at: DateTime<Utc>,
    /// 上次执行时间（RFC3339；未执行过为 None）。
    pub last_run_at: Option<DateTime<Utc>>,
    /// 下次预计执行时间（RFC3339）。
    pub next_run_at: Option<DateTime<Utc>>,
    /// 最近一次执行结果摘要。
    pub last_result: Option<String>,
}

impl From<&ScheduledAgentTask> for ScheduledTaskResponse {
    fn from(task: &ScheduledAgentTask) -> Self {
        // epoch → UTC；越界值（理论上不会出现）退化为「现在」，不 panic
        let to_dt = |secs: i64| DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_else(Utc::now);
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            interval_secs: task.interval_secs,
            workspace: task.workspace.clone(),
            prompt: task.prompt.clone(),
            enabled: task.enabled,
            created_at: to_dt(task.created_at),
            last_run_at: task.last_run_at.map(to_dt),
            next_run_at: task.next_run_at.map(to_dt),
            last_result: task.last_result.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ScheduledAgentTask {
        ScheduledAgentTask {
            id: "t1".to_string(),
            name: "n".to_string(),
            interval_secs: 60,
            cron: None,
            workspace: String::new(),
            prompt: "p".to_string(),
            enabled: true,
            created_at: 1_700_000_000,
            last_run_at: Some(1_700_000_100),
            next_run_at: None,
            last_result: None,
        }
    }

    /// 落盘保持 epoch 秒（旧版本能读 → 可回退），对外输出 RFC3339（统一口径）。
    #[test]
    fn persistent_keeps_epoch_but_response_is_rfc3339() {
        let task = sample();
        let disk = serde_json::to_string(&task).expect("序列化");
        assert!(
            disk.contains("\"created_at\":1700000000"),
            "落盘必须保持 epoch 秒：{disk}"
        );

        let resp = ScheduledTaskResponse::from(&task);
        let api = serde_json::to_string(&resp).expect("序列化");
        assert!(
            api.contains("\"created_at\":\"20"),
            "API 应输出 RFC3339 字符串：{api}"
        );
        assert_eq!(resp.created_at.timestamp(), 1_700_000_000);
        assert_eq!(
            resp.last_run_at.map(|d| d.timestamp()),
            Some(1_700_000_100),
            "last_run_at 也要转换"
        );
        assert!(resp.next_run_at.is_none());
    }
}
