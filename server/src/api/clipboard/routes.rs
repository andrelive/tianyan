//! 剪贴板 I/O 路由。

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

use super::handlers::{capture_handler, outbox_handler, pending_handler, respond_handler};

/// 剪贴板相关路由（挂载于 `/api/v1` 前缀下）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/clipboard/capture", post(capture_handler))
        .route("/clipboard/respond", post(respond_handler))
        .route("/clipboard/pending", get(pending_handler))
        .route("/clipboard/outbox", get(outbox_handler))
}
