//! 待办清单路由。

use std::sync::Arc;

use axum::{
    routing::{get, patch},
    Router,
};

use crate::state::AppState;

use super::handlers::{create_todo, delete_todo, list_todos, update_todo};

/// 待办清单路由（挂载于 `/api/v1` 前缀下）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/todos", get(list_todos).post(create_todo))
        .route("/todos/{id}", patch(update_todo).delete(delete_todo))
}
