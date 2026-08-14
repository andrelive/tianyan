use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use tracing::{error, info};

use crate::api::sessions::services::SessionService;
use crate::api::sessions::types::{
    CompressSessionResponse, DeleteMessageRequest, DeleteSessionResponse, ListSessionsResponse,
    RedoRequest, Session, SessionDetail, SessionMessagesResponse, UpdateTitleRequest,
    UpdateWorkspaceRequest,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 列出所有会话
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListSessionsResponse>, ApiError> {
    info!("列出所有会话");

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    service
        .list_sessions()
        .await
        .inspect_err(|e| error!("列出会话失败: {}", e))
        .map(Json)
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

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    service
        .get_session_detail(&session_id)
        .await
        .inspect_err(|e| error!("获取会话详情失败: {}", e))
        .map(Json)
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

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    service
        .get_messages(&session_id)
        .await
        .inspect_err(|e| error!("获取会话消息失败: {}", e))
        .map(Json)
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

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    service
        .delete_session(&session_id)
        .await
        .inspect_err(|e| error!("删除会话失败: {}", e))
        .map(Json)
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

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

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

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    let resp = service.redo_message(&session_id, request).await?;
    Ok(Json(resp))
}

/// 手动压缩会话（与自动压缩共用处理逻辑）。
///
/// 压缩成功后同步执行会话转换点刷新：learned rules 缓存清空（下一轮重新
/// 检索）+ 技能注册表增量注册（GEPA 新技能对后续对话可见）。
pub async fn compress_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<CompressSessionResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!("手动压缩会话: {}", session_id);

    let agent = state.agent().await;
    // ? 传播：会话不存在时由 core 返回 not_found 语义（404），不吞成 Internal
    let compressed = agent.compress_session(&session_id).await?;
    Ok(Json(CompressSessionResponse { compressed }))
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

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    service
        .update_title(&session_id, request)
        .await
        .inspect_err(|e| error!("更新会话标题失败: {}", e))
        .map(Json)
}

/// 更新会话绑定的工作目录（工作区归属；空串清除绑定）。
pub async fn update_session_workspace(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateWorkspaceRequest>,
) -> Result<Json<Session>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!("更新会话工作目录: {}", session_id);

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    service
        .update_workspace(&session_id, &request.working_directory)
        .await
        .inspect_err(|e| error!("更新会话工作目录失败: {}", e))
        .map(Json)
}
