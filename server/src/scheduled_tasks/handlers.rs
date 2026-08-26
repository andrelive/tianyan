//! 定时智能体任务 REST API 处理器。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::scheduled_tasks::types::{CreateScheduledTaskRequest, ScheduledAgentTask};
use crate::state::AppState;

/// 列出全部定时任务（无装配时返回空数组）。
pub async fn list_tasks(State(state): State<Arc<AppState>>) -> Json<Vec<ScheduledAgentTask>> {
    let tasks = match state.scheduled_agent_tasks().await {
        Some(m) => m.list().await,
        None => Vec::new(),
    };
    Json(tasks)
}

/// 创建定时任务（未装配 agent 时返回 503）。
pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateScheduledTaskRequest>,
) -> Result<Json<ScheduledAgentTask>, (StatusCode, String)> {
    let m = state.scheduled_agent_tasks().await.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "定时任务未装配".to_string(),
        )
    })?;
    Ok(Json(m.create(&req).await))
}

/// 删除定时任务。
pub async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let removed = match state.scheduled_agent_tasks().await {
        Some(m) => m.delete(&id).await,
        None => false,
    };
    Json(serde_json::json!({ "deleted": removed, "id": id }))
}
