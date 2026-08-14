use std::sync::Arc;

use axum::{
    routing::{get, post, put},
    Router,
};

use crate::api::sessions::handlers::{
    compress_session, delete_message, delete_session, get_session, get_session_messages,
    list_sessions, redo_message, update_session_title, update_session_workspace,
};
use crate::state::AppState;

/// 创建会话路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/messages", get(get_session_messages))
        .route("/sessions/{id}/messages/delete", post(delete_message))
        .route("/sessions/{id}/messages/redo", post(redo_message))
        .route("/sessions/{id}/title", post(update_session_title))
        .route("/sessions/{id}/workspace", put(update_session_workspace))
        .route("/sessions/{id}/compress", post(compress_session))
}
