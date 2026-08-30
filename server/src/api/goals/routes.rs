//! 目标路由。

use std::sync::Arc;

use axum::{
    routing::{get, patch},
    Router,
};

use crate::state::AppState;

use super::handlers::{create_goal, delete_goal, list_goals, update_goal};

/// 目标路由（挂载于 `/api/v1` 前缀下）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/goals", get(list_goals).post(create_goal))
        .route("/goals/{id}", patch(update_goal).delete(delete_goal))
}
