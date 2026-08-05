//! 内部状态视图路由。

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::state::AppState;

use super::handlers::{
    get_approval_status_handler, get_scheduler_status_handler, get_stats_handler,
    list_memories_handler, list_traces_handler, respond_approval_handler,
};

/// 构建内部状态视图路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory", get(list_memories_handler))
        .route("/stats", get(get_stats_handler))
        .route("/retrieval/traces", get(list_traces_handler))
        .route("/scheduler/status", get(get_scheduler_status_handler))
        .route("/approval/status", get(get_approval_status_handler))
        .route("/approval/respond", post(respond_approval_handler))
}
