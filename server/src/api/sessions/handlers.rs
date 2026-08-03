use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use tracing::{error, info};

use crate::api::sessions::services::SessionService;
use crate::api::sessions::types::{
    DeleteMessageRequest, DeleteSessionResponse, ListSessionsResponse, RedoRequest, Session,
    SessionDetail, SessionMessagesResponse, UpdateTitleRequest,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 列出所有会话
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListSessionsResponse>, ApiError> {
    info!("列出所有会话");

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    service.list_sessions().await.map(Json).map_err(|e| {
        error!("列出会话失败: {}", e);
        ApiError::Internal(format!("列出会话失败: {}", e))
    })
}

/// 获取会话详情
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionDetail>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!("获取会话详情: {}", session_id);

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    service
        .get_session_detail(&session_id)
        .await
        .map(Json)
        .map_err(|e| {
            error!("获取会话详情失败: {}", e);
            ApiError::Internal(format!("获取会话详情失败: {}", e))
        })
}

/// 获取会话消息
pub async fn get_session_messages(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionMessagesResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!("获取会话消息: {}", session_id);

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    service
        .get_messages(&session_id)
        .await
        .map(Json)
        .map_err(|e| {
            error!("获取会话消息失败: {}", e);
            ApiError::Internal(format!("获取会话消息失败: {}", e))
        })
}

/// 删除会话
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<DeleteSessionResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!("删除会话: {}", session_id);

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    service
        .delete_session(&session_id)
        .await
        .map(Json)
        .map_err(|e| {
            error!("删除会话失败: {}", e);
            ApiError::Internal(format!("删除会话失败: {}", e))
        })
}

/// 删除消息（该消息及其后的所有消息）
pub async fn delete_message(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(request): Json<DeleteMessageRequest>,
) -> Result<Json<SessionMessagesResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!(
        "删除消息请求: 会话={}, 索引={}",
        session_id, request.message_index
    );

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    let resp = service.delete_message(&session_id, request).await?;
    Ok(Json(resp))
}

/// 重做被回退的消息与工作区文件
pub async fn redo_message(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(request): Json<RedoRequest>,
) -> Result<Json<SessionMessagesResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!(
        "重做消息请求: 会话={}, 索引={}",
        session_id, request.message_index
    );

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    let resp = service.redo_message(&session_id, request).await?;
    Ok(Json(resp))
}

/// 更新会话标题
pub async fn update_session_title(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateTitleRequest>,
) -> Result<Json<Session>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!("更新会话标题: {}", session_id);

    let service = SessionService::new(state.session_manager(), state.snapshot_manager());

    service
        .update_title(&session_id, request)
        .await
        .map(Json)
        .map_err(|e| {
            error!("更新会话标题失败: {}", e);
            ApiError::Internal(format!("更新会话标题失败: {}", e))
        })
}
