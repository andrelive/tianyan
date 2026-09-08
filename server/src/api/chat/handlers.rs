use std::sync::Arc;

use axum::{
    extract::{Json, Path, State},
    Json as JsonResponse,
};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::api::chat::services::ChatService;
use crate::api::chat::types::{AnswerRequest, ChatRequest, ChatStreamEvent};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 流式事件转发通道缓冲区大小（chunk → 统一事件通道）。
const SSE_CHANNEL_BUFFER: usize = 100;

/// 追问回答提交处理器（ask_user 同步工具：回答提交到等待通道，工具执行恢复）。
///
/// 与流式对话共用同一 SSE 流：工具执行挂起期间流保持打开，回答提交后
/// 工具结果经原流推送（模型看到 调用→结果 对，继续决策）。
pub async fn chat_answer_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AnswerRequest>,
) -> Json<serde_json::Value> {
    let submitted = state
        .user_questions()
        .submit(&request.session_id, request.answers)
        .await;
    if submitted {
        info!("追问回答已提交: 会话={}", request.session_id);
        Json(serde_json::json!({ "status": "ok" }))
    } else {
        // 无等待中的追问：幂等空操作（前端在无追问时提交不报错）
        debug!("追问回答提交时无等待通道: 会话={}", request.session_id);
        Json(serde_json::json!({ "status": "ok", "no_pending": true }))
    }
}

/// 对话启动处理器（ADR-028 第 3 步：收敛为开关）。
///
/// 校验 + 启动 AgentLoop + 立即返回（不再承载 SSE 响应流）。
/// 输出全部经 `GET /events` 常驻流下发：循环内把 AgentStreamChunk
/// 映射为 ChatStreamEvent JSON 转发到统一事件通道（task_event_tx），
/// 前端经 EventSource 接收（useUnifiedEvents 按 session_id 路由）。
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Result<JsonResponse<serde_json::Value>, ApiError> {
    request.validate().map_err(ApiError::BadRequest)?;

    // 确保 request 携带 session_id（前端可能不传，此时生成新的）
    let mut request = request;
    let session_id = request
        .session_id
        .get_or_insert_with(|| format!("session-{}", uuid::Uuid::new_v4()))
        .clone();

    info!(
        "对话启动: 会话={}, 消息长度={}",
        session_id,
        request.message.content.as_deref().unwrap_or_default().len()
    );

    let agent = state.agent().await;
    let session_manager = state.session_manager();
    let skill_sync = state.skill_sync();
    let role_sync = state.role_sync();
    let config_arc = state.config();
    let config_guard = config_arc.read().await;
    let context_window = crate::state::resolve_chat_model_spec(&config_guard).context_length as u64;
    drop(config_guard);
    // 取消标志：服务关停时置位；客户端断开不置位（跑完再取——本地助手后台
    // 任务不应因 SSE 断线而中断），主动「停止」经 stream_cancels 端点显式触发。
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // 注册到流取消注册表（供显式「停止」端点触发）；agent 跑完后移除
    let cancels_registry = state.stream_cancels();
    cancels_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.clone(), cancel.clone());
    let session_id_for_cleanup = session_id.clone();

    // 统一事件通道（GET /events 广播源）
    let event_tx = state.task_event_tx.clone();

    tokio::spawn(async move {
        let service = ChatService::new(agent, session_manager)
            .with_skill_sync(skill_sync)
            .with_role_sync(role_sync)
            .with_context_window(context_window);

        // 流式事件转发：ChatStreamEvent → JSON（带 type=chat_stream）→ 统一事件通道。
        // 转发协程随 process_message_stream 结束（tx drop → rx 关闭）退出。
        let (event_tx_local, mut event_rx) = mpsc::channel::<ChatStreamEvent>(SSE_CHANNEL_BUFFER);
        let broadcast_tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let Ok(mut payload) = serde_json::to_value(&event) {
                    payload["type"] = serde_json::json!("chat_stream");
                    if let Ok(json) = serde_json::to_string(&payload) {
                        let _ = broadcast_tx.send(json);
                    }
                }
            }
        });

        // 捕获会话 id（request 随后被 move 进 process_message_stream）
        let request_session_id = request.session_id.clone().unwrap_or_default();
        if let Err(e) = service
            .process_message_stream(request, event_tx_local, cancel)
            .await
        {
            error!("对话处理错误: {}", e);
            // 运行时错误（模型未配置 / provider 挂 / 会话失效等）广播 error 事件：
            // 前端清理占位消息并提示。
            let event = ChatStreamEvent::error(
                "chatcmpl-runtime-error",
                &request_session_id,
                format!("对话处理失败: {e}"),
            );
            if let Ok(mut payload) = serde_json::to_value(&event) {
                payload["type"] = serde_json::json!("chat_stream");
                if let Ok(json) = serde_json::to_string(&payload) {
                    let _ = event_tx.send(json);
                }
            }
        }
        // 循环结束：从取消注册表移除（无论正常完成 / 显式取消 / 错误）
        cancels_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id_for_cleanup);
    });

    Ok(JsonResponse(serde_json::json!({
        "status": "started",
        "session_id": session_id,
    })))
}

/// 显式取消进行中的对话流（「停止」按钮）：置位该会话的 cancel 标志。
/// 跑完再取语义下，客户端断开不取消；用户主动停止经此端点显式取消。
pub async fn chat_stream_cancel_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let flag = state
        .stream_cancels()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&session_id)
        .cloned();
    match flag {
        Some(f) => {
            f.store(true, std::sync::atomic::Ordering::Relaxed);
            Json(serde_json::json!({ "status": "cancelling", "session_id": session_id }))
        }
        None => Json(serde_json::json!({ "status": "no_active_stream", "session_id": session_id })),
    }
}
