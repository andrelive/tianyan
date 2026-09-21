//! goal 动态工具：让智能体在会话内创建/更新/跟踪长期目标。
//!
//! 目标进度按关联待办完成比例自动计算（与 todolist 联动）——agent 用
//! `todo` 工具拆解任务、`goal` 工具跟踪目标，形成"目标 → 待办"闭环。
//! 数据与会话绑定：创建时归属当前会话，list/update/delete 只作用于
//! 本会话条目；会话页展示活跃目标，完成/删除后面板消失。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::goals::{GoalStatus, GoalStore};
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::TianyanError;

/// goal 工具参数。
///
/// 单一形态：`status` **只表示 update 的设置值**。此前它兼作 list 过滤
/// （同一字段两义：update 设置 / list 筛选）——已去掉 list 的过滤分支以消除
/// 歧义（目标数量少，模型可直接从返回列表里筛）。类型化后非法状态由 serde
/// 直接拒绝（不再靠运行时字符串解析）。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GoalArgs {
    /// 操作：create | update | list | delete。
    pub operation: String,
    /// 目标 ID（update/delete 必填）。
    pub id: Option<String>,
    /// 标题（create 必填；update 可选）。
    pub title: Option<String>,
    /// 详细描述（可选）。
    pub description: Option<String>,
    /// 新状态（update 可选）。
    pub status: Option<GoalStatus>,
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
        // schema 由 struct 派生（from_schema）——单一事实源
        ToolDefinition::function(FunctionDefinition::from_schema::<GoalArgs>(
            "goal",
            "管理当前会话的长期目标（会话绑定，仅本会话可见）：创建/更新/列出/删除目标。目标进度按关联待办完成比例自动计算。operation: create（title 必填）/ update（id + 可选字段，status 为设置的新状态）/ list（列出本会话全部目标）/ delete（id）。",
        ))
    }

    async fn execute(&self, session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        let args: GoalArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: goal 参数无效：{e}")))?;
        match args.operation.as_str() {
            "create" => {
                let title = args.title.unwrap_or_default();
                if title.trim().is_empty() {
                    return Err(TianyanError::invalid_input("tool: goal create 需要 title"));
                }
                let goal = self
                    .store
                    .create(title, args.description, None, Some(session_id.to_string()))
                    .await?;
                Ok(serde_json::json!({
                    "status": "created",
                    "id": goal.id,
                    "title": goal.title,
                }))
            }
            "update" => {
                let id = args
                    .id
                    .ok_or_else(|| TianyanError::invalid_input("tool: goal update 需要 id"))?;
                // 归属校验：只允许操作本会话的目标
                self.owned_goal(session_id, &id).await?;
                // 已是 Option<GoalStatus>（serde 校验；非法值在参数解析阶段被拒）
                let status = args.status;
                let updated = self
                    .store
                    .update(&id, args.title, args.description, status, None)
                    .await?;
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
                // 只列出本会话的目标（会话绑定数据源）；`status` 不再兼作过滤
                // （一词两用已消除：它只表示 update 的设置值）
                let goals = self.store.list_by_session(session_id).await;
                let filtered: Vec<_> = goals
                    .into_iter()
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
                let id = args
                    .id
                    .ok_or_else(|| TianyanError::invalid_input("tool: goal delete 需要 id"))?;
                self.owned_goal(session_id, &id).await?;
                let removed = self.store.delete(&id).await?;
                if removed {
                    Ok(serde_json::json!({ "status": "deleted", "id": id }))
                } else {
                    Err(TianyanError::not_found(format!("tool: goal 不存在：{id}")))
                }
            }
            other => Err(TianyanError::invalid_input(format!(
                "tool: goal 未知操作：{other}"
            ))),
        }
    }
}

impl GoalTool {
    /// 校验目标归属当前会话（不存在或属于其他会话 → not_found）。
    async fn owned_goal(&self, session_id: &str, id: &str) -> Result<()> {
        let owned = self
            .store
            .list_by_session(session_id)
            .await
            .iter()
            .any(|g| g.id == id);
        if owned {
            Ok(())
        } else {
            Err(TianyanError::not_found(format!(
                "tool: goal 不存在或不属于当前会话：{id}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> (GoalTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (GoalTool::new(Arc::new(GoalStore::new(dir.path()))), dir)
    }

    #[tokio::test]
    async fn test_list_no_longer_filters_by_status() {
        // status 一词两用已消除：它只表示 update 的设置值，list 不再按其过滤
        let (t, _dir) = tool();
        t.execute("s-1", "{ \"operation\": \"create\", \"title\": \"目标A\" }")
            .await
            .unwrap();
        let listed = t
            .execute(
                "s-1",
                "{ \"operation\": \"list\", \"status\": \"completed\" }",
            )
            .await
            .unwrap();
        assert_eq!(listed["count"], 1, "list 不再按 status 过滤");
    }

    #[tokio::test]
    async fn test_invalid_status_rejected_by_serde() {
        // 类型化（GoalStatus）后非法状态在参数解析阶段就被拒（不再靠运行时解析）
        let (t, _dir) = tool();
        let err = t
            .execute(
                "s-1",
                "{ \"operation\": \"update\", \"id\": \"x\", \"status\": \"done\" }",
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("参数无效"), "{err}");
    }

    #[test]
    fn test_definition_schema_derives_from_struct() {
        // schema 由 struct 派生（from_schema → schemars::schema_for!）：
        // 顶层结构 + 嵌套类型的 definitions。
        let (t, _dir) = tool();
        let def = t.definition();
        let params = &def.function.parameters;
        assert_eq!(params["type"], "object");
        assert_eq!(params["required"], serde_json::json!(["operation"]));
        // status 的枚举取值（active/completed/archived）来自 GoalStatus 的
        // JsonSchema 派生——schemars 把嵌套类型放进 definitions、属性用 $ref 引用
        let all = params.to_string();
        assert!(all.contains("GoalStatus"), "{all}");
        assert!(
            all.contains("active") && all.contains("archived"),
            "枚举取值应来自 GoalStatus 派生：{all}"
        );
    }
}
