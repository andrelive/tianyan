use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use tracing::{error, info};

use crate::api::sessions::services::SessionService;
use crate::api::sessions::types::{
    CompressSessionResponse, DeleteMessageRequest, DeleteSessionResponse, ListSessionsResponse,
    RedoRequest, Session, SessionDetail, SessionMessagesQuery, SessionMessagesResponse,
    UpdateTitleRequest, UpdateWorkspaceRequest,
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
    Query(query): Query<SessionMessagesQuery>,
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

    // 分段加载（ADR-035 §8）：带 `before_seq`/`limit` 时只取一页（带 seq 与
    // 上滚游标）；无分页参数时保持全量语义（向后兼容，前端切换到上滚后再改默认）。
    let result = if query.before_seq.is_some() || query.limit.is_some() {
        service
            .get_messages_page(&session_id, query.before_seq, query.limit)
            .await
    } else {
        service.get_messages(&session_id).await
    };
    result
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

    let response = service
        .delete_session(&session_id)
        .await
        .inspect_err(|e| error!("删除会话失败: {}", e))?;

    // 会话绑定的待办/目标级联清理（todo/goal 是会话内的临时推理辅助，
    // 会话删除后数据即失效；清理失败仅告警，不阻塞会话删除）
    let todos = state.todo_store().delete_by_session(&session_id).await;
    let goals = state.goal_store().delete_by_session(&session_id).await;
    if todos > 0 || goals > 0 {
        info!(session = %session_id, todos, goals, "已级联清理会话绑定的待办/目标");
    }

    Ok(Json(response))
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
        "删除消息请求: 会话={}, 消息={}",
        session_id, request.message_id
    );

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        state.agent_working_directory().await,
    );

    // 回退前置编排：① 停止 LLM 输出（置位 cancel 标志，AgentLoop 在轮次
    // 边界停止）→ ② 取消时序锚点之后的任务（委托 cancel + 命令 kill）。
    // ③ 回退文件到快照 + ④ 链截断丢弃由 delete_message 完成（基于完整链，
    // 压缩点前历史保留）。四种状态（有无输出/有无任务）统一覆盖，每步幂等。
    // ① 停止 LLM 输出：置位该会话的流取消标志（与「停止」按钮同语义）
    if let Some(flag) = state
        .stream_cancels()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&session_id)
        .cloned()
    {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let agent = state.agent().await;
    if let Err(e) = agent
        .rollback_session(&session_id, &request.message_id)
        .await
    {
        // 回退编排失败（如消息不存在）：透传错误，不执行截断
        return Err(e.into());
    }

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
        "重做消息请求: 会话={}, 消息={}",
        session_id, request.message_id
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
    let summary = agent.compress_session(&session_id).await?;
    let message = summary
        .as_ref()
        .map(crate::api::shared::types::ChatMessage::from_structured_light);
    Ok(Json(CompressSessionResponse {
        compressed: message.is_some(),
        message,
    }))
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
