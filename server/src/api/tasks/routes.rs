//! 后台任务路由。

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

use super::handlers::{cancel_task, get_task_log, list_tasks, stream_tasks};

/// 后台任务相关路由。
///
/// `/events` 为统一事件流（ADR-028）：全局单连接，事件带 type 区分
/// （message / task_status / command_output / 子代理消息），前端按
/// session_id / task_id 路由。`/tasks/stream` 保留兼容。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/tasks/stream", get(stream_tasks))
        .route("/events", get(stream_tasks))
        .route("/tasks/{id}/cancel", post(cancel_task))
        .route("/tasks/{id}/log", get(get_task_log))
}
