//! todo 动态工具：让智能体在会话内创建/更新/跟踪待办清单。
//!
//! 待办是智能体可调用的工具（非纯 UI 功能）：agent 自主拆分任务、跟踪进度、
//! 标记完成。数据与会话绑定（`session_id`）：创建时归属当前会话，
//! list/update/delete 只作用于本会话条目；会话页输入框上方展示活跃待办，
//! 全部完成后面板消失（对齐 DSH todo_write 的临时面板语义）。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::todos::{TodoPriority, TodoStatus, TodoStore};
use tianyan::TianyanError;

/// todo 工具参数。
#[derive(Debug, Deserialize)]
pub struct TodoArgs {
    /// 操作：create | update | list | delete。
    pub operation: String,
    /// 待办 ID（update/delete 必填）。
    pub id: Option<String>,
    /// 标题（create 必填；update 可选）。
    pub title: Option<String>,
    /// 详细描述（可选）。
    pub description: Option<String>,
    /// 状态（update 可选：pending | in_progress | completed）。
    pub status: Option<String>,
    /// 优先级（create/update 可选：low | medium | high）。
    pub priority: Option<String>,
    /// 关联目标 id（可选）。
    pub goal_id: Option<String>,
}

/// todo 动态工具（ADR-003 组件工具化：server 层适配 core 动态工具 trait）。
pub struct TodoTool {
    store: Arc<TodoStore>,
}

impl TodoTool {
    /// 绑定待办存储。
    pub fn new(store: Arc<TodoStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DynamicToolExecutor for TodoTool {
    fn tool_name(&self) -> String {
        "todo".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::new(
            "todo",
            "管理当前会话的待办清单（会话绑定，仅本会话可见）：创建/更新/列出/删除待办。用于把任务拆解为可跟踪的待办、标记进度（pending/in_progress/completed）、关联目标。operation: create（title 必填）/ update（id + 可选字段）/ list（可选 status/goal_id 过滤）/ delete（id）。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["create", "update", "list", "delete"], "description": "操作类型" },
                    "id": { "type": "string", "description": "待办 ID（update/delete 必填）" },
                    "title": { "type": "string", "description": "标题（create 必填）" },
                    "description": { "type": "string", "description": "详细描述" },
                    "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "状态（update 可选）" },
                    "priority": { "type": "string", "enum": ["low", "medium", "high"], "description": "优先级（create/update 可选）" },
                    "goal_id": { "type": "string", "description": "关联目标 id（可选）" }
                },
                "required": ["operation"]
            }),
        ))
    }

    async fn execute(&self, session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        let args: TodoArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: todo 参数无效：{e}")))?;
        match args.operation.as_str() {
            "create" => {
                let title = args.title.unwrap_or_default();
                if title.trim().is_empty() {
                    return Err(TianyanError::invalid_input("tool: todo create 需要 title"));
                }
                let priority = match args.priority.as_deref() {
                    Some(p) => TodoPriority::parse(p).ok_or_else(|| {
                        TianyanError::invalid_input(format!("tool: todo 优先级无效：{p}"))
                    })?,
                    None => TodoPriority::Medium,
                };
                let item = self
                    .store
                    .create(
                        title,
                        args.description,
                        priority,
                        args.goal_id,
                        None,
                        Some(session_id.to_string()),
                    )
                    .await?;
                Ok(serde_json::json!({
                    "status": "created",
                    "id": item.id,
                    "title": item.title,
                    "priority": format!("{:?}", item.priority).to_lowercase(),
                    "goal_id": item.goal_id,
                }))
            }
            "update" => {
                let id = args
                    .id
                    .ok_or_else(|| TianyanError::invalid_input("tool: todo update 需要 id"))?;
                // 归属校验：只允许操作本会话的待办（跨会话 id 一律 not_found）
                self.owned_todo(session_id, &id).await?;
                let status = match args.status.as_deref() {
                    Some(s) => Some(TodoStatus::parse(s).ok_or_else(|| {
                        TianyanError::invalid_input(format!("tool: todo 状态无效：{s}"))
                    })?),
                    None => None,
                };
                let priority = match args.priority.as_deref() {
                    Some(p) => Some(TodoPriority::parse(p).ok_or_else(|| {
                        TianyanError::invalid_input(format!("tool: todo 优先级无效：{p}"))
                    })?),
                    None => None,
                };
                let updated = self
                    .store
                    .update(
                        &id,
                        args.title,
                        args.description,
                        status,
                        priority,
                        args.goal_id,
                        None,
                    )
                    .await?;
                match updated {
                    Some(item) => Ok(serde_json::json!({
                        "status": "updated",
                        "id": item.id,
                        "title": item.title,
                        "todo_status": format!("{:?}", item.status).to_lowercase(),
                    })),
                    None => Err(TianyanError::not_found(format!("tool: todo 不存在：{id}"))),
                }
            }
            "list" => {
                // 只列出本会话的待办（会话绑定数据源）
                let items = self.store.list_by_session(session_id).await;
                let filtered: Vec<_> = items
                    .into_iter()
                    .filter(|i| {
                        let status_ok = match args.status.as_deref() {
                            Some(s) => format!("{:?}", i.status).to_lowercase() == s,
                            None => true,
                        };
                        let goal_ok = match args.goal_id.as_deref() {
                            Some(g) => i.goal_id.as_deref() == Some(g),
                            None => true,
                        };
                        status_ok && goal_ok
                    })
                    .map(|i| {
                        serde_json::json!({
                            "id": i.id,
                            "title": i.title,
                            "status": format!("{:?}", i.status).to_lowercase(),
                            "priority": format!("{:?}", i.priority).to_lowercase(),
                            "goal_id": i.goal_id,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "todos": filtered, "count": filtered.len() }))
            }
            "delete" => {
                let id = args
                    .id
                    .ok_or_else(|| TianyanError::invalid_input("tool: todo delete 需要 id"))?;
                self.owned_todo(session_id, &id).await?;
                let removed = self.store.delete(&id).await?;
                if removed {
                    Ok(serde_json::json!({ "status": "deleted", "id": id }))
                } else {
                    Err(TianyanError::not_found(format!("tool: todo 不存在：{id}")))
                }
            }
            other => Err(TianyanError::invalid_input(format!(
                "tool: todo 未知操作：{other}"
            ))),
        }
    }
}

impl TodoTool {
    /// 校验待办归属当前会话（不存在或属于其他会话 → not_found）。
    async fn owned_todo(&self, session_id: &str, id: &str) -> Result<()> {
        let owned = self
            .store
            .list_by_session(session_id)
            .await
            .iter()
            .any(|i| i.id == id);
        if owned {
            Ok(())
        } else {
            Err(TianyanError::not_found(format!(
                "tool: todo 不存在或不属于当前会话：{id}"
            )))
        }
    }
}
