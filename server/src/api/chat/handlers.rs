use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Json, Path, State},
    response::sse::{Event, Sse},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info};

use crate::api::chat::services::ChatService;
use crate::api::chat::types::{AnswerRequest, ChatRequest, ChatStreamEvent};
use crate::state::AppState;

/// SSE 流式通道缓冲区大小。
const SSE_CHANNEL_BUFFER: usize = 100;

/// 校验失败 → 以标准 ChatStreamEvent（chunk_type=error）推送到 SSE 通道；
/// 前端据此展示错误并清理占位消息。客户端断开时静默（debug 日志）。
fn send_validation_error(tx: mpsc::Sender<Result<Event, Infallible>>, message: String) {
    tokio::spawn(async move {
        let id = uuid::Uuid::new_v4().to_string();
        let event = ChatStreamEvent::error(&id, "", message);
        if let Ok(json) = serde_json::to_string(&event) {
            if tx.send(Ok(Event::default().data(json))).await.is_err() {
                debug!("SSE 客户端已断开，错误事件未送达");
            }
        }
    });
}

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

/// 对话完成处理器（流式/SSE via POST）
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(SSE_CHANNEL_BUFFER);

    if let Err(e) = request.validate() {
        send_validation_error(tx.clone(), e);
        return Sse::new(ReceiverStream::new(rx));
    }

    // 确保 request 携带 session_id（前端可能不传，此时生成新的）
    let mut request = request;
    let session_id = request
        .session_id
        .get_or_insert_with(|| format!("session-{}", uuid::Uuid::new_v4()));

    info!(
        "流式对话请求: 会话={}, 消息长度={}",
        session_id,
        request.message.content.len()
    );

    let (event_tx, event_rx) = mpsc::channel::<ChatStreamEvent>(SSE_CHANNEL_BUFFER);

    let agent = state.agent().await;
    let session_manager = state.session_manager();
    // 取消标志：服务关停时置位；客户端断开不置位（跑完再取——本地助手后台
    // 任务不应因 SSE 断线而中断），主动「停止」经 stream_cancels 端点显式触发。
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_flag = state.shutdown_flag();
    // 注册到流取消注册表（供显式「停止」端点触发）；agent 跑完后移除
    let cancels_registry = state.stream_cancels();
    cancels_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.clone(), cancel.clone());
    let session_id_for_cleanup = session_id.clone();

    // 生成任务处理流式响应
    let cancel_for_service = cancel.clone();
    let skill_sync = state.skill_sync();
    let role_sync = state.role_sync();
    let config_arc = state.config();
    let config_guard = config_arc.read().await;
    let context_window = crate::state::resolve_chat_model_spec(&config_guard).context_length as u64;
    drop(config_guard);
    tokio::spawn(async move {
        let service = ChatService::new(agent, session_manager)
            .with_skill_sync(skill_sync)
            .with_role_sync(role_sync)
            .with_context_window(context_window);

        // 捕获会话 id（request 随后被 move 进 process_message_stream）
        let request_session_id = request.session_id.clone().unwrap_or_default();
        if let Err(e) = service
            .process_message_stream(request, event_tx.clone(), cancel_for_service)
            .await
        {
            error!("流式处理错误: {}", e);
            // 运行时错误（模型未配置 / provider 挂 / 会话失效等）发 error 事件：
            // 否则 SSE 干净结束、前端留空 assistant 气泡且无任何提示。
            let _ = event_tx
                .send(ChatStreamEvent::error(
                    "chatcmpl-runtime-error",
                    &request_session_id,
                    format!("对话处理失败: {e}"),
                ))
                .await;
        }
        // 流结束：从取消注册表移除（无论正常完成 / 显式取消 / 错误）
        cancels_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id_for_cleanup);
    });

    spawn_sse_forwarder(event_rx, tx, cancel, shutdown_flag);

    Sse::new(ReceiverStream::new(rx))
}

/// 把事件通道转发为 SSE 输出（共享：chat/stream 与 chat/clarify/stream）。
///
/// - 15 秒间隔心跳（`:ping` 注释事件）防止连接超时；
/// - 客户端断开或服务关停时置位取消标志，停止后台 AgentLoop；
/// - `finish_reason` 非空表示流结束，追加 `[DONE]`。
fn spawn_sse_forwarder(
    mut event_rx: mpsc::Receiver<ChatStreamEvent>,
    tx: mpsc::Sender<Result<Event, Infallible>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
) {
    let tx_clone = tx.clone();
    let ping_tx = tx.clone();
    let cancel_for_sse = cancel.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        let mut shutdown_poll = tokio::time::interval(Duration::from_secs(1));
        // 客户端断开后进入排空：不再发送，但仍消费 event_rx 直到 finish_reason，
        // 让 agent 跑完并持久化结果（跑完再取——本地助手后台任务不因断线中断）。
        // 主动「停止」经 stream_cancels 端点置位 cancel（不是断线）。
        let mut draining = false;
        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if !draining {
                                // SSE 事件带 id（流 stream_id）：Last-Event-ID 恢复的基础
                                let event_id = event.id.clone();
                                if let Ok(json) = serde_json::to_string(&event) {
                                    let sse_event = Event::default().id(event_id).data(json);
                                    if tx_clone.send(Ok(sse_event)).await.is_err() {
                                        // 客户端断开：进入排空（不取消 agent，跑完再取）
                                        draining = true;
                                    }
                                }
                            }
                            if event.finish_reason.is_some() {
                                if !draining {
                                    let _ = tx_clone
                                        .send(Ok(Event::default().data("[DONE]")))
                                        .await;
                                }
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = shutdown_poll.tick() => {
                    if shutdown_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        cancel_for_sse.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                }
                _ = interval.tick() => {
                    if !draining && ping_tx.send(Ok(Event::default().data(":ping"))).await.is_err() {
                        // 客户端断开：进入排空（不取消 agent，跑完再取）
                        draining = true;
                    }
                }
            }
        }
    });
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
