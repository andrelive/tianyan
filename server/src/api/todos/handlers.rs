//! 待办清单处理器。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

use super::types::{CreateTodoRequest, UpdateTodoRequest};

/// 列表查询参数。
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// 按归属会话过滤（会话页数据源；缺省返回全部）。
    pub session_id: Option<String>,
}

/// 列出待办（`?session_id=` 过滤归属会话）。
pub async fn list_todos(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let todos = match query.session_id.as_deref() {
        Some(sid) if !sid.trim().is_empty() => state.todo_store().list_by_session(sid).await,
        _ => state.todo_store().list().await,
    };
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
            request.session_id,
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
