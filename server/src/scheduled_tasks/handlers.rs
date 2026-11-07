//! 定时智能体任务 REST API 处理器。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::scheduled_tasks::types::{
    CreateScheduledTaskRequest, ScheduledTaskResponse, UpdateScheduledTaskRequest,
};
use crate::state::AppState;

/// 列出全部定时任务（无装配时返回空数组）。
///
/// 返回 [`ScheduledTaskResponse`]：时间字段对外统一 RFC3339（落盘仍是 epoch 秒，
/// 见该类型的文档）。
pub async fn list_tasks(State(state): State<Arc<AppState>>) -> Json<Vec<ScheduledTaskResponse>> {
    let tasks = match state.scheduled_agent_tasks().await {
        Some(m) => m.list().await,
        None => Vec::new(),
    };
    Json(tasks.iter().map(ScheduledTaskResponse::from).collect())
}

/// 创建定时任务（未装配 agent 时返回 503）。
pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateScheduledTaskRequest>,
) -> Result<Json<ScheduledTaskResponse>, (StatusCode, String)> {
    let m = state.scheduled_agent_tasks().await.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "定时任务未装配".to_string(),
        )
    })?;
    Ok(Json(ScheduledTaskResponse::from(&m.create(&req).await)))
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

/// 更新定时任务（`PUT` / `PATCH /scheduled-tasks/{id}`，**部分字段**语义）。
///
/// 两个方法都挂同一个处理器：请求体里没出现的字段保持原值（见
/// [`UpdateScheduledTaskRequest`]），因此 `PUT` 在这里不是「整体替换」——前端与
/// 智能体都只需要传要改的那几项，不必先把任务读出来抄一遍（抄漏一个字段就会把
/// 工作区或指令清空）。
///
/// 错误映射：404 = 任务不存在；400 = 参数非法（空名称 / 空指令 / 间隔 < 60 秒）；
/// 503 = 定时任务未装配（无模型服务）。
pub async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateScheduledTaskRequest>,
) -> Result<Json<ScheduledTaskResponse>, (StatusCode, String)> {
    let m = state.scheduled_agent_tasks().await.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "定时任务未装配".to_string(),
        )
    })?;
    match m.update(&id, &req).await {
        Ok(task) => Ok(Json(ScheduledTaskResponse::from(&task))),
        Err(e) if e.is_not_found() => Err((StatusCode::NOT_FOUND, e.to_string())),
        Err(e) if e.is_invalid_input() => Err((StatusCode::BAD_REQUEST, e.to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}
