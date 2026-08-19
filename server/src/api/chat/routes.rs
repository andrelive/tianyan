use std::sync::Arc;

use axum::{routing::post, Router};

use crate::api::chat::handlers::{
    chat_clarify_handler, chat_clarify_stream_handler, chat_stream_handler,
};
use crate::state::AppState;

/// 创建对话路由（仅流式：/chat/stream 与追问流式；非流式 /chat 已移除——
/// GUI 只用 SSE，避免维护两份解析逻辑）
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/chat/stream", post(chat_stream_handler))
        .route("/chat/clarify", post(chat_clarify_handler))
        .route("/chat/clarify/stream", post(chat_clarify_stream_handler))
}
