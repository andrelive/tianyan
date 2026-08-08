//! 后台任务视图：`GET /api/v1/tasks`。
//!
//! 提供 delegate_to_agent(background) 后台任务的只读列表（状态/结果/所属会话），
//! 供前端任务面板与调试使用；任务生命周期由 core 后台任务管理器驱动。

use std::sync::Arc;

use axum::extract::State;
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
