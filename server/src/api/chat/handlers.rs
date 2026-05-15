use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::{Json, State},
    response::sse::{Event, Sse},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info};

use crate::api::chat::services::ChatService;
use crate::api::chat::types::{
    ChatRequest, ChatResponse, ChatStreamEvent, EditMessageRequest, RegenerateRequest,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

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

/// 对话完成处理器（流式/SSE via POST）
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(100);

    if let Err(e) = request.validate() {
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let _ = tx_clone
                .send(Ok(Event::default().data(format!("{{\"error\":\"{}\"}}", e))))
                .await;
        });
        return Sse::new(ReceiverStream::new(rx));
    }

    let session_id = request
        .session_id
        .as_ref()
        .map(|s| s.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    info!(
        "流式对话请求: 会话={}, 消息数={}",
        session_id,
        request.messages.len()
    );

    let (event_tx, mut event_rx) = mpsc::channel::<ChatStreamEvent>(100);

    let agent = state.agent().await;
    let session_manager = state.session_manager();

    // 生成任务处理流式响应
    tokio::spawn(async move {
        let service = ChatService::new(agent, session_manager);

        if let Err(e) = service.process_message_stream(request, event_tx).await {
            error!("流式处理错误: {}", e);
        }
    });

    // 转发事件到 SSE
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                let sse_event = Event::default().data(json);
                if tx_clone.send(Ok(sse_event)).await.is_err() {
                    break;
                }
            }
        }

        // 发送流结束标记
        let _ = tx_clone.send(Ok(Event::default().data("[DONE]"))).await;
    });

    Sse::new(ReceiverStream::new(rx))
}

/// 重新生成消息处理器
pub async fn regenerate_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RegenerateRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!(
        "重新生成消息: 会话={}, 索引={}",
        request.session_id, request.message_index
    );

    let agent = state.agent().await;
    let session_manager = state.session_manager();

    let service = ChatService::new(agent, session_manager);

    service
        .regenerate_message(request)
        .await
        .map(Json)
        .map_err(|e| {
            error!("重新生成失败: {}", e);
            ApiError::Internal(format!("重新生成失败: {}", e))
        })
}

/// 编辑消息处理器
pub async fn edit_message_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EditMessageRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!(
        "编辑消息: 会话={}, 索引={}",
        request.session_id, request.message_index
    );

    let agent = state.agent().await;
    let session_manager = state.session_manager();

    let service = ChatService::new(agent, session_manager);

    service.edit_message(request).await.map(Json).map_err(|e| {
        error!("编辑消息失败: {}", e);
        ApiError::Internal(format!("编辑消息失败: {}", e))
    })
}
