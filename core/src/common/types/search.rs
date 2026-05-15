use serde::{Deserialize, Serialize};

use super::content::ContentLevel;
use super::uri::TianyanUri;

/// 带相关性评分的搜索结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 找到的条目的 URI
    pub uri: TianyanUri,
    /// 相关性评分（0.0 - 1.0）
    pub score: f32,
    /// 匹配的内容层级
    pub matched_level: ContentLevel,
    /// 匹配层级的内容
    pub content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::namespace::ContextNamespace;

    #[test]
    fn test_search_result() {
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["test".to_string()]);
        let result = SearchResult {
            uri: uri.clone(),
            score: 0.85,
            matched_level: ContentLevel::Abstract,
            content: Some("Test content".to_string()),
        };
        assert_eq!(result.uri, uri);
        assert!((result.score - 0.85).abs() < 0.001);
    }
}
