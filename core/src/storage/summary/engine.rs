//! 用于生成分层摘要的摘要引擎。

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

/// 用于生成分层摘要的摘要引擎。
pub struct SummaryEngine {
    model_service: Arc<dyn ChatService>,
    embedding_service: Arc<dyn EmbeddingService>,
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
    ) -> Self {
        Self {
            model_service,
            embedding_service,
            model_name: model_name.into(),
            embedding_model_name: embedding_model_name.into(),
        }
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

        Ok(response)
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

        Ok(response)
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

/// 用于测试的模拟摘要引擎。
pub struct MockSummaryEngine;

impl MockSummaryEngine {
    /// 创建新的模拟摘要引擎。
    pub fn new() -> Self {
        Self
    }

    /// 生成模拟摘要。
    pub fn generate_abstract(&self, content: &str) -> String {
        content.to_string()
    }

    /// 生成模拟概览。
    pub fn generate_overview(&self, content: &str) -> String {
        content.to_string()
    }
}

impl Default for MockSummaryEngine {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_level_token_limit() {
        assert_eq!(SummaryLevel::Abstract.token_limit(), 100);
        assert_eq!(SummaryLevel::Overview.token_limit(), 2000);
    }

    #[test]
    fn test_summary_level_variants() {
        let abstract_level = SummaryLevel::Abstract;
        let overview_level = SummaryLevel::Overview;

        assert_ne!(abstract_level.token_limit(), overview_level.token_limit());
        assert!(abstract_level.token_limit() < overview_level.token_limit());
    }

    #[test]
    fn test_mock_summary_engine() {
        let engine = MockSummaryEngine::new();

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
    fn test_token_limit_constants() {
        assert_eq!(ABSTRACT_TOKEN_LIMIT, 100);
        assert_eq!(OVERVIEW_TOKEN_LIMIT, 2000);
        assert!(ABSTRACT_TOKEN_LIMIT < OVERVIEW_TOKEN_LIMIT);
    }
}
