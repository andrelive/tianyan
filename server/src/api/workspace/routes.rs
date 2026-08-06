//! 工作区路由定义

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use crate::api::workspace::handlers::{diff_handler, read_handler, tree_handler};
use crate::state::AppState;

/// 创建工作区路由（挂载于 `/api/v1` 前缀下）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workspace/tree", get(tree_handler))
        .route("/workspace/read", get(read_handler))
        .route("/workspace/diff", get(diff_handler))
}
