//! 知识领域模块
//!
//! 本模块提供知识管理功能，包括：
//! - 文件摄入和处理
//! - 摄入任务状态跟踪
//! - 文档检索
//! - 检索建议

/// 知识库请求处理函数
pub mod handlers;
/// 知识库路由定义
pub mod routes;
/// 知识库业务逻辑
pub mod services;
/// 知识库类型定义
pub mod types;

pub use routes::routes;
pub use types::{
    FileIngestResult, IngestRequest, IngestResponse, IngestStatusResponse, SearchQuery,
    SearchResponse, SearchResult, SearchResultMetadata, SearchSuggestionsResponse,
};
