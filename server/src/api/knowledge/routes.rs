use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::knowledge::handlers::{
    delete_entry_handler, ingest_handler, list_entries_handler, read_entry_handler, search_handler,
    search_suggestions_handler,
};
use crate::state::AppState;

/// 创建知识路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/knowledge/ingest", post(ingest_handler))
        .route("/knowledge/search", get(search_handler))
        .route(
            "/knowledge/search/suggestions",
            get(search_suggestions_handler),
        )
        // 浏览：列表 + 读内容 + 删除（删除是用户主动管理，仅限 knowledge 命名空间）
        .route("/knowledge/entries", get(list_entries_handler))
        .route("/knowledge/entries/read", get(read_entry_handler))
        .route("/knowledge/entries/delete", post(delete_entry_handler))
}
