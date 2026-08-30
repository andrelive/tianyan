//! schedule_task 动态工具：让智能体建立定时任务。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::TianyanError;

use crate::scheduled_tasks::manager::ScheduledAgentTaskManager;
use crate::scheduled_tasks::types::CreateScheduledTaskRequest;

/// schedule_task 工具参数。
#[derive(Debug, Deserialize)]
pub struct ScheduleTaskArgs {
    /// 任务显示名称。
    pub name: String,
    /// Cron 表达式：*/N 秒、*/N 分钟、每天 H:M。如 "0 */30 * * * *" = 每 30 分钟。
    pub cron: String,
    /// 工作目录（agent 将在此目录完成工作）。
    pub workspace: String,
    /// 给智能体的指令（到点后让它做的事）。
    pub prompt: String,
}

/// 定时任务动态工具（ADR-003 组件工具化：server 层适配 core 动态工具 trait）。
///
/// 持 manager 的 [std::sync::Weak]：agent（→ ToolRegistry → ScheduleTaskTool）→ manager
/// → registrar → scheduler → handler → agent_lock → agent 构成功能环，强引用会阻止
/// AppState drop 时组件释放（SQLite 文件锁无法解除）；工具调用发生在 agent 运行期间
/// （manager 必然存活，upgrade 成功），Weak 不损失功能。
pub struct ScheduleTaskTool {
    manager: std::sync::Weak<ScheduledAgentTaskManager>,
}

impl ScheduleTaskTool {
    /// 绑定到定时任务管理器（Weak——agent ↔ manager ↔ scheduler ↔ handler 功能环）。
    pub fn new(manager: Arc<ScheduledAgentTaskManager>) -> Self {
        Self {
            manager: Arc::downgrade(&manager),
        }
    }
}

#[async_trait]
impl DynamicToolExecutor for ScheduleTaskTool {
    fn tool_name(&self) -> String {
        "schedule_task".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::new(
            "schedule_task",
            "创建一个后台定时任务：按 cron 周期调用智能体在指定工作目录完成给定指令。用于用户要求定时/周期执行某项工作（如每 30 分钟、或每天固定时间检查项目）。cron 支持：*/N 秒、*/N 分钟、每天 H:M。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "任务显示名称" },
                    "cron": { "type": "string", "description": "Cron 表达式，如 '0 */30 * * * *'（每30分钟）或 '0 30 9 * * *'（每天09:30）" },
                    "workspace": { "type": "string", "description": "工作目录绝对路径" },
                    "prompt": { "type": "string", "description": "到点后让智能体在该工作区完成的工作指令" }
                },
                "required": ["name", "cron", "workspace", "prompt"]
            }),
        ))
    }

    async fn execute(&self, arguments: &str) -> Result<serde_json::Value> {
        let args: ScheduleTaskArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: schedule_task 参数无效：{e}")))?;
        if args.name.trim().is_empty() || args.prompt.trim().is_empty() {
            return Err(TianyanError::Custom(
                "tool: schedule_task 需要 name 与 prompt".to_string(),
            ));
        }
        if args.cron.trim().is_empty() {
            return Err(TianyanError::Custom(
                "tool: schedule_task 需要 cron（如 '0 */30 * * * *'）".to_string(),
            ));
        }
        let req = CreateScheduledTaskRequest {
            name: args.name,
            cron: args.cron,
            workspace: args.workspace,
            prompt: args.prompt,
        };
        // Weak upgrade：manager 已销毁（应用关停中）时工具调用失败可接受
        let Some(manager) = self.manager.upgrade() else {
            return Err(TianyanError::Custom(
                "tool: schedule_task 定时任务管理器不可用".to_string(),
            ));
        };
        let task = manager.create(&req).await;
        Ok(serde_json::json!({
            "status": "created",
            "id": task.id,
            "name": task.name,
            "cron": task.cron,
            "next_run_at": task.next_run_at,
        }))
    }
}
