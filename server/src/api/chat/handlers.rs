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
use crate::api::chat::types::{ChatRequest, ChatResponse, ChatStreamEvent, ClarifyRequest};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// SSE 流式通道缓冲区大小。
const SSE_CHANNEL_BUFFER: usize = 100;

/// 对话完成处理器（非流式）
pub async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!("收到对话请求，会话: {:?}", request.session_id);
    debug!("消息数量: {}", request.messages.len());

    let agent = state.agent().await;
    let session_manager = state.session_manager();

    let service = ChatService::new(agent, session_manager);

    service
        .process_message(request)
        .await
        .map(Json)
        .map_err(|e| {
            error!("对话处理错误: {}", e);
            ApiError::Internal(format!("对话处理错误: {}", e))
        })
}

/// 追问回答处理器（非流式）
pub async fn chat_clarify_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ClarifyRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!("收到追问回答请求，会话: {}", request.session_id);

    let agent = state.agent().await;
    let session_manager = state.session_manager();

    let service = ChatService::new(agent, session_manager);
    service
        .handle_clarification(&request.session_id, &request.answer)
        .await
        .map(Json)
        .map_err(|e| {
            error!("追问回答处理错误: {}", e);
            ApiError::Internal(format!("追问回答处理错误: {}", e))
        })
}

/// 对话完成处理器（流式/SSE via POST）
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(SSE_CHANNEL_BUFFER);

    if let Err(e) = request.validate() {
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            // 以标准 ChatStreamEvent 发送错误（chunk_type=error），
            // 前端据此展示错误并清理占位消息。
            let event = ChatStreamEvent {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: String::new(),
                delta: e,
                finish_reason: None,
                chunk_type: tianyan::agent::StreamChunkType::Error,
                skill_calls: None,
            };
            if let Ok(json) = serde_json::to_string(&event) {
                if tx_clone
                    .send(Ok(Event::default().data(json)))
                    .await
                    .is_err()
                {
                    debug!("SSE 客户端已断开，错误事件未送达");
                }
            }
        });
        return Sse::new(ReceiverStream::new(rx));
    }

    // 确保 request 携带 session_id（前端可能不传，此时生成新的）
    let mut request = request;
    let session_id = request
        .session_id
        .get_or_insert_with(|| format!("session-{}", uuid::Uuid::new_v4()));

    info!(
        "流式对话请求: 会话={}, 消息数={}",
        session_id,
        request.messages.len()
    );

    let (event_tx, mut event_rx) = mpsc::channel::<ChatStreamEvent>(SSE_CHANNEL_BUFFER);

    let agent = state.agent().await;
    let session_manager = state.session_manager();

    // 生成任务处理流式响应
    tokio::spawn(async move {
        let service = ChatService::new(agent, session_manager);

        if let Err(e) = service.process_message_stream(request, event_tx).await {
            error!("流式处理错误: {}", e);
        }
    });

    // 转发事件到 SSE，同时发送 15 秒间隔心跳防止连接超时
    let tx_clone = tx.clone();
    let ping_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if tx_clone.send(Ok(Event::default().data(json))).await.is_err() {
                                    break;
                                }
                            }
                            // finish_reason 非空表示流结束
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
                _ = interval.tick() => {
                    if ping_tx.send(Ok(Event::default().data(":ping"))).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
}
