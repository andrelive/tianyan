//! 事件路由（挂载于 `/api/v1` 前缀下）。
//!
//! handler 经 `AppState` 直读事件总线与 webhook 令牌（见 [`super::handlers`]），
//! 路由为常规 `Router<Arc<AppState>>`，直接 merge 进主路由。

use std::sync::Arc;

use axum::routing::post;
use axum::Router;

use crate::state::AppState;

use super::handlers::receive_webhook;

/// 事件 API 路由（相对路径，挂载于 `/api/v1` 前缀下）。
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/events", post(receive_webhook))
}
