use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::sessions::handlers::{
    delete_session, get_session, get_session_messages, list_sessions, update_session_title,
};
use crate::state::AppState;

/// 创建会话路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/messages", get(get_session_messages))
        .route("/sessions/{id}/title", post(update_session_title))
}
