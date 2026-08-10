//! 结构化 Trace 路由（G6）：`GET /api/v1/traces`。

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use crate::state::AppState;

use super::handlers::query_traces;

/// Trace 回放查询路由。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/traces", get(query_traces))
}
