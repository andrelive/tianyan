//! 后台任务路由。

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

use super::handlers::{cancel_task, list_tasks, stream_tasks};

/// 后台任务相关路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/tasks/stream", get(stream_tasks))
        .route("/tasks/{id}/cancel", post(cancel_task))
}
