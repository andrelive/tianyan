//! schedule_task 动态工具：让智能体建立定时任务。
//!
//! # 解耦设计（不依赖 manager 类型）
//!
//! 工具**不持有** [`ScheduledAgentTaskManager`]（也不抱 Weak）——只依赖一个
//! **创建任务请求通道**（`mpsc::Sender<CreateTaskRequest>`）。装配层在 agent
//! 之外创建消费者（持 manager 的 Weak）监听通道处理请求，经 one-shot 响应返回
//! 结果。这样依赖方向单向：工具 → 通道 → 装配层消费者 → manager（无反向依赖），
//! `agent → 工具 → ... → handler → agent` 的功能环被**异步边界切断**（结构上
//! 无环，非 Weak 弱化）。通道生命周期随工具自动结束：agent drop → 工具 drop →
//! sender drop → 通道关闭 → 消费者退出。

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::TianyanError;

use crate::scheduled_tasks::types::{CreateScheduledTaskRequest, ScheduledAgentTask};

/// schedule_task 工具参数。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScheduleTaskArgs {
    /// 任务显示名称。
    pub name: String,
    /// 执行间隔（秒）——**旧参数**，仅在没给 `schedule` 时生效。
    /// 如 1800 = 每 30 分钟、86400 = 每天、604800 = 每周（最小 60）。
    #[serde(default)]
    pub interval_secs: u64,
    /// 调度规格（**推荐**，优先于 `interval_secs`）。
    ///
    /// - 定点：`{"kind":"daily","at":"15:30"}`（本地时间）、
    ///   `{"kind":"weekly","weekdays":[1,3,5],"at":"09:00"}`、
    ///   `{"kind":"once","at":"2026-11-05T15:30"}`
    /// - 间隔：`{"kind":"every","secs":1800}`
    #[serde(default)]
    pub schedule: Option<tianyan::scheduler::Schedule>,
    /// 错过策略（时刻点已过、但服务没在跑时怎么办）。
    ///
    /// - `{"kind":"skip"}`：只在准点跑，错过就等下一个时刻（**收盘后/定点类任务必须用它**）；
    /// - `{"kind":"within","grace_secs":28800}`：时刻点后 8 小时内可补，超出跳过；
    /// - `{"kind":"run_once"}`：错过就立即补跑一次（缺省；维护类任务用）。
    #[serde(default)]
    pub missed: Option<tianyan::scheduler::MissedPolicy>,
    /// 工作目录（agent 将在此目录完成工作）。
    pub workspace: String,
    /// 给智能体的指令（到点后让它做的事）。
    pub prompt: String,
}

/// 创建任务请求（工具 → 通道 → 装配层消费者 → manager）。
pub struct CreateTaskRequest {
    /// 创建请求参数。
    pub req: CreateScheduledTaskRequest,
    /// 一次性响应通道（工具等待创建结果；manager 不可用时返回 Err）。
    pub resp_tx: oneshot::Sender<std::result::Result<ScheduledAgentTask, String>>,
}

/// 定时任务动态工具（ADR-003 组件工具化：server 层适配 core 动态工具 trait）。
///
/// **不依赖 manager**：只持有创建请求通道。创建任务由装配层消费者（持 manager
/// Weak）处理——工具与 manager 之间是异步边界（channel），无引用依赖。
pub struct ScheduleTaskTool {
    req_tx: mpsc::Sender<CreateTaskRequest>,
}

impl ScheduleTaskTool {
    /// 绑定创建请求通道（装配层创建 channel 并注入）。
    pub fn new(req_tx: mpsc::Sender<CreateTaskRequest>) -> Self {
        Self { req_tx }
    }
}

#[async_trait]
impl DynamicToolExecutor for ScheduleTaskTool {
    fn tool_name(&self) -> String {
        "schedule_task".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        // schema 由 struct 派生（from_schema）——单一事实源
        ToolDefinition::function(FunctionDefinition::from_schema::<ScheduleTaskArgs>(
            "schedule_task",
            "创建一个后台定时任务：到点在指定工作目录调用智能体完成给定指令。\
             【两种调度】① 定点（推荐给「每天/每周某时刻」）——`schedule={\"kind\":\"daily\",\"at\":\"15:30\"}`（本地时间），也支持 weekly / once；\
             ② 间隔——`schedule={\"kind\":\"every\",\"secs\":1800}` 或旧参数 `interval_secs`（距上次执行达到间隔即触发，最小 60 秒）。\
             【错过策略】`missed` 决定「时刻点已过但服务没在跑」时怎么办：`{\"kind\":\"skip\"}` 只在准点跑、错过等下一个（**收盘后/盘中不能跑的定点任务必须用它**）；\
             `{\"kind\":\"within\",\"grace_secs\":28800}` 时刻点后 8 小时内可补、超出跳过；`{\"kind\":\"run_once\"}` 错过立即补跑一次（缺省，维护类任务用）。\
             注意 `run_once` + 间隔制的旧组合会在服务重启后立刻补跑——定点任务请显式指定 schedule 与 missed=skip/within。",
        ))
    }

    async fn execute(&self, _session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        let args: ScheduleTaskArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: schedule_task 参数无效：{e}")))?;
        if args.name.trim().is_empty() || args.prompt.trim().is_empty() {
            return Err(TianyanError::Custom(
                "tool: schedule_task 需要 name 与 prompt".to_string(),
            ));
        }
        // 间隔下限只对「间隔制」有意义（日历类任务没有这个概念）
        match &args.schedule {
            Some(tianyan::scheduler::Schedule::Every { secs }) if *secs < 60 => {
                return Err(TianyanError::Custom(
                    "tool: schedule_task 间隔最小 60 秒（个人 PC 无需更高频）".to_string(),
                ));
            }
            Some(_) => {}
            None if args.interval_secs < 60 => {
                return Err(TianyanError::Custom(
                    "tool: schedule_task interval_secs 最小 60（个人 PC 无需更高频）；\
                     定点任务请改用 schedule={\"kind\":\"daily\",\"at\":\"HH:MM\"}"
                        .to_string(),
                ));
            }
            None => {}
        }
        let req = CreateScheduledTaskRequest {
            name: args.name,
            interval_secs: match &args.schedule {
                Some(tianyan::scheduler::Schedule::Every { secs }) => *secs,
                // 日历类：落盘前由 manager 用 `schedule` 回填等价间隔（双写）
                Some(_) => 0,
                None => args.interval_secs,
            },
            schedule: args.schedule,
            missed: args.missed,
            workspace: args.workspace,
            prompt: args.prompt,
        };
        // 经通道发送创建请求（不持有 manager），等待装配层消费者响应
        let (resp_tx, resp_rx) = oneshot::channel();
        let request = CreateTaskRequest { req, resp_tx };
        self.req_tx.send(request).await.map_err(|_| {
            TianyanError::Custom("tool: schedule_task 定时任务管理器不可用".to_string())
        })?;
        let task = resp_rx
            .await
            .map_err(|_| {
                TianyanError::Custom("tool: schedule_task 定时任务管理器不可用".to_string())
            })?
            .map_err(|e| TianyanError::Custom(format!("tool: schedule_task 创建任务失败：{e}")))?;
        Ok(serde_json::json!({
            "status": "created",
            "id": task.id,
            "name": task.name,
            "cron": task.cron,
            // 与 REST API 同口径：对外一律 RFC3339（内部仍按 epoch 秒记账）
            "next_run_at": task
                .next_run_at
                .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
                .map(|d| d.to_rfc3339()),
        }))
    }
}
