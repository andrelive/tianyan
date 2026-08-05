//! 内部状态视图路由。

use std::sync::Arc;

use axum::{routing::get, Router};

use crate::state::AppState;

use super::handlers::{get_stats_handler, list_memories_handler, list_traces_handler};

/// 构建内部状态视图路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory", get(list_memories_handler))
        .route("/stats", get(get_stats_handler))
        .route("/retrieval/traces", get(list_traces_handler))
}
