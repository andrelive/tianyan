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
    /// 执行间隔（秒）：距上次执行 ≥ 此间隔即在下个扫描周期执行；
    /// 服务未运行期间超期的任务在重启后自动补跑一次。如 1800 = 每 30 分钟、
    /// 86400 = 每天、604800 = 每周。
    pub interval_secs: u64,
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
            "创建一个后台定时任务：按执行间隔周期调用智能体在指定工作目录完成给定指令。用于用户要求定时/周期执行某项工作（如每 30 分钟检查一次、每天总结一次）。间隔制（ADR-024）：距上次执行达到间隔即触发，服务未运行期间超期的任务会在重启后自动补跑一次。",
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
        if args.interval_secs < 60 {
            return Err(TianyanError::Custom(
                "tool: schedule_task interval_secs 最小 60（个人 PC 无需更高频）".to_string(),
            ));
        }
        let req = CreateScheduledTaskRequest {
            name: args.name,
            interval_secs: args.interval_secs,
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
            "next_run_at": task.next_run_at,
        }))
    }
}
