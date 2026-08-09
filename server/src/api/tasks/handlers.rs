//! 后台任务视图：`GET /api/v1/tasks` + `POST /api/v1/tasks/{id}/cancel`。
//!
//! 提供 delegate_to_agent(background) 后台任务的只读列表与取消
//! （状态/结果/所属会话），供前端任务面板使用；任务生命周期由 core
//! 后台任务管理器驱动。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 后台任务列表。
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<tianyan::agent::background::BackgroundTask>>, ApiError> {
    let agent = state.agent().await;
    let tasks = agent.background_tasks().await;
    Ok(Json(tasks))
}

/// 取消后台任务（终态任务幂等；任务不存在返回 404）。
pub async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent = state.agent().await;
    let cancelled = agent
        .cancel_background_task(&task_id)
        .await
        .map_err(|e| ApiError::Internal(format!("取消任务失败: {}", e)))?;
    if !cancelled {
        return Err(ApiError::NotFound(format!("任务不存在：{}", task_id)));
    }
    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "status": "cancelled",
    })))
}
