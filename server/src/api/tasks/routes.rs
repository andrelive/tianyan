//! 后台任务路由。

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

use super::handlers::{cancel_task, get_task_log, list_tasks, stream_tasks, subscribe_events};

/// 后台任务相关路由。
///
/// `/events` 为统一事件流（ADR-028）：全局单连接，事件带 type 区分
/// （message / task_status / command_output / 子代理消息），前端按
/// session_id / task_id 路由；旧路径 `/tasks/stream` 已删除
/// （ADR-028/030：并入统一流）。
/// `/events/subscribe` 为事件订阅（ADR-029）：快照恢复的触发点。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/events", get(stream_tasks))
        .route("/events/subscribe", post(subscribe_events))
        .route("/tasks/{id}/cancel", post(cancel_task))
        .route("/tasks/{id}/log", get(get_task_log))
}
