use std::sync::Arc;

use axum::{routing::get, Router};

use crate::api::tools::handlers::list_tools;
use crate::state::AppState;

/// 创建工具路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/tools", get(list_tools))
}
