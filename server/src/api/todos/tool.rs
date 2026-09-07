//! todo 动态工具：让智能体在会话内创建/更新/跟踪待办清单。
//!
//! 待办是智能体可调用的工具（非纯 UI 功能）：agent 自主拆分任务、跟踪进度、
//! 标记完成。数据与会话绑定（`session_id`）：创建时归属当前会话，
//! list/update/delete 只作用于本会话条目；会话页输入框上方展示待办，
//! 完成条目保留展示（划线样式），无任何条目后面板消失（对齐 DSH todo_write
//! 的临时面板语义）。
//!
//! 当前批语义（同一会话同一时期只保留一批同源待办）：
//! - create 时若本会话已有待办且全部完成 → 自动移除旧批、建立新批；
//!   仍有未完成项则作为新工作追加进当前批。
//! - `close` 主动清空当前批全部待办（目标变更、现有待办不再相关时使用，
//!   即使有未完成项）；完成项划线保留，是否清理由智能体自行决定（delete）。
//!
//! 批量语义（对齐 DSH todo_write 的一次性整单写入）：create 接受 `todos`
//! 数组一次创建整份清单；update 接受 `updates` 数组一次合并多条状态变更；
//! delete 接受 `ids` 数组。子任务用 `parent_id` 挂靠父待办。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::todos::{TodoDraft, TodoItem, TodoPatch, TodoPriority, TodoStatus, TodoStore};
use tianyan::TianyanError;

/// 批量创建的单条输入。
#[derive(Debug, Deserialize)]
pub struct TodoDraftArgs {
    /// 标题（必填，非空）。
    pub title: String,
    /// 详细描述。
    #[serde(default)]
    pub description: Option<String>,
    /// 初始状态（缺省 pending）。
    #[serde(default)]
    pub status: Option<String>,
    /// 优先级（缺省 medium）。
    #[serde(default)]
    pub priority: Option<String>,
    /// 关联目标 id。
    #[serde(default)]
    pub goal_id: Option<String>,
    /// 父待办 id（子任务挂靠；父待办须已存在）。
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// 批量更新的单条输入。
#[derive(Debug, Deserialize)]
pub struct TodoUpdateArgs {
    /// 目标待办 id（必填）。
    pub id: String,
    /// 新标题。
    #[serde(default)]
    pub title: Option<String>,
    /// 新描述。
    #[serde(default)]
    pub description: Option<String>,
    /// 新状态（pending | in_progress | completed）。
    #[serde(default)]
    pub status: Option<String>,
    /// 新优先级。
    #[serde(default)]
    pub priority: Option<String>,
    /// 新关联目标 id（空串清除）。
    #[serde(default)]
    pub goal_id: Option<String>,
    /// 新父待办 id（空串清除；父待办须已存在且属于本会话）。
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// todo 工具参数（批量数组与单条快捷形态二选一）。
#[derive(Debug, Deserialize)]
pub struct TodoArgs {
    /// 操作：create | update | list | delete。
    pub operation: String,
    /// 批量创建（推荐）：一次传入整份清单。
    #[serde(default)]
    pub todos: Option<Vec<TodoDraftArgs>>,
    /// 批量更新（推荐）：一次合并多条状态变更。
    #[serde(default)]
    pub updates: Option<Vec<TodoUpdateArgs>>,
    /// 批量删除的待办 id 列表。
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    // —— 单条快捷形态（向后兼容；等价于长度 1 的批量）——
    /// 待办 ID（update/delete 单条形态必填）。
    #[serde(default)]
    pub id: Option<String>,
    /// 标题（create 单条形态必填）。
    #[serde(default)]
    pub title: Option<String>,
    /// 详细描述。
    #[serde(default)]
    pub description: Option<String>,
    /// 状态。
    #[serde(default)]
    pub status: Option<String>,
    /// 优先级。
    #[serde(default)]
    pub priority: Option<String>,
    /// 关联目标 id。
    #[serde(default)]
    pub goal_id: Option<String>,
    /// 父待办 id。
    #[serde(default)]
    pub parent_id: Option<String>,
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

    fn parse_status(s: &str) -> Result<TodoStatus> {
        TodoStatus::parse(s)
            .ok_or_else(|| TianyanError::invalid_input(format!("tool: todo 状态无效：{s}")))
    }

    fn parse_priority(s: &str) -> Result<TodoPriority> {
        TodoPriority::parse(s)
            .ok_or_else(|| TianyanError::invalid_input(format!("tool: todo 优先级无效：{s}")))
    }

    fn todo_json(i: &TodoItem) -> serde_json::Value {
        // 状态/优先级经 serde 序列化（snake_case/lowercase），与 REST API 语义一致；
        // Debug 格式会产出 "inprogress" 这类与 parse 口径不符的值（历史 bug）
        serde_json::json!({
            "id": i.id,
            "title": i.title,
            "status": i.status,
            "priority": i.priority,
            "goal_id": i.goal_id,
            "parent_id": i.parent_id,
        })
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
            "管理当前会话的待办清单（会话绑定，仅本会话可见；同一时期只保留一批同源待办）。支持批量，避免逐条调用：operation=create 时传 todos 数组一次性写入整份清单（推荐）——若当前批已全部完成，旧批自动移除并建立新批；仍有未完成项则作为新工作追加进当前批。operation=update 时传 updates 数组（id + 可选 status/title 等）一次合并多条状态变更——开始/完成多条时务必合并到一次调用。发现新工作随时 create 追加，子任务用 parent_id 挂到父待办下。operation=list 查看当前清单（含 id 与状态）。operation=delete 传 ids 数组删除指定项——完成项默认划线保留展示，是否清理由你自行决定。operation=close 清空当前批全部待办——当用户目标变更、现有待办不再反映当前意图时使用（即使有未完成项）。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["create", "update", "list", "delete", "close"], "description": "操作类型" },
                    "todos": {
                        "type": "array",
                        "description": "批量创建（推荐）：一次传入整份清单",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "标题（必填）" },
                                "description": { "type": "string", "description": "详细描述" },
                                "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "初始状态（缺省 pending）" },
                                "priority": { "type": "string", "enum": ["low", "medium", "high"], "description": "优先级（缺省 medium）" },
                                "goal_id": { "type": "string", "description": "关联目标 id" },
                                "parent_id": { "type": "string", "description": "父待办 id（子任务挂靠）" }
                            },
                            "required": ["title"]
                        }
                    },
                    "updates": {
                        "type": "array",
                        "description": "批量更新（推荐）：一次合并多条状态变更",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "待办 ID（必填）" },
                                "title": { "type": "string", "description": "新标题" },
                                "description": { "type": "string", "description": "新描述" },
                                "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "新状态" },
                                "priority": { "type": "string", "enum": ["low", "medium", "high"], "description": "新优先级" },
                                "goal_id": { "type": "string", "description": "新关联目标 id（空串清除）" },
                                "parent_id": { "type": "string", "description": "新父待办 id（空串清除）" }
                            },
                            "required": ["id"]
                        }
                    },
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "批量删除的待办 id 列表"
                    },
                    "id": { "type": "string", "description": "待办 ID（单条形态）" },
                    "title": { "type": "string", "description": "标题（单条创建）" },
                    "description": { "type": "string", "description": "详细描述（单条形态）" },
                    "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "状态（单条形态）" },
                    "priority": { "type": "string", "enum": ["low", "medium", "high"], "description": "优先级（单条形态）" },
                    "goal_id": { "type": "string", "description": "关联目标 id（单条形态）" },
                    "parent_id": { "type": "string", "description": "父待办 id（单条形态）" }
                },
                "required": ["operation"]
            }),
        ))
    }

    async fn execute(&self, session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        let args: TodoArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: todo 参数无效：{e}")))?;
        match args.operation.as_str() {
            "create" => self.create(session_id, args).await,
            "update" => self.update(session_id, args).await,
            "list" => self.list(session_id, &args).await,
            "delete" => self.delete(session_id, args).await,
            "close" => self.close(session_id).await,
            other => Err(TianyanError::invalid_input(format!(
                "tool: todo 未知操作：{other}"
            ))),
        }
    }
}

impl TodoTool {
    // —— create：批量数组优先，回落单条形态 ——
    async fn create(&self, session_id: &str, args: TodoArgs) -> Result<serde_json::Value> {
        // 当前批语义：本会话已有待办且全部完成 → 先移除旧批，再建新批
        let existing = self.store.list_by_session(session_id).await;
        if !existing.is_empty() && existing.iter().all(|i| i.status == TodoStatus::Completed) {
            let olds: Vec<String> = existing.iter().map(|i| i.id.clone()).collect();
            self.store.delete_many(&olds).await?;
        }
        let drafts: Vec<TodoDraftArgs> = match args.todos {
            Some(list) if !list.is_empty() => list,
            // 单条形态：title 视为必填
            _ => vec![TodoDraftArgs {
                title: args.title.unwrap_or_default(),
                description: args.description,
                status: args.status,
                priority: args.priority,
                goal_id: args.goal_id,
                parent_id: args.parent_id,
            }],
        };
        let mut drafts_parsed = Vec::with_capacity(drafts.len());
        for d in drafts {
            let status = match d.status.as_deref() {
                Some(s) => Some(Self::parse_status(s)?),
                None => None,
            };
            let priority = match d.priority.as_deref() {
                Some(p) => Some(Self::parse_priority(p)?),
                None => None,
            };
            drafts_parsed.push(TodoDraft {
                title: d.title,
                description: d.description,
                status,
                priority,
                goal_id: d.goal_id,
                parent_id: d.parent_id,
            });
        }
        let created = self
            .store
            .create_many(Some(session_id.to_string()), drafts_parsed)
            .await?;
        Ok(serde_json::json!({
            "status": "created",
            "count": created.len(),
            "todos": created.iter().map(Self::todo_json).collect::<Vec<_>>(),
        }))
    }

    // —— close：清空当前会话全部待办（目标变更时主动关闭当前批）——
    async fn close(&self, session_id: &str) -> Result<serde_json::Value> {
        let removed = self.store.delete_by_session(session_id).await;
        Ok(serde_json::json!({ "status": "closed", "removed": removed }))
    }

    // —— update：批量数组优先，回落单条形态；原子归属校验 ——
    async fn update(&self, session_id: &str, args: TodoArgs) -> Result<serde_json::Value> {
        let updates: Vec<TodoUpdateArgs> = match args.updates {
            Some(list) if !list.is_empty() => list,
            _ => vec![TodoUpdateArgs {
                id: args.id.ok_or_else(|| {
                    TianyanError::invalid_input("tool: todo update 需要 id 或 updates 数组")
                })?,
                title: args.title,
                description: args.description,
                status: args.status,
                priority: args.priority,
                goal_id: args.goal_id,
                parent_id: args.parent_id,
            }],
        };
        // 归属校验（原子）：任一 id 不属于当前会话则整体拒绝
        let owned: Vec<String> = self
            .store
            .list_by_session(session_id)
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        let unknown: Vec<String> = updates
            .iter()
            .map(|u| u.id.clone())
            .filter(|id| !owned.contains(id))
            .collect();
        if !unknown.is_empty() {
            return Err(TianyanError::not_found(format!(
                "tool: todo 不存在或不属于当前会话：{}",
                unknown.join(", ")
            )));
        }
        // parent_id 亦须属于本会话（空串 = 清除挂靠）
        let parent_unknown: Vec<String> = updates
            .iter()
            .filter_map(|u| u.parent_id.clone())
            .filter(|p| !p.is_empty() && !owned.contains(p))
            .collect();
        if !parent_unknown.is_empty() {
            return Err(TianyanError::not_found(format!(
                "tool: todo 父待办不存在或不属于当前会话：{}",
                parent_unknown.join(", ")
            )));
        }
        let mut patches = Vec::with_capacity(updates.len());
        for u in updates {
            let status = match u.status.as_deref() {
                Some(s) => Some(Self::parse_status(s)?),
                None => None,
            };
            let priority = match u.priority.as_deref() {
                Some(p) => Some(Self::parse_priority(p)?),
                None => None,
            };
            patches.push(TodoPatch {
                id: u.id,
                title: u.title,
                description: u.description,
                status,
                priority,
                goal_id: u.goal_id,
                parent_id: u.parent_id,
                due_at: None,
            });
        }
        let updated = self.store.update_many(patches).await?;
        Ok(serde_json::json!({
            "status": "updated",
            "count": updated.len(),
            "todos": updated.iter().map(Self::todo_json).collect::<Vec<_>>(),
        }))
    }

    // —— list：本会话条目（可选 status/goal_id 过滤）——
    async fn list(&self, session_id: &str, args: &TodoArgs) -> Result<serde_json::Value> {
        let items = self.store.list_by_session(session_id).await;
        let filtered: Vec<_> = items
            .into_iter()
            .filter(|i| {
                let status_ok = match args.status.as_deref() {
                    Some(s) => TodoStatus::parse(s) == Some(i.status),
                    None => true,
                };
                let goal_ok = match args.goal_id.as_deref() {
                    Some(g) => i.goal_id.as_deref() == Some(g),
                    None => true,
                };
                status_ok && goal_ok
            })
            .map(|i| Self::todo_json(&i))
            .collect();
        let count = filtered.len();
        Ok(serde_json::json!({ "todos": filtered, "count": count }))
    }

    // —— delete：批量数组优先，回落单条形态 ——
    async fn delete(&self, session_id: &str, args: TodoArgs) -> Result<serde_json::Value> {
        let ids: Vec<String> = match args.ids {
            Some(list) if !list.is_empty() => list,
            _ => vec![args.id.ok_or_else(|| {
                TianyanError::invalid_input("tool: todo delete 需要 id 或 ids 数组")
            })?],
        };
        // 归属校验（原子）
        let owned: Vec<String> = self
            .store
            .list_by_session(session_id)
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        let unknown: Vec<String> = ids
            .iter()
            .filter(|id| !owned.contains(id))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(TianyanError::not_found(format!(
                "tool: todo 不存在或不属于当前会话：{}",
                unknown.join(", ")
            )));
        }
        let removed = self.store.delete_many(&ids).await?;
        Ok(serde_json::json!({ "status": "deleted", "count": removed }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> (TodoTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (TodoTool::new(Arc::new(TodoStore::new(dir.path()))), dir)
    }

    #[tokio::test]
    async fn test_batch_create_and_list() {
        let (tool, _dir) = tool();
        let result = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"调研\", \"priority\": \"high\" }, { \"title\": \"实现\", \"status\": \"in_progress\" }, { \"title\": \"写文档\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(result["count"], 3);
        assert_eq!(result["todos"][1]["status"], "in_progress");

        // 单条快捷形态仍可用
        let single = tool
            .execute("s-1", "{ \"operation\": \"create\", \"title\": \"单条\" }")
            .await
            .unwrap();
        assert_eq!(single["count"], 1);

        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 4);
    }

    #[tokio::test]
    async fn test_batch_update_progress_and_ownership() {
        let (tool, _dir) = tool();
        let created = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"a\" }, { \"title\": \"b\" } ] }",
            )
            .await
            .unwrap();
        let todos = created["todos"].as_array().unwrap();
        let id_a = todos[0]["id"].as_str().unwrap().to_string();
        let id_b = todos[1]["id"].as_str().unwrap().to_string();

        // 一次调用推进两条
        let updated = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"update\", \"updates\": [ {{ \"id\": \"{id_a}\", \"status\": \"completed\" }}, {{ \"id\": \"{id_b}\", \"status\": \"in_progress\" }} ] }}"
                ),
            )
            .await
            .unwrap();
        assert_eq!(updated["count"], 2);
        assert_eq!(updated["todos"][0]["status"], "completed");
        assert_eq!(updated["todos"][1]["status"], "in_progress");

        // 跨会话 id → 整体拒绝（原子归属校验）
        tool.execute(
            "s-2",
            "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"他会的\" } ] }",
        )
        .await
        .unwrap();
        let err = tool
            .execute(
                "s-2",
                &format!(
                    "{{ \"operation\": \"update\", \"updates\": [ {{ \"id\": \"{id_a}\", \"status\": \"completed\" }} ] }}"
                ),
            )
            .await
            .unwrap_err();
        assert!(err.is_not_found());
        assert!(err.to_string().contains("不属于当前会话"));
    }

    #[tokio::test]
    async fn test_parent_id_subtask() {
        let (tool, _dir) = tool();
        let created = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"父任务\" } ] }",
            )
            .await
            .unwrap();
        let parent_id = created["todos"][0]["id"].as_str().unwrap().to_string();

        // 子任务挂靠
        let child = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"create\", \"todos\": [ {{ \"title\": \"子任务\", \"parent_id\": \"{parent_id}\" }} ] }}"
                ),
            )
            .await
            .unwrap();
        assert_eq!(child["todos"][0]["parent_id"], parent_id.as_str());

        // 挂靠不存在的父 → 报错
        let err = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"孤儿\", \"parent_id\": \"ghost\" } ] }",
            )
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());

        // 批量删除
        let deleted = tool
            .execute(
                "s-1",
                &format!("{{ \"operation\": \"delete\", \"ids\": [\"{parent_id}\"] }}"),
            )
            .await
            .unwrap();
        assert_eq!(deleted["count"], 1);
    }

    #[tokio::test]
    async fn test_create_auto_rotates_batch_when_all_completed() {
        let (tool, _dir) = tool();
        // 第一批：两条
        let created = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"旧1\" }, { \"title\": \"旧2\" } ] }",
            )
            .await
            .unwrap();
        let ids: Vec<String> = created["todos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect();
        // 全部完成
        let updates = ids
            .iter()
            .map(|id| format!("{{ \"id\": \"{id}\", \"status\": \"completed\" }}"))
            .collect::<Vec<_>>()
            .join(",");
        tool.execute(
            "s-1",
            &format!("{{ \"operation\": \"update\", \"updates\": [{updates}] }}"),
        )
        .await
        .unwrap();
        // 再 create → 旧批自动移除，新批只有新条目
        let new_batch = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"新1\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(new_batch["count"], 1);
        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 1, "旧批应被移除，仅剩新批");
        assert_eq!(list["todos"][0]["title"], "新1");
    }

    #[tokio::test]
    async fn test_create_appends_when_incomplete_remains() {
        let (tool, _dir) = tool();
        tool.execute(
            "s-1",
            "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"进行中\" } ] }",
        )
        .await
        .unwrap();
        // 有未完成项时 create → 追加进当前批
        let appended = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"新发现\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(appended["count"], 1);
        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 2, "未完成时新工作应追加，不移除旧批");
    }

    #[tokio::test]
    async fn test_close_clears_current_batch() {
        let (tool, _dir) = tool();
        tool.execute(
            "s-1",
            "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"a\" }, { \"title\": \"b\" } ] }",
        )
        .await
        .unwrap();
        // close 清空当前批（含未完成项）
        let closed = tool
            .execute("s-1", "{ \"operation\": \"close\" }")
            .await
            .unwrap();
        assert_eq!(closed["removed"], 2);
        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 0);
        // 只清本会话：他会话不受影响
        tool.execute(
            "s-2",
            "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"别的\" } ] }",
        )
        .await
        .unwrap();
        let _ = tool.execute("s-1", "{ \"operation\": \"close\" }").await;
        let other = tool
            .execute("s-2", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(other["count"], 1, "close 不应影响其他会话");
    }
}
