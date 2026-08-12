use serde::{Deserialize, Serialize};

use super::uri::TianyanUri;

/// 带相关性评分的搜索结果。
///
/// 向量搜索不携带内容（恒为 `None` 的死字段已移除），调用方按需加载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 找到的条目的 URI
    pub uri: TianyanUri,
    /// 相关性评分（0.0 - 1.0）
    pub score: f32,
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
        };
        assert_eq!(result.uri, uri);
        assert!((result.score - 0.85).abs() < 0.001);
    }
}
