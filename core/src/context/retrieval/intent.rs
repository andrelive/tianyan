//! 意图分析模块。
//!
//! 本模块实现检索系统的意图分析，包括查询向量生成和目标范围确定。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::context::compression::estimate_tokens;

/// 查询类型分类。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QueryType {
    /// 搜索查询 - 查找特定信息
    Search(String),
    /// 导航查询 - 浏览到特定位置
    Navigate(String),
    /// 检索查询 - 获取特定内容
    Retrieve(String),
}

// 构造器/查询方法由测试使用，为调试 UI 预留；生产构建豁免未使用告警。
#[cfg_attr(not(test), allow(dead_code))]
impl QueryType {
    /// 创建搜索查询。
    pub fn search(query: impl Into<String>) -> Self {
        Self::Search(query.into())
    }

    /// 创建导航查询。
    pub fn navigate(target: impl Into<String>) -> Self {
        Self::Navigate(target.into())
    }

    /// 创建检索查询。
    pub fn retrieve(uri: impl Into<String>) -> Self {
        Self::Retrieve(uri.into())
    }

    /// 获取查询文本。
    pub fn as_str(&self) -> &str {
        match self {
            QueryType::Search(q) => q,
            QueryType::Navigate(q) => q,
            QueryType::Retrieve(q) => q,
        }
    }

    /// 检查是否为搜索查询。
    pub fn is_search(&self) -> bool {
        matches!(self, QueryType::Search(_))
    }

    /// 检查是否为导航查询。
    pub fn is_navigate(&self) -> bool {
        matches!(self, QueryType::Navigate(_))
    }

    /// 检查是否为检索查询。
    pub fn is_retrieve(&self) -> bool {
        matches!(self, QueryType::Retrieve(_))
    }
}

impl std::fmt::Display for QueryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryType::Search(q) => write!(f, "Search({})", q),
            QueryType::Navigate(q) => write!(f, "Navigate({})", q),
            QueryType::Retrieve(q) => write!(f, "Retrieve({})", q),
        }
    }
}

/// 表示分析后的查询意图的结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// 原始查询文本。
    pub original_query: String,
    /// 目标范围（类别过滤）。
    pub(crate) target_scope: Option<TargetScope>,
    /// 查询类型分类。
    pub(crate) query_type: QueryType,
    /// 分析时间戳。
    pub timestamp: DateTime<Utc>,
    /// 查询 Token 数量。
    pub token_count: usize,
}

impl Intent {
    /// 从查询创建新意图。
    pub fn new(query: impl Into<String>) -> Self {
        let query = query.into();
        let token_count = estimate_tokens(&query);
        Self {
            original_query: query.clone(),
            target_scope: None,
            query_type: QueryType::Search(query),
            timestamp: Utc::now(),
            token_count,
        }
    }

    /// 设置目标范围（测试路径使用）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_scope(mut self, scope: TargetScope) -> Self {
        self.target_scope = Some(scope);
        self
    }

    /// 获取类别过滤器（如果可用）。
    pub fn category_filter(&self) -> Option<&str> {
        self.target_scope.as_ref().map(|s| s.namespace())
    }
}

/// 检索的目标范围。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TargetScope {
    /// 要搜索的类别。
    category: ContextNamespace,
    /// 可选的子类别路径。
    sub_path: Vec<String>,
    /// 可选的 URI 前缀，用于更精确的定位。
    uri_prefix: Option<String>,
}

// 构造器由测试使用，为调试 UI 预留；生产构建豁免未使用告警。
#[cfg_attr(not(test), allow(dead_code))]
impl TargetScope {
    /// 为类别创建新目标范围。
    pub fn new(category: ContextNamespace) -> Self {
        Self {
            category,
            sub_path: Vec::new(),
            uri_prefix: None,
        }
    }

    /// 创建带有子路径的目标范围。
    pub fn with_path(category: ContextNamespace, path: Vec<String>) -> Self {
        let uri_prefix = if path.is_empty() {
            None
        } else {
            // 统一走 TianyanUri::new（前缀构造与 URI 解析同一实现，不手拼）
            Some(TianyanUri::new(category, path.clone()).as_str().to_string())
        };
        Self {
            category,
            sub_path: path,
            uri_prefix,
        }
    }

    /// 从 URI 创建目标范围。
    pub fn from_uri(uri: &TianyanUri) -> Self {
        Self {
            category: uri.namespace(),
            sub_path: uri.path().to_vec(),
            uri_prefix: Some(uri.to_string()),
        }
    }

    /// 获取类别。
    pub fn namespace(&self) -> &str {
        self.category.dir_name()
    }

    /// 获取类别枚举。
    pub fn category_enum(&self) -> ContextNamespace {
        self.category
    }

    /// 获取子路径。
    pub fn sub_path(&self) -> &[String] {
        &self.sub_path
    }

    /// 获取 URI 前缀。
    pub fn uri_prefix(&self) -> Option<&str> {
        self.uri_prefix.as_deref()
    }
}

/// 查询理解的意图分析器。
pub struct IntentAnalyzer {
    /// 用于范围推断的类别关键词。
    category_keywords: std::collections::HashMap<ContextNamespace, Vec<String>>,
}

impl IntentAnalyzer {
    /// 创建新的意图分析器。
    pub fn new() -> Self {
        Self {
            category_keywords: Self::build_category_keywords(),
        }
    }

    /// 构建默认类别关键词。
    fn build_category_keywords() -> std::collections::HashMap<ContextNamespace, Vec<String>> {
        let mut map = std::collections::HashMap::new();

        // 使用更具体的关键词以避免误匹配
        map.insert(
            ContextNamespace::User,
            vec![
                "user profile".to_string(),
                "user preference".to_string(),
                "user setting".to_string(),
                "my profile".to_string(),
                "my preference".to_string(),
                "my setting".to_string(),
            ],
        );

        map.insert(
            ContextNamespace::Memory,
            vec![
                "memory".to_string(),
                "session".to_string(),
                "conversation".to_string(),
                "history".to_string(),
                "past".to_string(),
                "remember".to_string(),
                "recall".to_string(),
                "event".to_string(),
                "case".to_string(),
            ],
        );

        map.insert(
            ContextNamespace::Knowledge,
            vec![
                "knowledge".to_string(),
                "document".to_string(),
                "documents".to_string(),
                "file".to_string(),
                "code".to_string(),
                "image".to_string(),
                "data".to_string(),
                "project".to_string(),
                "article".to_string(),
                "paper".to_string(),
                "book".to_string(),
            ],
        );

        map.insert(
            ContextNamespace::Agent,
            vec![
                "skill".to_string(),
                "skills".to_string(),
                "ability".to_string(),
                "pattern".to_string(),
                "config".to_string(),
                "agent".to_string(),
                "assistant".to_string(),
                "tool".to_string(),
            ],
        );

        map.insert(
            ContextNamespace::Skill,
            vec![
                "技能".to_string(),
                "能力".to_string(),
                "自动化".to_string(),
                "脚本".to_string(),
                "workflow".to_string(),
                "capability".to_string(),
                "automation".to_string(),
                "script".to_string(),
            ],
        );

        map
    }

    /// 分析查询并创建意图。
    ///
    /// 注意：不做查询嵌入 —— 向量检索路径由 VFS 内部统一嵌入
    /// （`VirtualFileSystem::search`），此处若再嵌入一次会产生
    /// 一次被丢弃的模型调用（双 embedding）。
    pub(crate) async fn analyze(&self, query: impl Into<String>) -> Result<Intent> {
        let query = query.into();
        let mut intent = Intent::new(&query);

        // 确定查询类型
        intent.query_type = self.classify_query_type(&query);

        // 确定目标范围
        intent.target_scope = self.infer_target_scope(&query);

        Ok(intent)
    }

    /// 分类查询类型。
    fn classify_query_type(&self, query: &str) -> QueryType {
        let lower = query.to_lowercase();

        // 检查导航模。
        let nav_patterns = [
            "go to",
            "open",
            "show me",
            "navigate",
            "browse",
            "list",
            "ls ",
            "directory",
            "folder",
        ];
        for pattern in nav_patterns {
            if lower.contains(pattern) {
                return QueryType::Navigate(query.to_string());
            }
        }

        // 检查检索模。
        let retrieve_patterns = [
            "get ",
            "fetch ",
            "load ",
            "read ",
            "content of",
            "full content",
            "tianyan://",
        ];
        for pattern in retrieve_patterns {
            if lower.contains(pattern) {
                return QueryType::Retrieve(query.to_string());
            }
        }

        // 默认为搜。
        QueryType::Search(query.to_string())
    }

    /// 从查询内容推断目标范围。
    fn infer_target_scope(&self, query: &str) -> Option<TargetScope> {
        let lower = query.to_lowercase();

        // 按确定性顺序检查类别
        // 优先级：User > Memory > Knowledge > Agent > Skill
        //
        // Session 不在候选中：会话内容不入向量库（ADR-018），把它作为检索目标
        // 只会得到恒空结果（曾使"对话/上次/摘要"类 query 静默检索为空，且无
        // 日志线索）；会话回忆统一走 FTS5 关键词回忆（`session_recall`）。
        let ordered_categories = [
            ContextNamespace::User,
            ContextNamespace::Memory,
            ContextNamespace::Knowledge,
            ContextNamespace::Agent,
            ContextNamespace::Skill,
        ];

        for category in ordered_categories {
            if let Some(keywords) = self.category_keywords.get(&category) {
                for keyword in keywords {
                    if lower.contains(keyword) {
                        return Some(TargetScope::new(category));
                    }
                }
            }
        }

        // 检查显式 URI 引用
        if let Some(uri_start) = lower.find("tianyan://") {
            let uri_str = &query[uri_start..];
            if let Ok(uri) = TianyanUri::parse(uri_str.split_whitespace().next().unwrap_or(uri_str))
            {
                return Some(TargetScope::from_uri(&uri));
            }
        }

        None
    }
}

impl Default for IntentAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_type() {
        let search = QueryType::search("find documents");
        assert!(search.is_search());
        assert_eq!(search.as_str(), "find documents");

        let nav = QueryType::navigate("go to user profile");
        assert!(nav.is_navigate());

        let ret = QueryType::retrieve("get tianyan://user/profile");
        assert!(ret.is_retrieve());
    }

    #[test]
    fn test_intent_creation() {
        let intent = Intent::new("test query");
        assert_eq!(intent.original_query, "test query");
        assert!(intent.target_scope.is_none());
    }

    #[test]
    fn test_target_scope() {
        let scope = TargetScope::new(ContextNamespace::Knowledge);
        assert_eq!(scope.namespace(), "knowledge");
        assert!(scope.sub_path().is_empty());

        let scope_with_path = TargetScope::with_path(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), "api".to_string()],
        );
        assert_eq!(scope_with_path.sub_path().len(), 2);
        assert!(scope_with_path.uri_prefix().is_some());
    }

    #[test]
    fn test_intent_analyzer_classify() {
        let analyzer = IntentAnalyzer::new();

        let search_type = analyzer.classify_query_type("find my documents");
        assert!(search_type.is_search());

        let nav_type = analyzer.classify_query_type("go to user profile");
        assert!(nav_type.is_navigate());

        let retrieve_type = analyzer.classify_query_type("get tianyan://user/profile");
        assert!(retrieve_type.is_retrieve());
    }

    #[test]
    fn test_intent_analyzer_scope_inference() {
        let analyzer = IntentAnalyzer::new();

        // Test with explicit category keywords that are unambiguous
        // Use keywords that only match one category
        let scope = analyzer.infer_target_scope("user preference settings");
        assert!(scope.is_some());
        let scope = scope.unwrap();
        assert_eq!(scope.category_enum(), ContextNamespace::User);

        // Use "documents" which is a Knowledge keyword
        let scope = analyzer.infer_target_scope("search for documents");
        assert!(scope.is_some());
        let scope = scope.unwrap();
        assert_eq!(scope.category_enum(), ContextNamespace::Knowledge);

        // Use "skills" which is an Agent keyword
        let scope = analyzer.infer_target_scope("what skills do you have");
        assert!(scope.is_some());
        let scope = scope.unwrap();
        assert_eq!(scope.category_enum(), ContextNamespace::Agent);

        // Use "memory" which is a Memory keyword
        let scope = analyzer.infer_target_scope("recall memory events");
        assert!(scope.is_some());
        let scope = scope.unwrap();
        assert_eq!(scope.category_enum(), ContextNamespace::Memory);
    }

    #[test]
    fn test_scope_inference_never_targets_session() {
        // 回归（P0-8）：Session 不入向量库——把会话类关键词映射到 Session 会让
        // "上次/对话/摘要"类 query 静默检索为空（无日志线索）。
        let analyzer = IntentAnalyzer::new();
        for q in [
            "上次我们讨论的方案",
            "看一下对话摘要",
            "session notes",
            "聊天记录",
        ] {
            let scope = analyzer.infer_target_scope(q);
            assert_ne!(
                scope.as_ref().map(|s| s.category_enum()),
                Some(ContextNamespace::Session),
                "query={q} 不得映射到 Session"
            );
        }
    }

    #[tokio::test]
    async fn test_intent_analyzer_analyze() {
        let analyzer = IntentAnalyzer::new();
        let intent = analyzer.analyze("find my user profile").await.unwrap();

        assert_eq!(intent.original_query, "find my user profile");
        // 意图分析不做查询嵌入（VFS 检索路径统一嵌入），无向量字段
        assert!(intent.target_scope.is_some());
    }

    #[test]
    fn test_estimate_token_count() {
        assert!(estimate_tokens("hello") > 0);
        assert!(estimate_tokens("hello world") > 0);
        assert!(estimate_tokens("a".repeat(100).as_str()) > 0);
    }
}
