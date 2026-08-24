use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Json, State},
    response::sse::{Event, Sse},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info};

use crate::api::chat::services::ChatService;
use crate::api::chat::types::{ChatRequest, ChatStreamEvent, ClarifyRequest};
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

/// 追问回答处理器（流式/SSE via POST）。
///
/// 用户确认（如审批追问回答"允许"）后，澄清轮以流式执行：思考/工具/输出
/// 逐块到达，避免整轮等待超过 HTTP 超时（此前非流式路径在前端表现为
/// "按钮转圈后报错"）。
pub async fn chat_clarify_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ClarifyRequest>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(SSE_CHANNEL_BUFFER);

    if let Err(e) = request.validate() {
        send_validation_error(tx.clone(), e);
        return Sse::new(ReceiverStream::new(rx));
    }

    info!("流式追问回答请求: 会话={}", request.session_id);

    let (event_tx, event_rx) = mpsc::channel::<ChatStreamEvent>(SSE_CHANNEL_BUFFER);
    let agent = state.agent().await;
    let session_manager = state.session_manager();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_flag = state.shutdown_flag();

    let session_id = request.session_id.clone();
    let answer = request.answer.clone();
    let config_arc = state.config();
    let config_guard = config_arc.read().await;
    let context_window = crate::state::resolve_chat_model_spec(&config_guard).context_length as u64;
    drop(config_guard);
    tokio::spawn(async move {
        let service = ChatService::new(agent, session_manager).with_context_window(context_window);
        if let Err(e) = service
            .handle_clarification_stream(&session_id, &answer, event_tx)
            .await
        {
            error!("流式追问处理错误: {}", e);
        }
    });

    spawn_sse_forwarder(event_rx, tx, cancel, shutdown_flag);

    Sse::new(ReceiverStream::new(rx))
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
    // 取消标志：客户端断开或服务关停时置位，停止后台 AgentLoop（不再烧 token）
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_flag = state.shutdown_flag();

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

        if let Err(e) = service
            .process_message_stream(request, event_tx, cancel_for_service)
            .await
        {
            error!("流式处理错误: {}", e);
        }
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
        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if tx_clone.send(Ok(Event::default().data(json))).await.is_err() {
                                    cancel_for_sse.store(true, std::sync::atomic::Ordering::Relaxed);
                                    break;
                                }
                            }
                            if event.finish_reason.is_some() {
                                if let Err(e) = tx_clone.send(Ok(Event::default().data("[DONE]"))).await {
                                    tracing::warn!(error = %e, "SSE [DONE] 发送失败");
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
                    if ping_tx.send(Ok(Event::default().data(":ping"))).await.is_err() {
                        cancel_for_sse.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    });
}
