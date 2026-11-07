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
        .route(
            "/scheduled-tasks/{id}",
            // PUT 与 PATCH 同一个处理器：语义是「只改传了的字段」（见 handler 文档）
            delete(handlers::delete_task)
                .put(handlers::update_task)
                .patch(handlers::update_task),
        )
}
