//! 内容加载模块。
//!
//! 本模块实现带 Token 预算管理的内容加载策略。

use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, TianyanUri};
use crate::context::compression::estimate_tokens;
use crate::storage::ContentLoader;

/// 内容加载 Token 预算。
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// 最大 Token 数量。
    pub max_tokens: usize,
    /// 已使用的 Token 数量。
    pub used_tokens: usize,
    /// 为系统提示词预留的 Token 数量。
    pub reserved_tokens: usize,
}

impl TokenBudget {
    /// 创建新的 Token 预算。
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            used_tokens: 0,
            reserved_tokens: 0,
        }
    }

    /// 创建带有预留 Token 的预算。
    pub fn with_reserved(max_tokens: usize, reserved_tokens: usize) -> Self {
        Self {
            max_tokens,
            used_tokens: 0,
            reserved_tokens,
        }
    }

    /// 获取可用 Token 数量。
    pub fn available(&self) -> usize {
        self.max_tokens
            .saturating_sub(self.used_tokens)
            .saturating_sub(self.reserved_tokens)
    }

    /// 检查是否有足够的 Token 可用。
    pub fn can_afford(&self, tokens: usize) -> bool {
        self.available() >= tokens
    }

    /// 使用 Token。
    pub fn use_tokens(&mut self, tokens: usize) -> Result<()> {
        if !self.can_afford(tokens) {
            return Err(TianyanError::TokenLimitExceeded {
                current: self.used_tokens.saturating_add(tokens),
                limit: self.max_tokens,
            });
        }
        self.used_tokens = self.used_tokens.saturating_add(tokens);
        Ok(())
    }

    /// 重置已使用的 Token。
    pub fn reset(&mut self) {
        self.used_tokens = 0;
    }

    /// 获取使用率百分比。
    pub fn utilization(&self) -> f32 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        (self.used_tokens as f32 / self.max_tokens as f32) * 100.0
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::new(4096) // 默认 4K Token 预算
    }
}

/// 基于相关性分数的内容加载策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLoadStrategy {
    /// 加载完整内容（L2） - 用于高相关性（分数 > 0.85）
    Full,
    /// 加载概览（L1） - 用于中等相关性（0.6 < 分数 <= 0.85）
    Overview,
    /// 仅加载摘要（L0） - 用于低相关性（分数 <= 0.6）
    Abstract,
}

impl ContentLoadStrategy {
    /// 根据相关性分数确定策略。
    pub fn from_score(score: f32) -> Self {
        if score > 0.85 {
            Self::Full
        } else if score > 0.6 {
            Self::Overview
        } else {
            Self::Abstract
        }
    }

    /// 根据分数和 Token 预算确定策略。
    pub fn from_score_and_budget(score: f32, budget: &TokenBudget) -> Self {
        // 如果预算紧张，降级策略
        let available = budget.available();

        if available < 100 {
            // 预算非常紧张 - 仅加载摘要
            Self::Abstract
        } else if available < 500 {
            // 预算紧张 - 优先加载概览
            if score > 0.9 {
                Self::Overview // 即使高相关性也只加载概览
            } else {
                Self::Abstract
            }
        } else {
            // 正常预算 - 使用基于分数的策略
            Self::from_score(score)
        }
    }

    /// 获取此策略对应的内容层级。
    pub fn to_content_level(&self) -> ContentLevel {
        match self {
            Self::Full => ContentLevel::Detail,
            Self::Overview => ContentLevel::Overview,
            Self::Abstract => ContentLevel::Abstract,
        }
    }

    /// 获取此策略的大致 Token 限制。
    pub fn token_limit(&self) -> Option<usize> {
        match self {
            Self::Full => None, // 无限制
            Self::Overview => Some(2000),
            Self::Abstract => Some(100),
        }
    }
}

/// 带有 Token 预算管理的内容加载器实现。
pub struct ContentLoaderImpl {
    /// 内部内容加载器（通常为 VirtualFileSystem）。
    inner: Arc<dyn ContentLoader>,
    /// Token 预算。
    budget: TokenBudget,
    /// 用于估算 Token 数量的计数器。
    token_counter: Option<TokenCounter>,
}

/// 使用字符估算的简单 Token 计数器。
#[derive(Debug, Clone)]
pub struct TokenCounter;

impl TokenCounter {
    /// 创建新的 Token 计数器。
    pub fn new() -> Self {
        Self
    }

    /// 计算字符串中的 Token 数量。
    pub fn count(&self, text: &str) -> usize {
        estimate_tokens(text)
    }

    /// 将文本截断到 Token 限制内。
    pub fn truncate(&self, text: &str, max_tokens: usize) -> String {
        let max_chars = max_tokens * 4;
        let chars: Vec<char> = text.chars().take(max_chars).collect();
        chars.into_iter().collect()
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentLoaderImpl {
    /// 创建新的内容加载器。
    pub fn new(inner: Arc<dyn ContentLoader>) -> Self {
        Self {
            inner,
            budget: TokenBudget::default(),
            token_counter: Some(TokenCounter::new()),
        }
    }

    /// 创建带有特定 Token 预算的内容加载器。
    pub fn with_budget(inner: Arc<dyn ContentLoader>, budget: TokenBudget) -> Self {
        Self {
            inner,
            budget,
            token_counter: Some(TokenCounter::new()),
        }
    }

    /// 获取 Token 预算。
    pub fn budget(&self) -> &TokenBudget {
        &self.budget
    }

    /// 获取 Token 预算的可变引用。
    pub fn budget_mut(&mut self) -> &mut TokenBudget {
        &mut self.budget
    }

    /// 使用特定策略加载内容。
    pub async fn load_with_strategy(
        &mut self,
        uri: &TianyanUri,
        strategy: ContentLoadStrategy,
    ) -> Result<LoadedContent> {
        let level = strategy.to_content_level();
        let content = self.inner.load_content(uri, level).await?;

        // 处理空内容
        if content.trim().is_empty() {
            tracing::warn!(uri = %uri, "加载到空内容");
            return Ok(LoadedContent {
                uri: uri.clone(),
                level,
                content: String::new(),
                tokens: 0,
                strategy,
            });
        }

        // 计算 Token 数量
        let tokens = self
            .token_counter
            .as_ref()
            .map(|c| c.count(&content))
            .unwrap_or_else(|| estimate_tokens(&content));

        // 更新预算
        self.budget.use_tokens(tokens)?;

        Ok(LoadedContent {
            uri: uri.clone(),
            level,
            content,
            tokens,
            strategy,
        })
    }

    /// 根据分数和预算加载内容。
    pub async fn load_by_score(&mut self, uri: &TianyanUri, score: f32) -> Result<LoadedContent> {
        let strategy = ContentLoadStrategy::from_score_and_budget(score, &self.budget);
        self.load_with_strategy(uri, strategy).await
    }

    /// 带有预算管理的批量内容加载。
    pub async fn load_batch(
        &mut self,
        items: Vec<(TianyanUri, f32)>,
    ) -> Result<Vec<LoadedContent>> {
        let mut results = Vec::new();

        // 按分数降序排序，优先加载高相关性内容
        let mut sorted_items = items;
        sorted_items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (uri, score) in sorted_items {
            match self.load_by_score(&uri, score).await {
                Ok(content) => results.push(content),
                Err(TianyanError::TokenLimitExceeded { .. }) => {
                    // 预算耗尽，停止加载
                    tracing::warn!(
                        "Token budget exhausted after loading {} items",
                        results.len()
                    );
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    /// 重置 Token 预算。
    pub fn reset_budget(&mut self) {
        self.budget.reset();
    }

    /// 设置 Token 预算。
    pub fn set_budget(&mut self, budget: TokenBudget) {
        self.budget = budget;
    }

    /// 检查特定层级的内容是否存在。
    pub async fn has_content_level(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool> {
        self.inner.has_content(uri, level).await
    }

    /// 获取 URI 的最佳可用内容层级。
    pub async fn get_best_available_level(
        &self,
        uri: &TianyanUri,
        preferred_level: ContentLevel,
    ) -> Result<ContentLevel> {
        // 检查首选层级是否可用
        if self.inner.has_content(uri, preferred_level).await? {
            return Ok(preferred_level);
        }

        // 回退到较低层级
        match preferred_level {
            ContentLevel::Detail => {
                if self.inner.has_content(uri, ContentLevel::Overview).await? {
                    Ok(ContentLevel::Overview)
                } else {
                    Ok(ContentLevel::Abstract)
                }
            }
            ContentLevel::Overview => Ok(ContentLevel::Abstract),
            ContentLevel::Abstract => Err(TianyanError::EntryNotFound(format!(
                "No content available at any level for URI: {}",
                uri
            ))),
        }
    }
}

/// 带有元数据的已加载内容。
#[derive(Debug, Clone)]
pub struct LoadedContent {
    /// 内容的 URI。
    pub uri: TianyanUri,
    /// 加载的内容层级。
    pub level: ContentLevel,
    /// 内容文本。
    pub content: String,
    /// 内容的 Token 数量。
    pub tokens: usize,
    /// 使用的加载策略。
    pub strategy: ContentLoadStrategy,
}

impl LoadedContent {
    /// 创建新的已加载内容。
    pub fn new(uri: TianyanUri, level: ContentLevel, content: String) -> Self {
        let tokens = estimate_tokens(&content);
        Self {
            uri,
            level,
            content,
            tokens,
            strategy: ContentLoadStrategy::from_score(0.5), // 默认策略
        }
    }

    /// 创建带有特定策略的已加载内容。
    pub fn with_strategy(
        uri: TianyanUri,
        level: ContentLevel,
        content: String,
        strategy: ContentLoadStrategy,
    ) -> Self {
        let tokens = estimate_tokens(&content);
        Self {
            uri,
            level,
            content,
            tokens,
            strategy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ContextEntry;
    use async_trait::async_trait;

    /// Mock content loader for testing.
    struct MockContentLoader;

    #[async_trait]
    impl ContentLoader for MockContentLoader {
        async fn load_content(&self, _uri: &TianyanUri, level: ContentLevel) -> Result<String> {
            Ok(match level {
                ContentLevel::Abstract => "Abstract content".to_string(),
                ContentLevel::Overview => "Overview content with more details".to_string(),
                ContentLevel::Detail => "Full detail content with all the information".to_string(),
            })
        }

        async fn load_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
            Ok(ContextEntry::new_file(uri.clone()))
        }

        async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> {
            Ok(true)
        }
    }

    #[test]
    fn test_token_budget() {
        let mut budget = TokenBudget::new(1000);
        assert_eq!(budget.available(), 1000);
        assert!(budget.can_afford(500));

        budget.use_tokens(300).unwrap();
        assert_eq!(budget.available(), 700);
        assert_eq!(budget.used_tokens, 300);

        budget.use_tokens(700).unwrap();
        assert_eq!(budget.available(), 0);
        assert!(!budget.can_afford(1));
    }

    #[test]
    fn test_token_budget_exceeded() {
        let mut budget = TokenBudget::new(100);
        budget.use_tokens(50).unwrap();
        let result = budget.use_tokens(100);
        assert!(result.is_err());
    }

    #[test]
    fn test_token_budget_reserved() {
        let budget = TokenBudget::with_reserved(1000, 200);
        assert_eq!(budget.available(), 800);
    }

    #[test]
    fn test_content_load_strategy() {
        assert_eq!(
            ContentLoadStrategy::from_score(0.9),
            ContentLoadStrategy::Full
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.7),
            ContentLoadStrategy::Overview
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.5),
            ContentLoadStrategy::Abstract
        );
    }

    #[test]
    fn test_content_load_strategy_to_level() {
        assert_eq!(
            ContentLoadStrategy::Full.to_content_level(),
            ContentLevel::Detail
        );
        assert_eq!(
            ContentLoadStrategy::Overview.to_content_level(),
            ContentLevel::Overview
        );
        assert_eq!(
            ContentLoadStrategy::Abstract.to_content_level(),
            ContentLevel::Abstract
        );
    }

    #[test]
    fn test_token_counter() {
        let counter = TokenCounter::new();
        let text = "Hello, world!";
        let count = counter.count(text);
        assert!(count > 0);

        let truncated = counter.truncate("Hello, world! This is a test.", 2);
        assert!(!truncated.is_empty());
    }

    #[tokio::test]
    async fn test_content_loader_impl() {
        let inner = Arc::new(MockContentLoader);
        let mut loader = ContentLoaderImpl::with_budget(inner, TokenBudget::new(1000));

        let uri = TianyanUri::new(
            crate::common::types::ContextNamespace::User,
            vec!["test".to_string()],
        );
        let content = loader
            .load_with_strategy(&uri, ContentLoadStrategy::Abstract)
            .await
            .unwrap();

        assert_eq!(content.level, ContentLevel::Abstract);
        assert!(content.tokens > 0);
        assert!(loader.budget().used_tokens > 0);
    }

    #[tokio::test]
    async fn test_content_loader_batch() {
        let inner = Arc::new(MockContentLoader);
        let mut loader = ContentLoaderImpl::with_budget(
            inner,
            TokenBudget::new(50), // Small budget
        );

        let items = vec![
            (
                TianyanUri::new(
                    crate::common::types::ContextNamespace::User,
                    vec!["a".to_string()],
                ),
                0.9,
            ),
            (
                TianyanUri::new(
                    crate::common::types::ContextNamespace::User,
                    vec!["b".to_string()],
                ),
                0.7,
            ),
            (
                TianyanUri::new(
                    crate::common::types::ContextNamespace::User,
                    vec!["c".to_string()],
                ),
                0.5,
            ),
        ];

        let results = loader.load_batch(items).await.unwrap();
        // Should load some items before budget is exhausted
        assert!(!results.is_empty());
    }

    #[test]
    fn test_loaded_content() {
        let uri = TianyanUri::new(
            crate::common::types::ContextNamespace::User,
            vec!["test".to_string()],
        );
        let content = LoadedContent::new(
            uri.clone(),
            ContentLevel::Overview,
            "Test content".to_string(),
        );

        assert_eq!(content.uri, uri);
        assert_eq!(content.level, ContentLevel::Overview);
        assert!(content.tokens > 0);
    }
}
