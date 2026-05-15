//! 知识领域模块
//!
//! 本模块提供知识管理功能，包括：
//! - 文件摄入和处理
//! - 摄入任务状态跟踪
//! - 文档检索
//! - 检索建议

pub mod handlers;
pub mod routes;
pub mod services;
pub mod types;

pub use routes::routes;
pub use types::{
    FileIngestResult, IngestRequest, IngestResponse, IngestStatusResponse, SearchQuery,
    SearchResponse, SearchResult, SearchResultMetadata, SearchSuggestionsResponse,
};
