//! 后台任务路由。

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use crate::state::AppState;

use super::handlers::list_tasks;

/// 后台任务相关路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/tasks", get(list_tasks))
}
