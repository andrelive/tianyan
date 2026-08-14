//! 工作区路由定义

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::api::workspace::handlers::{
    apply_edit_handler, apply_patch_handler, diff_handler, dirs_handler, read_handler, tree_handler,
};
use crate::state::AppState;

/// 创建工作区路由（挂载于 `/api/v1` 前缀下）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workspace/tree", get(tree_handler))
        .route("/workspace/dirs", get(dirs_handler))
        .route("/workspace/read", get(read_handler))
        .route("/workspace/diff", get(diff_handler))
        .route("/workspace/apply-patch", post(apply_patch_handler))
        .route("/workspace/apply-edit", post(apply_edit_handler))
}
