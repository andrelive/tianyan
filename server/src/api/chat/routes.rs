use std::sync::Arc;

use axum::{routing::post, Router};

use crate::api::chat::handlers::{
    chat_answer_handler, chat_stream_cancel_handler, chat_stream_handler,
};
use crate::state::AppState;

/// 创建对话路由（仅流式：/chat/stream；追问回答提交 /chat/answer——
/// ask_user 同步工具的回答提交入口，工具执行挂起等待，回答经此恢复）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/chat/stream", post(chat_stream_handler))
        .route("/chat/answer", post(chat_answer_handler))
        // 显式「停止」：置位该会话流取消标志（跑完再取语义下断线不取消，仅主动停止）
        .route(
            "/chat/streams/{session_id}/cancel",
            post(chat_stream_cancel_handler),
        )
}
