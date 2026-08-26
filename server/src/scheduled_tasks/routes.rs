//! 定时智能体任务路由。

use std::sync::Arc;

use axum::{
    routing::{delete, get},
    Router,
};

use crate::scheduled_tasks::handlers;
use crate::state::AppState;

/// 定时智能体任务路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/scheduled-tasks",
            get(handlers::list_tasks).post(handlers::create_task),
        )
        .route("/scheduled-tasks/{id}", delete(handlers::delete_task))
}
