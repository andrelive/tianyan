//! 用于生成分层摘要的摘要引擎。

use async_trait::async_trait;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, TianyanUri};
use crate::model::{ChatService, EmbeddingService};
use crate::storage::traits::StorageBackend;
use crate::storage::types::ContextEntry;

/// 不同内容级别的 token 限制。
pub const ABSTRACT_TOKEN_LIMIT: usize = 100;
/// Overview 级别 token 限制。
pub const OVERVIEW_TOKEN_LIMIT: usize = 2000;

/// 摘要级别枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryLevel {
    /// L0: 摘要级别，约 100 个 token。
    Abstract,
    /// L1: 概览级别，约 2K 个 token。
    Overview,
}

impl SummaryLevel {
    /// 获取此级别的 token 限制。
    pub fn token_limit(&self) -> usize {
        match self {
            SummaryLevel::Abstract => ABSTRACT_TOKEN_LIMIT,
            SummaryLevel::Overview => OVERVIEW_TOKEN_LIMIT,
        }
    }

    /// 获取此级别的名称。
    pub fn name(&self) -> &'static str {
        match self {
            SummaryLevel::Abstract => "abstract",
            SummaryLevel::Overview => "overview",
        }
    }
}

/// 摘要生成 trait。
///
/// 此 trait 定义了生成分层摘要的接口，支持生成 Abstract 和 Overview 两个级别的内容。
/// Abstract 是不超过 100 token 的语义摘要，用于向量搜索和快速过滤。
/// Overview 是不超过 2000 token 的结构化概览，用于内容导航和重新排序。
#[async_trait]
pub trait SummaryGenerator: Send + Sync {
    /// 生成摘要（L0）。
    ///
    /// 摘要是约 100 个 token 的简洁摘要，用于向量搜索和快速过滤。
    ///
    /// # 参数
    /// - `content`: 要生成摘要的内容
    ///
    /// # 返回
    /// - `Ok(String)`: 生成的摘要内容
    /// - `Err`: 生成失败
    ///
    /// # Errors
    /// LLM 调用失败或内容处理出错时返回错误。
    async fn generate_abstract(&self, content: &str) -> Result<String>;

    /// 生成概览（L1）。
    ///
    /// 概览是约 2K 个 token 的详细摘要，用于内容导航和重新排序。
    ///
    /// # 参数
    /// - `content`: 要生成概览的内容
    ///
    /// # 返回
    /// - `Ok(String)`: 生成的概览内容
    /// - `Err`: 生成失败
    ///
    /// # Errors
    /// LLM 调用失败或内容处理出错时返回错误。
    async fn generate_overview(&self, content: &str) -> Result<String>;

    /// 同时生成摘要和概览。
    ///
    /// 这是一个便捷方法，同时调用 `generate_abstract` 和 `generate_overview`。
    ///
    /// # 参数
    /// - `content`: 要生成摘要和概览的内容
    ///
    /// # 返回
    /// - `Ok((String, String))`: (摘要，概览) 元组
    /// - `Err`: 生成失败
    ///
    /// # Errors
    /// LLM 调用失败或内容处理出错时返回错误。
    async fn generate_summaries(&self, content: &str) -> Result<(String, String)> {
        let abstract_content = self.generate_abstract(content).await?;
        let overview_content = self.generate_overview(content).await?;
        Ok((abstract_content, overview_content))
    }
}

/// 使用 tiktoken 的 token 计数器。
pub struct TokenCounter {
    encoder: tiktoken_rs::CoreBPE,
}

impl TokenCounter {
    /// 创建新的 token 计数器。
    pub fn new() -> Result<Self> {
        let encoder = tiktoken_rs::cl100k_base()
            .map_err(|e| TianyanError::TokenCounting(format!("初始化分词器失败: {}", e)))?;
        Ok(Self { encoder })
    }

    /// 计算文本中的 token 数量。
    pub fn count_tokens(&self, text: &str) -> usize {
        self.encoder.encode_with_special_tokens(text).len()
    }

    /// 检查文本是否超过 token 限制。
    pub fn exceeds_limit(&self, text: &str, limit: usize) -> bool {
        self.count_tokens(text) > limit
    }

    /// 截断文本以适应 token 限制。
    pub fn truncate_to_limit(&self, text: &str, limit: usize) -> String {
        let tokens = self.encoder.encode_with_special_tokens(text);
        if tokens.len() <= limit {
            return text.to_string();
        }

        // 解码截断后的 token
        let truncated_tokens: Vec<usize> = tokens[..limit].to_vec();
        self.encoder.decode(truncated_tokens).unwrap_or_default()
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new().expect("创建默认 TokenCounter 失败")
    }
}

/// 用于生成分层摘要的摘要引擎。
pub struct SummaryEngine {
    model_service: Arc<dyn ChatService>,
    embedding_service: Arc<dyn EmbeddingService>,
    token_counter: TokenCounter,
    model_name: String,
    embedding_model_name: String,
}

impl SummaryEngine {
    /// 创建新的摘要引擎。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        embedding_service: Arc<dyn EmbeddingService>,
        model_name: impl Into<String>,
        embedding_model_name: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            model_service,
            embedding_service,
            token_counter: TokenCounter::new()?,
            model_name: model_name.into(),
            embedding_model_name: embedding_model_name.into(),
        })
    }

    /// 获取 token 计数器。
    pub fn token_counter(&self) -> &TokenCounter {
        &self.token_counter
    }

    /// 生成摘要（L0）。
    ///
    /// 摘要是约 100 个 token 的简洁摘要，
    /// 用于向量搜索和快速过滤。
    pub async fn generate_abstract(&self, content: &str) -> Result<String> {
        let prompt = format!(
            r#"请为以下内容生成一个简洁的摘要，限制在 100 个 token 以内。
摘要应该：
1. 突出主要内容
2. 包含关键信息
3. 便于快速理解

内容：
{}

请直接输出摘要，不要包含其他说明。"#,
            content
        );

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(&prompt)],
            )
            .await?;

        // 确保摘要不超过限制
        let abstract_content = if self
            .token_counter
            .exceeds_limit(&response, ABSTRACT_TOKEN_LIMIT)
        {
            self.token_counter
                .truncate_to_limit(&response, ABSTRACT_TOKEN_LIMIT)
        } else {
            response
        };

        Ok(abstract_content)
    }

    /// 生成概览（L1）。
    ///
    /// 概览是约 2K 个 token 的详细摘要，
    /// 用于内容导航和重新排序。
    pub async fn generate_overview(&self, content: &str) -> Result<String> {
        let prompt = format!(
            r#"请为以下内容生成一个详细的概览，限制在 2000 个 token 以内。
概览应该：
1. 包含主要结构和关键点
2. 保留重要细节
3. 便于内容导航

内容：
{}

请直接输出概览，不要包含其他说明。"#,
            content
        );

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(&prompt)],
            )
            .await?;

        // 确保概览不超过限制
        let overview_content = if self
            .token_counter
            .exceeds_limit(&response, OVERVIEW_TOKEN_LIMIT)
        {
            self.token_counter
                .truncate_to_limit(&response, OVERVIEW_TOKEN_LIMIT)
        } else {
            response
        };

        Ok(overview_content)
    }

    /// 同时生成摘要和概览。
    pub async fn generate_summaries(&self, content: &str) -> Result<(String, String)> {
        let abstract_content = self.generate_abstract(content).await?;
        let overview_content = self.generate_overview(content).await?;
        Ok((abstract_content, overview_content))
    }

    /// 为文本生成嵌入向量。
    pub async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let embedding = self
            .embedding_service
            .embed_single(&self.embedding_model_name, text)
            .await?;
        Ok(embedding.vector)
    }

    /// 为摘要和概览生成嵌入向量。
    pub async fn generate_embeddings(
        &self,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        let abstract_embedding = self.generate_embedding(abstract_content).await?;
        let overview_embedding = self.generate_embedding(overview_content).await?;
        Ok((abstract_embedding, overview_embedding))
    }

    /// 更新条目的摘要。
    pub async fn update_entry_summaries(
        &self,
        storage: &dyn StorageBackend,
        uri: &TianyanUri,
    ) -> Result<ContextEntry> {
        // 读取当前条目
        let mut entry = storage.read_entry(uri).await?;

        // 获取详细内容
        let detail_content = match &entry.detail_content {
            Some(content) => content.clone(),
            None => {
                // 尝试从存储中读取
                match storage.read_content(uri, ContentLevel::Detail).await {
                    Ok(content) => content,
                    Err(_) => {
                        return Err(TianyanError::SummaryGeneration(format!(
                            "{} 没有可用的详细内容",
                            uri
                        )));
                    }
                }
            }
        };

        // 生成摘要
        let (abstract_content, overview_content) = self.generate_summaries(&detail_content).await?;

        // 更新条目
        entry.abstract_content = Some(abstract_content);
        entry.overview_content = Some(overview_content);

        // 更新 token 计数
        entry.token_counts.abstract_tokens = Some(
            self.token_counter
                .count_tokens(entry.abstract_content.as_ref().unwrap()),
        );
        entry.token_counts.overview_tokens = Some(
            self.token_counter
                .count_tokens(entry.overview_content.as_ref().unwrap()),
        );

        // 写入更新后的条目
        storage.write_entry(&entry).await?;

        Ok(entry)
    }

    /// 通过聚合子条目摘要来更新父目录摘要。
    pub async fn update_parent_summaries(
        &self,
        storage: &dyn StorageBackend,
        uri: &TianyanUri,
    ) -> Result<()> {
        let mut current_uri = uri.clone();

        while let Some(parent_uri) = current_uri.parent() {
            // 获取所有子条目
            let children = storage.list_directory(&parent_uri).await?;

            if children.is_empty() {
                current_uri = parent_uri;
                continue;
            }

            // 合并子条目的摘要
            let combined_abstracts: Vec<String> = children
                .iter()
                .filter_map(|c| c.abstract_content.as_ref())
                .cloned()
                .collect();

            if combined_abstracts.is_empty() {
                current_uri = parent_uri;
                continue;
            }

            let combined_text = combined_abstracts.join("\n\n");

            // 生成父条目摘要
            let (abstract_content, overview_content) =
                self.generate_summaries(&combined_text).await?;

            // 更新父条目
            let mut parent_entry = storage.read_entry(&parent_uri).await?;
            parent_entry.abstract_content = Some(abstract_content);
            parent_entry.overview_content = Some(overview_content);
            parent_entry.metadata.touch();

            storage.write_entry(&parent_entry).await?;

            current_uri = parent_uri;
        }

        Ok(())
    }

    /// 检查内容是否需要生成摘要。
    pub fn needs_summarization(&self, content: &str) -> bool {
        let token_count = self.token_counter.count_tokens(content);
        token_count > ABSTRACT_TOKEN_LIMIT
    }

    /// 估算内容是否为需要分块的长文档。
    pub fn is_long_document(&self, content: &str) -> bool {
        let token_count = self.token_counter.count_tokens(content);
        // 超过 8000 个 token 的文档被视为长文档
        token_count > 8000
    }

    /// 为图片描述生成摘要。
    pub async fn generate_image_abstract(&self, unified_text: &str) -> Result<String> {
        let prompt = format!(
            r#"请用一句话（不超过 50 字）概括这张图片。

{}

请直接输出概括，不要包含其他说明。"#,
            unified_text
        );

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(&prompt)],
            )
            .await?;

        Ok(response)
    }

    /// 为图片生成概览。
    pub async fn generate_image_overview(
        &self,
        description: &str,
        elements: &[String],
        ocr_text: Option<&str>,
    ) -> Result<String> {
        let mut overview = format!("## 图片描述\n{}\n\n", description);

        if !elements.is_empty() {
            overview.push_str("## 识别元素\n");
            for elem in elements {
                overview.push_str(&format!("- {}\n", elem));
            }
            overview.push('\n');
        }

        if let Some(text) = ocr_text {
            if !text.is_empty() {
                overview.push_str(&format!("## 图中文字\n```\n{}\n```\n", text));
            }
        }

        Ok(overview)
    }
}

/// 为 SummaryEngine 实现 SummaryGenerator trait。
#[async_trait]
impl SummaryGenerator for SummaryEngine {
    async fn generate_abstract(&self, content: &str) -> Result<String> {
        // 使用完全限定语法调用 inherent method，避免递归
        SummaryEngine::generate_abstract(self, content).await
    }

    async fn generate_overview(&self, content: &str) -> Result<String> {
        // 使用完全限定语法调用 inherent method，避免递归
        SummaryEngine::generate_overview(self, content).await
    }

    async fn generate_summaries(&self, content: &str) -> Result<(String, String)> {
        // 使用完全限定语法调用 inherent method，避免递归
        SummaryEngine::generate_summaries(self, content).await
    }
}

/// 用于测试的模拟摘要引擎。
pub struct MockSummaryEngine {
    token_counter: TokenCounter,
}

impl MockSummaryEngine {
    /// 创建新的模拟摘要引擎。
    pub fn new() -> Result<Self> {
        Ok(Self {
            token_counter: TokenCounter::new()?,
        })
    }

    /// 获取 token 计数器。
    pub fn token_counter(&self) -> &TokenCounter {
        &self.token_counter
    }

    /// 生成模拟摘要。
    pub fn generate_abstract(&self, content: &str) -> String {
        // 简单截断用于测试
        if self
            .token_counter
            .exceeds_limit(content, ABSTRACT_TOKEN_LIMIT)
        {
            self.token_counter
                .truncate_to_limit(content, ABSTRACT_TOKEN_LIMIT)
        } else {
            content.to_string()
        }
    }

    /// 生成模拟概览。
    pub fn generate_overview(&self, content: &str) -> String {
        // 简单截断用于测试
        if self
            .token_counter
            .exceeds_limit(content, OVERVIEW_TOKEN_LIMIT)
        {
            self.token_counter
                .truncate_to_limit(content, OVERVIEW_TOKEN_LIMIT)
        } else {
            content.to_string()
        }
    }
}

impl Default for MockSummaryEngine {
    fn default() -> Self {
        Self::new().expect("创建默认 MockSummaryEngine 失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Token 计数器测试 ====================

    #[test]
    fn test_token_counter() {
        let counter = TokenCounter::new().unwrap();

        let text = "Hello, world! This is a test.";
        let count = counter.count_tokens(text);
        assert!(count > 0);

        // 测试限制检查
        assert!(!counter.exceeds_limit(text, 100));
        assert!(counter.exceeds_limit(text, 1));
    }

    #[test]
    fn test_token_counter_truncate() {
        let counter = TokenCounter::new().unwrap();

        let text = "This is a longer text that will be truncated.";
        let truncated = counter.truncate_to_limit(text, 5);

        // 截断后的文本应该有更少的 token
        assert!(counter.count_tokens(&truncated) <= 5);
    }

    #[test]
    fn test_token_counter_chinese() {
        let counter = TokenCounter::new().unwrap();

        // 中文文本
        let chinese = "这是一个中文测试文本";
        let count = counter.count_tokens(chinese);
        assert!(count > 0);
    }

    #[test]
    fn test_token_counter_mixed_language() {
        let counter = TokenCounter::new().unwrap();

        // 混合语言文本
        let mixed = "Hello 世界! This is a 测试.";
        let count = counter.count_tokens(mixed);
        assert!(count > 0);
    }

    #[test]
    fn test_token_counter_empty() {
        let counter = TokenCounter::new().unwrap();

        let count = counter.count_tokens("");
        assert_eq!(count, 0);
    }

    #[test]
    fn test_token_counter_code() {
        let counter = TokenCounter::new().unwrap();

        // 代码片段
        let code = r#"
fn main() {
    println!("Hello, world!");
}
"#;
        let count = counter.count_tokens(code);
        assert!(count > 0);
    }

    #[test]
    fn test_token_counter_default() {
        let counter1 = TokenCounter::new().unwrap();
        let counter2 = TokenCounter::default();

        let text = "Test text";
        assert_eq!(counter1.count_tokens(text), counter2.count_tokens(text));
    }

    // ==================== 摘要级别测试 ====================

    #[test]
    fn test_summary_level_token_limit() {
        assert_eq!(SummaryLevel::Abstract.token_limit(), 100);
        assert_eq!(SummaryLevel::Overview.token_limit(), 2000);
    }

    #[test]
    fn test_summary_level_variants() {
        // 测试预期的变体
        let abstract_level = SummaryLevel::Abstract;
        let overview_level = SummaryLevel::Overview;

        assert_ne!(abstract_level.token_limit(), overview_level.token_limit());
        assert!(abstract_level.token_limit() < overview_level.token_limit());
    }

    // ==================== 模拟摘要引擎测试 ====================

    #[test]
    fn test_mock_summary_engine() {
        let engine = MockSummaryEngine::new().unwrap();

        let content = "This is test content for the summary engine.";
        let abstract_content = engine.generate_abstract(content);
        let overview_content = engine.generate_overview(content);

        assert!(!abstract_content.is_empty());
        assert!(!overview_content.is_empty());
    }

    #[test]
    fn test_mock_summary_engine_default() {
        let engine = MockSummaryEngine::default();
        let content = "Test content";
        let abstract_content = engine.generate_abstract(content);
        assert!(!abstract_content.is_empty());
    }

    #[test]
    fn test_needs_summarization() {
        let engine = MockSummaryEngine::new().unwrap();

        // 短内容不需要生成摘要
        let short_content = "Short text.";
        assert!(!engine
            .token_counter()
            .exceeds_limit(short_content, ABSTRACT_TOKEN_LIMIT));

        // 长内容需要生成摘要
        let long_content: String = "word ".repeat(200);
        assert!(engine
            .token_counter()
            .exceeds_limit(&long_content, ABSTRACT_TOKEN_LIMIT));
    }

    #[test]
    fn test_is_long_document() {
        let engine = MockSummaryEngine::new().unwrap();

        // 普通文档
        let normal_content = "This is a normal document.".repeat(100);
        assert!(!engine.token_counter().exceeds_limit(&normal_content, 8000));

        // 长文档
        let long_content: String = "word ".repeat(10000);
        assert!(engine.token_counter().exceeds_limit(&long_content, 8000));
    }

    #[test]
    fn test_mock_abstract_truncation() {
        let engine = MockSummaryEngine::new().unwrap();

        // 创建超过摘要限制的内容
        let long_content: String = "word ".repeat(500);
        let abstract_content = engine.generate_abstract(&long_content);

        // 摘要应该被截断以适应限制
        let token_count = engine.token_counter().count_tokens(&abstract_content);
        assert!(token_count <= ABSTRACT_TOKEN_LIMIT);
    }

    #[test]
    fn test_mock_overview_truncation() {
        let engine = MockSummaryEngine::new().unwrap();

        // 创建超过概览限制的内容
        let long_content: String = "word ".repeat(5000);
        let overview_content = engine.generate_overview(&long_content);

        // 概览应该被截断以适应限制
        let token_count = engine.token_counter().count_tokens(&overview_content);
        assert!(token_count <= OVERVIEW_TOKEN_LIMIT);
    }

    // ==================== Token 限制常量测试 ====================

    #[test]
    fn test_token_limit_constants() {
        assert_eq!(ABSTRACT_TOKEN_LIMIT, 100);
        assert_eq!(OVERVIEW_TOKEN_LIMIT, 2000);
        assert!(ABSTRACT_TOKEN_LIMIT < OVERVIEW_TOKEN_LIMIT);
    }

    // ==================== Token 计数器边界情况 ====================

    #[test]
    fn test_token_counter_whitespace() {
        let counter = TokenCounter::new().unwrap();

        let whitespace = "   \t\n  ";
        let count = counter.count_tokens(whitespace);
        // 空白字符也应该计入 token (count 为 usize 类型，无需检查 >= 0)
        assert!(count > 0);
    }

    #[test]
    fn test_token_counter_special_chars() {
        let counter = TokenCounter::new().unwrap();

        let special = "!@#$%^&*()_+-=[]{}|;':\",./<>?";
        let count = counter.count_tokens(special);
        assert!(count > 0);
    }

    #[test]
    fn test_token_counter_unicode() {
        let counter = TokenCounter::new().unwrap();

        // Emoji 等 Unicode 字符
        let unicode = "Hello 👋 World 🌍 Test 🧪";
        let count = counter.count_tokens(unicode);
        assert!(count > 0);
    }

    #[test]
    fn test_truncate_preserves_start() {
        let counter = TokenCounter::new().unwrap();

        let text = "The quick brown fox jumps over the lazy dog.";
        let truncated = counter.truncate_to_limit(text, 5);

        // 截断后的文本应该以相同内容开头
        assert!(truncated.starts_with("The"));
    }

    #[test]
    fn test_truncate_short_text_unchanged() {
        let counter = TokenCounter::new().unwrap();

        let short_text = "Short";
        let truncated = counter.truncate_to_limit(short_text, 100);

        // 短文本不应该被修改
        assert_eq!(truncated, short_text);
    }
}
