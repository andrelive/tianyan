//! 检索结果类型定义。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::common::types::{ContentLevel, TianyanUri};
use crate::context::compression::estimate_tokens;

// 检索追踪类型（RetrievalStep / RetrievalStepType / RetrievalTrace）已下沉到
// `common::types::retrieval_trace`（跨模块共享的持久化契约），此处仅重新导出
// 以保持既有引用路径 `crate::context::retrieval::types::*` 不变。
pub use crate::common::types::retrieval_trace::{RetrievalStep, RetrievalStepType, RetrievalTrace};

/// 带有内容的检索结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    /// 结果 URI。
    pub uri: TianyanUri,
    /// 相关性分数。
    pub score: f32,
    /// 已加载的内容（如果可用）。
    pub content: Option<String>,
    /// 加载的内容层级。
    pub content_level: ContentLevel,
    /// 内容的 Token 数量。
    pub token_count: usize,
    /// 结果的类别。
    pub category: String,
    /// 结果关联的标签。
    pub tags: Vec<String>,
    /// 源内容最后更新时间（用于新鲜度计算）。
    pub source_updated_at: Option<DateTime<Utc>>,
    /// 新鲜度评分（0.0~1.0，1.0 为最新）。
    pub freshness_score: f32,
}

impl RetrievalResult {
    /// 创建新的检索结果。
    pub fn new(uri: TianyanUri, score: f32) -> Self {
        Self {
            uri,
            score,
            content: None,
            content_level: ContentLevel::Abstract,
            token_count: 0,
            category: String::new(),
            tags: Vec::new(),
            source_updated_at: None,
            freshness_score: 1.0,
        }
    }

    /// 检查是否已加载内容。
    pub fn has_content(&self) -> bool {
        self.content.is_some()
    }

    /// 设置内容。
    pub fn with_content(mut self, content: String, level: ContentLevel) -> Self {
        self.token_count = estimate_tokens(&content);
        self.content = Some(content);
        self.content_level = level;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retrieval_result() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let result = RetrievalResult::new(uri.clone(), 0.9);
        assert_eq!(result.uri, uri);
        assert_eq!(result.score, 0.9);
        assert!(!result.has_content());
    }

    #[test]
    fn test_retrieval_result_with_content() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let result =
            RetrievalResult::new(uri, 0.9).with_content("Test".to_string(), ContentLevel::Overview);
        assert!(result.has_content());
        assert!(result.token_count > 0);
    }

    #[test]
    fn test_retrieval_trace() {
        let mut trace = RetrievalTrace::new("test query");
        let step = RetrievalStep {
            step_type: RetrievalStepType::L0Search,
            target_uri: TianyanUri::parse("tianyan://user/profile").unwrap(),
            score: Some(0.9),
            tokens_used: 100,
            timestamp: Utc::now(),
        };
        trace.add_step(step);
        assert_eq!(trace.total_tokens, 100);
    }
}
