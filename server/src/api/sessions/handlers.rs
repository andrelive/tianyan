use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use tracing::{error, info};

use crate::api::sessions::services::SessionService;
use crate::api::sessions::types::{
    CompressSessionResponse, DeleteMessageRequest, DeleteMessageResponse, DeleteSessionResponse,
    ListSessionsResponse, RedoRequest, RedoResponse, Session, SessionDetail, SessionMessagesQuery,
    SessionMessagesResponse, UpdateTitleRequest, UpdateWorkspaceRequest,
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
        Some(state.working_sets()),
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
        Some(state.working_sets()),
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
        Some(state.working_sets()),
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
        Some(state.working_sets()),
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

/// 接管流后等待「会话静默」的上限（超时即放弃命令：避免与仍在写会话的旧轮竞态）。
const OP_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// 同 opId 命令的幂等等待上限（超时 → 409 `same_operation_in_flight`）。
const OP_INFLIGHT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// 控制面命令统一门（ADR-041）：幂等（`opId`）+ 互斥（会话租约）。
enum OpGate {
    /// 首次执行：执行完 `finish_op`；失败 `abort_op`。
    Execute {
        token: crate::session_leases::LeaseToken,
    },
    /// 幂等重放：既有结果（不重复执行）。
    Replay(serde_json::Value),
}

/// 接入控制面命令：忙 → 409（结构化 reason）；接管流 → 等静默后再执行；
/// 同 `opId` 重复 → 重放既有结果（等待进行中的完成后返回）。
async fn gate_control_op(
    state: &Arc<AppState>,
    session_id: &str,
    operation_id: Option<&str>,
) -> Result<OpGate, ApiError> {
    use crate::session_leases::{BusyReason, OpAccess};
    let leases = state.session_leases();
    match leases
        .acquire_op(session_id, operation_id)
        .map_err(ApiError::Busy)?
    {
        OpAccess::Replay { outcome } => Ok(OpGate::Replay(outcome)),
        OpAccess::InFlight { quiet } => {
            let op_id = operation_id.ok_or(ApiError::Busy(BusyReason::SameOperationInFlight))?;
            if !leases
                .wait_inflight(session_id, op_id, quiet, OP_INFLIGHT_WAIT)
                .await
            {
                return Err(ApiError::Busy(BusyReason::SameOperationInFlight));
            }
            let outcome = leases
                .op_outcome(session_id, op_id)
                .ok_or(ApiError::Busy(BusyReason::SameOperationInFlight))?;
            Ok(OpGate::Replay(outcome))
        }
        OpAccess::Owned { token, drain } => {
            if let Some(quiet) = drain {
                // 接管了正在跑的流（取消标志已置位）：等它**真正退出**再动手——
                // 回退/重做事务与轮写入互斥；超时放弃（宁可 409，也不与仍在写
                // 会话的旧轮竞态）。
                if !leases.wait_quiet(session_id, quiet, OP_DRAIN_TIMEOUT).await {
                    leases.abort_op(session_id, token);
                    return Err(ApiError::Busy(BusyReason::StreamInProgress));
                }
            }
            Ok(OpGate::Execute { token })
        }
    }
}

/// 删除消息（该消息及其后的所有消息）——回退（ADR-040）。
///
/// handler 是薄壳：置位取消标志 → 调用 core 的完整回退事务（取消轮与任务 →
/// 保存重做数据 → 恢复工作区文件 → 截断时序链，一把会话锁内完成）→ 映射响应。
/// 数据截断**不再**由本层编排：此前「读链 → 改链 → 写链」在锁外分三段执行，
/// 窗口内新轮落库的消息会被整表重写覆盖（库为权威 → 不可自愈）。
pub async fn delete_message(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(request): Json<DeleteMessageRequest>,
) -> Result<Json<DeleteMessageResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!(
        "删除消息请求: 会话={}, 消息={}",
        session_id, request.message_id
    );

    // 控制面门（ADR-041）：opId 幂等（双击/重试不二次截断）+ 会话租约互斥
    // （有流在跑 → 接管：置位其取消标志、挡住新流、等旧轮真正退出再执行）。
    let token = match gate_control_op(&state, &session_id, request.operation_id.as_deref()).await? {
        OpGate::Replay(outcome) => {
            // 同 opId 的重复请求：重放既有回退结果（不重复执行）
            let rollback: tianyan::agent::RollbackOutcome =
                serde_json::from_value(outcome).unwrap_or_default();
            let service = SessionService::new(
                state.session_manager(),
                state.snapshot_manager(),
                Some(state.working_sets()),
            );
            let messages = service.get_messages(&session_id).await?;
            return Ok(Json(DeleteMessageResponse { messages, rollback }));
        }
        OpGate::Execute { token } => token,
    };

    // 完整回退事务（core，ADR-040）：文件先、链后——失败即整体中止（全或无）
    let agent = state.agent().await;
    let rollback = match agent.rollback_to(&session_id, &request.message_id).await {
        Ok(outcome) => outcome,
        Err(e) => {
            state.session_leases().abort_op(&session_id, token);
            return Err(e.into());
        }
    };
    // 幂等台账：记录结果（同 opId 重试直接重放）
    state.session_leases().finish_op(
        &session_id,
        token,
        serde_json::to_value(&rollback).unwrap_or(serde_json::Value::Null),
    );

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        Some(state.working_sets()),
    );
    let messages = service.get_messages(&session_id).await?;
    Ok(Json(DeleteMessageResponse { messages, rollback }))
}

/// 重做被回退的消息与工作区文件（与回退同一排他事务入口，ADR-040）。
pub async fn redo_message(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(request): Json<RedoRequest>,
) -> Result<Json<RedoResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("会话ID不能为空".to_string()));
    }

    info!(
        "重做消息请求: 会话={}, 消息={}",
        session_id, request.message_id
    );

    // 控制面门（ADR-041）：与回退/压缩互斥 + opId 幂等（重试不重复恢复文件）
    let token = match gate_control_op(&state, &session_id, request.operation_id.as_deref()).await? {
        OpGate::Replay(outcome) => {
            let redo: tianyan::agent::RedoOutcome =
                serde_json::from_value(outcome).unwrap_or_default();
            let service = SessionService::new(
                state.session_manager(),
                state.snapshot_manager(),
                Some(state.working_sets()),
            );
            let messages = service.get_messages(&session_id).await?;
            return Ok(Json(RedoResponse { messages, redo }));
        }
        OpGate::Execute { token } => token,
    };

    let agent = state.agent().await;
    let redo = match agent.redo_to(&session_id, &request.message_id).await {
        Ok(outcome) => outcome,
        Err(e) => {
            state.session_leases().abort_op(&session_id, token);
            return Err(e.into());
        }
    };
    state.session_leases().finish_op(
        &session_id,
        token,
        serde_json::to_value(&redo).unwrap_or(serde_json::Value::Null),
    );

    let service = SessionService::new(
        state.session_manager(),
        state.snapshot_manager(),
        Some(state.working_sets()),
    );
    let messages = service.get_messages(&session_id).await?;
    Ok(Json(RedoResponse { messages, redo }))
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

    // 控制面门（ADR-041）：压缩与回退/重做互斥（接管流后等静默再执行）；
    // 无 opId（无请求体）→ 保持非幂等。
    let OpGate::Execute { token } = gate_control_op(&state, &session_id, None).await? else {
        unreachable!("无 opId 不会重放");
    };
    let agent = state.agent().await;
    // ? 传播：会话不存在时由 core 返回 not_found 语义（404），不吞成 Internal
    let summary = match agent.compress_session(&session_id).await {
        Ok(summary) => summary,
        Err(e) => {
            state.session_leases().abort_op(&session_id, token);
            return Err(e.into());
        }
    };
    let message = summary
        .as_ref()
        .map(crate::api::shared::types::ChatMessage::from_structured_light);
    let response = CompressSessionResponse {
        compressed: message.is_some(),
        message,
    };
    state.session_leases().finish_op(
        &session_id,
        token,
        serde_json::to_value(&response).unwrap_or(serde_json::Value::Null),
    );
    Ok(Json(response))
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
        Some(state.working_sets()),
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
        Some(state.working_sets()),
    );

    service
        .update_workspace(&session_id, &request.working_directory)
        .await
        .inspect_err(|e| error!("更新会话工作目录失败: {}", e))
        .map(Json)
}
