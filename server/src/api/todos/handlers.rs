//! 待办清单处理器。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde_json::json;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

use super::types::{CreateTodoRequest, UpdateTodoRequest};

/// 列出全部待办。
pub async fn list_todos(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let todos = state.todo_store().list().await;
    Ok(Json(json!({ "todos": todos, "total": todos.len() })))
}

/// 创建待办。
pub async fn create_todo(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTodoRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let priority = request.parse_priority().map_err(ApiError::BadRequest)?;
    let todo = state
        .todo_store()
        .create(
            request.title,
            request.description,
            priority,
            request.goal_id,
            request.due_at,
        )
        .await?;
    Ok(Json(json!({ "todo": todo })))
}

/// 更新待办（部分字段）。
pub async fn update_todo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateTodoRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let status = request.parse_status().map_err(ApiError::BadRequest)?;
    let priority = request.parse_priority().map_err(ApiError::BadRequest)?;
    let todo = state
        .todo_store()
        .update(
            &id,
            request.title,
            request.description,
            status,
            priority,
            request.goal_id,
            request.due_at,
        )
        .await?;
    match todo {
        Some(t) => Ok(Json(json!({ "todo": t }))),
        None => Err(ApiError::NotFound(format!("待办不存在：{id}"))),
    }
}

/// 删除待办。
pub async fn delete_todo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let removed = state.todo_store().delete(&id).await?;
    if !removed {
        return Err(ApiError::NotFound(format!("待办不存在：{id}")));
    }
    Ok(Json(json!({ "deleted": true })))
}
