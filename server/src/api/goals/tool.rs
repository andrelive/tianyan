//! goal 动态工具：让智能体创建/更新/跟踪长期目标（计划面板数据同源）。
//!
//! 目标进度按关联待办完成比例自动计算（与 todolist 联动）——agent 用
//! `todo` 工具拆解任务、`goal` 工具跟踪目标，形成"目标 → 待办"闭环。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::goals::{GoalStatus, GoalStore};
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::TianyanError;

/// goal 工具参数。
#[derive(Debug, Deserialize)]
pub struct GoalArgs {
    /// 操作：create | update | list | delete。
    pub operation: String,
    /// 目标 ID（update/delete 必填）。
    pub id: Option<String>,
    /// 标题（create 必填；update 可选）。
    pub title: Option<String>,
    /// 详细描述（可选）。
    pub description: Option<String>,
    /// 状态（update 可选：active | completed | archived）。
    pub status: Option<String>,
}

/// goal 动态工具（ADR-003 组件工具化：server 层适配 core 动态工具 trait）。
pub struct GoalTool {
    store: Arc<GoalStore>,
}

impl GoalTool {
    /// 绑定目标存储。
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DynamicToolExecutor for GoalTool {
    fn tool_name(&self) -> String {
        "goal".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::new(
            "goal",
            "管理长期目标（计划面板数据同源）：创建/更新/列出/删除目标。目标进度按关联待办完成比例自动计算。operation: create（title 必填）/ update（id + 可选字段）/ list（可选 status 过滤）/ delete（id）。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["create", "update", "list", "delete"], "description": "操作类型" },
                    "id": { "type": "string", "description": "目标 ID（update/delete 必填）" },
                    "title": { "type": "string", "description": "标题（create 必填）" },
                    "description": { "type": "string", "description": "详细描述" },
                    "status": { "type": "string", "enum": ["active", "completed", "archived"], "description": "状态（update 可选）" }
                },
                "required": ["operation"]
            }),
        ))
    }

    async fn execute(&self, arguments: &str) -> Result<serde_json::Value> {
        let args: GoalArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: goal 参数无效：{e}")))?;
        match args.operation.as_str() {
            "create" => {
                let title = args.title.unwrap_or_default();
                if title.trim().is_empty() {
                    return Err(TianyanError::invalid_input("tool: goal create 需要 title"));
                }
                let goal = self.store.create(title, args.description, None).await?;
                Ok(serde_json::json!({
                    "status": "created",
                    "id": goal.id,
                    "title": goal.title,
                }))
            }
            "update" => {
                let id = args.id.ok_or_else(|| TianyanError::invalid_input("tool: goal update 需要 id"))?;
                let status = match args.status.as_deref() {
                    Some(s) => Some(GoalStatus::parse(s).ok_or_else(|| {
                        TianyanError::invalid_input(format!("tool: goal 状态无效：{s}"))
                    })?),
                    None => None,
                };
                let updated = self.store.update(&id, args.title, args.description, status, None).await?;
                match updated {
                    Some(goal) => Ok(serde_json::json!({
                        "status": "updated",
                        "id": goal.id,
                        "title": goal.title,
                        "goal_status": format!("{:?}", goal.status).to_lowercase(),
                    })),
                    None => Err(TianyanError::not_found(format!("tool: goal 不存在：{id}"))),
                }
            }
            "list" => {
                let goals = self.store.list().await;
                let filtered: Vec<_> = goals
                    .into_iter()
                    .filter(|g| {
                        match args.status.as_deref() {
                            Some(s) => format!("{:?}", g.status).to_lowercase() == s,
                            None => true,
                        }
                    })
                    .map(|g| {
                        serde_json::json!({
                            "id": g.id,
                            "title": g.title,
                            "status": format!("{:?}", g.status).to_lowercase(),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "goals": filtered, "count": filtered.len() }))
            }
            "delete" => {
                let id = args.id.ok_or_else(|| TianyanError::invalid_input("tool: goal delete 需要 id"))?;
                let removed = self.store.delete(&id).await?;
                if removed {
                    Ok(serde_json::json!({ "status": "deleted", "id": id }))
                } else {
                    Err(TianyanError::not_found(format!("tool: goal 不存在：{id}")))
                }
            }
            other => Err(TianyanError::invalid_input(format!("tool: goal 未知操作：{other}"))),
        }
    }
}
