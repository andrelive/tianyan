use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::knowledge::handlers::{
    get_ingest_status_handler, ingest_handler, list_entries_handler, read_entry_handler,
    search_handler, search_suggestions_handler,
};
use crate::state::AppState;

/// 创建知识路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/knowledge/ingest", post(ingest_handler))
        .route(
            "/knowledge/ingest/{job_id}/status",
            get(get_ingest_status_handler),
        )
        .route("/knowledge/search", get(search_handler))
        .route(
            "/knowledge/search/suggestions",
            get(search_suggestions_handler),
        )
        // 只读浏览：列表 + 读内容（不提供写操作，知识条目通过 ingest 管理）
        .route("/knowledge/entries", get(list_entries_handler))
        .route("/knowledge/entries/read", get(read_entry_handler))
}
