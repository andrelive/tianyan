use std::sync::Arc;

use axum::{routing::post, Router};

use crate::api::chat::handlers::{
    chat_handler, chat_stream_handler, edit_message_handler, regenerate_handler,
};
use crate::state::AppState;

/// 创建对话路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/chat", post(chat_handler))
        .route("/chat/stream", post(chat_stream_handler))
        .route("/chat/regenerate", post(regenerate_handler))
        .route("/chat/edit", post(edit_message_handler))
}
