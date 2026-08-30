//! 待办 API 请求/响应类型。

use serde::Deserialize;
use tianyan::todos::{TodoPriority, TodoStatus};

/// 创建待办请求。
#[derive(Debug, Deserialize)]
pub struct CreateTodoRequest {
    /// 标题（必填，非空）。
    pub title: String,
    /// 详细描述（可选）。
    #[serde(default)]
    pub description: Option<String>,
    /// 优先级（low/medium/high；缺省 medium）。
    #[serde(default)]
    pub priority: Option<String>,
    /// 关联目标 id（可选）。
    #[serde(default)]
    pub goal_id: Option<String>,
    /// 截止时间（epoch 秒；可选）。
    #[serde(default)]
    pub due_at: Option<i64>,
}

impl CreateTodoRequest {
    /// 解析优先级（未知值 → 400）。
    pub fn parse_priority(&self) -> Result<TodoPriority, String> {
        match &self.priority {
            None => Ok(TodoPriority::Medium),
            Some(s) => TodoPriority::parse(s)
                .ok_or_else(|| format!("无效的优先级：{s}（可选：low / medium / high）")),
        }
    }
}

/// 更新待办请求（全部字段可选；缺省 = 不修改）。
#[derive(Debug, Deserialize)]
pub struct UpdateTodoRequest {
    /// 新标题（空串 = 不修改）。
    #[serde(default)]
    pub title: Option<String>,
    /// 新描述（缺省 = 不修改）。
    #[serde(default)]
    pub description: Option<String>,
    /// 新状态（pending/in_progress/completed；缺省 = 不修改）。
    #[serde(default)]
    pub status: Option<String>,
    /// 新优先级（low/medium/high；缺省 = 不修改）。
    #[serde(default)]
    pub priority: Option<String>,
    /// 关联目标 id（空串 = 解除关联）。
    #[serde(default)]
    pub goal_id: Option<String>,
    /// 截止时间（<=0 = 清除）。
    #[serde(default)]
    pub due_at: Option<i64>,
}

impl UpdateTodoRequest {
    /// 解析状态（未知值 → 400）。
    pub fn parse_status(&self) -> Result<Option<TodoStatus>, String> {
        match &self.status {
            None => Ok(None),
            Some(s) => TodoStatus::parse(s)
                .map(Some)
                .ok_or_else(|| format!("无效的状态：{s}（可选：pending / in_progress / completed）")),
        }
    }

    /// 解析优先级（未知值 → 400）。
    pub fn parse_priority(&self) -> Result<Option<TodoPriority>, String> {
        match &self.priority {
            None => Ok(None),
            Some(s) => TodoPriority::parse(s)
                .map(Some)
                .ok_or_else(|| format!("无效的优先级：{s}（可选：low / medium / high）")),
        }
    }
}
