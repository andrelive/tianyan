//! todo 动态工具：让智能体在会话内创建/更新/跟踪待办清单。
//!
//! 待办是智能体可调用的工具（非纯 UI 功能）：agent 自主拆分任务、跟踪进度、
//! 标记完成。数据与会话绑定（`session_id`）：创建时归属当前会话，
//! list/update/delete 只作用于本会话条目；会话页输入框上方展示待办，
//! 完成条目保留展示（划线样式），无任何条目后面板消失（对齐 DSH todo_write
//! 的临时面板语义）。
//!
//! 当前批语义（同一会话同一时期只有一批同源待办；create = 整表替换，
//! 对齐 DSH todo_write 的 last-write-wins）：
//! - create 传 `todos` 数组 = "当前完整计划"，旧批（含已完成项）整体替换——
//!   未包含的旧条目即被移除（被中止/放弃的任务随重规划自然消失）；
//!   想保留的条目（包括已完成的）必须包含在新列表中。
//! - 推进/完成用 `update`（id 批量合并，比全量重发省）；`close` 主动清空
//!   当前批（目标变更、整批作废时使用）；`delete` 按 id 删除；完成项批内
//!   划线保留；子任务建后用 update 的 parent_id 挂靠。

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
    /// 父待办 id（整表替换下不可用——非空会被拒绝；请先创建、再用 update 挂靠）。
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

/// todo 工具参数。
///
/// **形态唯一**：每种 operation 只有一种写法——要改/删**单条也放进数组**
/// （长度 1）。此前 update/delete 另有「单条快捷形态」（顶层 `id` + 各字段），
/// 与数组形态表达同一意图 → 同一份意图两套 schema，是模型参数生成的抖动源
/// （与已收敛的 `ask_user` 同型问题）。create 早已收敛（只接受 `todos`）。
#[derive(Debug, Deserialize)]
pub struct TodoArgs {
    /// 操作：create | update | list | delete | close。
    pub operation: String,
    /// 整表替换（create 必填）：当前完整计划——未包含的旧条目即被移除。
    #[serde(default)]
    pub todos: Option<Vec<TodoDraftArgs>>,
    /// 更新列表（update 必填；单条更新也放进数组，长度 1）。
    #[serde(default)]
    pub updates: Option<Vec<TodoUpdateArgs>>,
    /// 删除列表（delete 必填；单条删除也放进数组，长度 1）。
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    /// list 过滤：只返回该状态的条目（可选）。
    #[serde(default)]
    pub status: Option<String>,
    /// list 过滤：只返回关联到该目标的条目（可选）。
    #[serde(default)]
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
            "管理当前会话的待办清单（会话绑定，仅本会话可见；同一时期只保留一批同源待办）。**每种操作只有一种写法**——要改/删单条也放进数组（长度 1）：create 传 `todos` 数组 = 当前完整计划（**整表替换**——未包含的旧条目（含未完成项）即被移除；想保留的条目必须包含在新列表中）；update 传 `updates` 数组（按 id 批量合并状态）；delete 传 `ids` 数组；list 查看当前清单（可用 status / goal_id 过滤）；close 清空当前批全部待办（用户目标变更、现有待办不再反映当前意图时使用，即使有未完成项）。开始多步工作先写入整份清单；推进/完成用 update（比全量重发省）；计划增删改时重发完整列表。子任务挂靠：先创建任务，再用 update 的 parent_id 挂靠（整表替换中 parent_id 不可用）。完成项默认划线保留展示，是否清理由你自行决定。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["create", "update", "list", "delete", "close"], "description": "操作类型" },
                    "todos": {
                        "type": "array",
                        "description": "整表替换：当前完整计划（未包含的旧条目即被移除）",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "标题（必填）" },
                                "description": { "type": "string", "description": "详细描述" },
                                "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "初始状态（缺省 pending）" },
                                "priority": { "type": "string", "enum": ["low", "medium", "high"], "description": "优先级（缺省 medium）" },
                                "goal_id": { "type": "string", "description": "关联目标 id" },
                                "parent_id": { "type": "string", "description": "父待办 id（整表替换下不可用；建后经 update 挂靠）" }
                            },
                            "required": ["title"]
                        }
                    },
                    "updates": {
                        "type": "array",
                        "description": "更新列表（update 必填；单条更新也放进数组，长度 1）",
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
                        "description": "删除列表（delete 必填；单条删除也放进数组，长度 1）"
                    },
                    "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "list 过滤：只返回该状态的条目（可选）" },
                    "goal_id": { "type": "string", "description": "list 过滤：只返回关联到该目标的条目（可选）" }
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
    // —— create：整表替换（todos 数组 = 当前完整计划；未包含的旧条目即被移除） ——
    async fn create(&self, session_id: &str, args: TodoArgs) -> Result<serde_json::Value> {
        let drafts: Vec<TodoDraftArgs> = match args.todos {
            Some(list) if !list.is_empty() => list,
            _ => {
                return Err(TianyanError::invalid_input(
                    "tool: todo create 需要非空 todos 数组（整表替换：传入当前完整清单）",
                ))
            }
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
        let (created, replaced) = self.store.replace_many(session_id, drafts_parsed).await?;
        Ok(serde_json::json!({
            "status": "created",
            "count": created.len(),
            "replaced": replaced,
            "todos": created.iter().map(Self::todo_json).collect::<Vec<_>>(),
        }))
    }

    // —— close：清空当前会话全部待办（目标变更时主动关闭当前批）——
    async fn close(&self, session_id: &str) -> Result<serde_json::Value> {
        let removed = self.store.delete_by_session(session_id).await;
        Ok(serde_json::json!({ "status": "closed", "removed": removed }))
    }

    // —— update：只接受 updates 数组（单条更新也放进数组）；原子归属校验 ——
    async fn update(&self, session_id: &str, args: TodoArgs) -> Result<serde_json::Value> {
        let updates: Vec<TodoUpdateArgs> = match args.updates {
            Some(list) if !list.is_empty() => list,
            _ => {
                return Err(TianyanError::invalid_input(
                    "tool: todo update 需要非空 updates 数组（单条更新也放进数组，长度 1）",
                ))
            }
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

    // —— delete：只接受 ids 数组（单条删除也放进数组） ——
    async fn delete(&self, session_id: &str, args: TodoArgs) -> Result<serde_json::Value> {
        let ids: Vec<String> = match args.ids {
            Some(list) if !list.is_empty() => list,
            _ => {
                return Err(TianyanError::invalid_input(
                    "tool: todo delete 需要非空 ids 数组（单条删除也放进数组，长度 1）",
                ))
            }
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
        assert_eq!(result["replaced"], 0);
        assert_eq!(result["todos"][1]["status"], "in_progress");

        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 3);

        // create 为整表替换：再次 create 单条新批 → 旧批 3 条被整体替换
        let replaced = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"新批\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(replaced["count"], 1);
        assert_eq!(replaced["replaced"], 3);
        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 1);

        // create 不再接受单条快捷形态（缺 todos → 明确报错）
        let err = tool
            .execute("s-1", "{ \"operation\": \"create\", \"title\": \"单条\" }")
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());
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
        // 先创建整批（整表替换语义），再用 update 挂靠
        let created = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"父任务\" }, { \"title\": \"待挂子任务\" } ] }",
            )
            .await
            .unwrap();
        let parent_id = created["todos"][0]["id"].as_str().unwrap().to_string();
        let child_id = created["todos"][1]["id"].as_str().unwrap().to_string();
        // update 挂靠
        let child = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"update\", \"updates\": [ {{ \"id\": \"{child_id}\", \"parent_id\": \"{parent_id}\" }} ] }}"
                ),
            )
            .await
            .unwrap();
        assert_eq!(child["todos"][0]["parent_id"], parent_id.as_str());

        // create 带 parent_id → 明确拒绝（整表替换：旧条目将被替换、新 id 未生成）
        let err = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"create\", \"todos\": [ {{ \"title\": \"孤儿\", \"parent_id\": \"{parent_id}\" }} ] }}"
                ),
            )
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());

        // update 挂靠不存在的父 → 报错
        let err = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"update\", \"updates\": [ {{ \"id\": \"{child_id}\", \"parent_id\": \"ghost\" }} ] }}"
                ),
            )
            .await
            .unwrap_err();
        assert!(err.is_not_found());

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
    async fn test_create_replaces_whole_batch() {
        let (tool, _dir) = tool();
        // 旧批 2 条：1 完成 + 1 未完成——旧语义下"未完成项"导致只追加不替换（回归点）
        let created = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"旧完成\", \"status\": \"completed\" }, { \"title\": \"旧未完成\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(created["count"], 2);
        assert_eq!(created["replaced"], 0);
        // 整表替换：新批完全取代旧批（未包含的旧条目即被移除）
        let new_batch = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"新1\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(new_batch["count"], 1);
        assert_eq!(new_batch["replaced"], 2);
        let list = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(list["count"], 1, "旧批（含未完成项）应被整表替换移除");
        assert_eq!(list["todos"][0]["title"], "新1");
        // 全完成批同样被替换（原"全完成才轮换"逻辑被整表替换覆盖）
        tool.execute(
            "s-1",
            "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"全完成\", \"status\": \"completed\" } ] }",
        )
        .await
        .unwrap();
        let third = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"新2\" } ] }",
            )
            .await
            .unwrap();
        assert_eq!(third["replaced"], 1);
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

    #[tokio::test]
    async fn test_single_item_shapes_are_rejected() {
        // 形态唯一（与已收敛的 ask_user 同型）：update/delete 只有数组写法——
        // 旧「单条快捷形态」（顶层 id + 字段）必须被拒并给出改法提示。
        let (tool, _dir) = tool();
        let created = tool
            .execute(
                "s-1",
                "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"A\" } ] }",
            )
            .await
            .unwrap();
        let id = created["todos"][0]["id"].as_str().unwrap().to_string();

        // update 单条形态 → 拒绝（invalid_input）
        let err = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"update\", \"id\": \"{id}\", \"status\": \"completed\" }}"
                ),
            )
            .await
            .unwrap_err();
        assert!(err.is_invalid_input(), "{err}");

        // delete 单条形态 → 拒绝
        let err = tool
            .execute(
                "s-1",
                &format!("{{ \"operation\": \"delete\", \"id\": \"{id}\" }}"),
            )
            .await
            .unwrap_err();
        assert!(err.is_invalid_input(), "{err}");

        // 数组写法（长度 1）→ 接受
        let updated = tool
            .execute(
                "s-1",
                &format!(
                    "{{ \"operation\": \"update\", \"updates\": [ {{ \"id\": \"{id}\", \"status\": \"completed\" }} ] }}"
                ),
            )
            .await
            .unwrap();
        assert_eq!(updated["count"], 1);
        let deleted = tool
            .execute(
                "s-1",
                &format!("{{ \"operation\": \"delete\", \"ids\": [\"{id}\"] }}"),
            )
            .await
            .unwrap();
        assert_eq!(deleted["count"], 1);
    }

    #[tokio::test]
    async fn test_list_filters_by_status() {
        // list 的 status 过滤此前只存在于实现里（schema 描述写的是"update 单条形态"，
        // 功能被藏且无覆盖）——语义单一化后补上覆盖。
        let (tool, _dir) = tool();
        tool.execute(
            "s-1",
            "{ \"operation\": \"create\", \"todos\": [ { \"title\": \"待办1\", \"status\": \"completed\" }, { \"title\": \"待办2\" } ] }",
        )
        .await
        .unwrap();
        let done = tool
            .execute(
                "s-1",
                "{ \"operation\": \"list\", \"status\": \"completed\" }",
            )
            .await
            .unwrap();
        assert_eq!(done["count"], 1);
        assert_eq!(done["todos"][0]["title"], "待办1");
        let all = tool
            .execute("s-1", "{ \"operation\": \"list\" }")
            .await
            .unwrap();
        assert_eq!(all["count"], 2, "无过滤应返回全部");
    }
}
