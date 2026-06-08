use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::knowledge::handlers::{
    ingest_handler, search_handler, search_suggestions_handler,
};
use crate::state::AppState;

/// 创建知识路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ingest", post(ingest_handler))
        .route("/search", get(search_handler))
        .route("/search/suggestions", get(search_suggestions_handler))
}
