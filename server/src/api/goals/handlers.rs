//! 目标处理器。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde_json::json;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

use super::types::{CreateGoalRequest, UpdateGoalRequest};

/// 列出全部目标（含进度：关联待办完成比例）。
pub async fn list_goals(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let goals = state.goal_store().list().await;
    let mut items = Vec::with_capacity(goals.len());
    for goal in goals {
        let (todo_total, todo_done) = state.todo_store().count_by_goal(&goal.id).await;
        let progress = if todo_total == 0 {
            0
        } else {
            (todo_done as f64 * 100.0 / todo_total as f64).round() as u32
        };
        items.push(json!({
            "goal": goal,
            "progress": progress,
            "todo_total": todo_total,
            "todo_done": todo_done,
        }));
    }
    Ok(Json(json!({ "goals": items, "total": items.len() })))
}

/// 创建目标。
pub async fn create_goal(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateGoalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let goal = state
        .goal_store()
        .create(request.title, request.description, request.target_date)
        .await?;
    Ok(Json(json!({ "goal": goal })))
}

/// 更新目标（部分字段）。
pub async fn update_goal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateGoalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let status = request.parse_status().map_err(ApiError::BadRequest)?;
    let goal = state
        .goal_store()
        .update(&id, request.title, request.description, status, request.target_date)
        .await?;
    match goal {
        Some(g) => Ok(Json(json!({ "goal": g }))),
        None => Err(ApiError::NotFound(format!("目标不存在：{id}"))),
    }
}

/// 删除目标。
pub async fn delete_goal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let removed = state.goal_store().delete(&id).await?;
    if !removed {
        return Err(ApiError::NotFound(format!("目标不存在：{id}")));
    }
    Ok(Json(json!({ "deleted": true })))
}
