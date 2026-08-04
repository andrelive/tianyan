//! 用于生成分层摘要的摘要引擎。

use std::sync::Arc;

use crate::common::error::Result;
use crate::model::{ChatService, EmbeddingService};

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
    model_name: String,
}

impl SummaryEngine {
    /// 创建新的摘要引擎。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        _embedding_service: Arc<dyn EmbeddingService>,
        model_name: impl Into<String>,
        _embedding_model_name: impl Into<String>,
    ) -> Self {
        Self {
            model_service,
            model_name: model_name.into(),
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
#[cfg(test)]
pub struct MockSummaryEngine;

#[cfg(test)]
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

#[cfg(test)]
impl Default for MockSummaryEngine {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::TokenUsage;
    use crate::model::types::{
        ChatChoice, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse,
    };
    use crate::model::{EmbeddingService, MockChatService};
    use async_trait::async_trait;

    /// Helper to build a ChatCompletionResponse from a content string.
    fn mock_chat_response(content: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: crate::common::types::Message::assistant(content),
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    mockall::mock! {
        pub TestEmbeddingService {}
        #[async_trait]
        impl EmbeddingService for TestEmbeddingService {
            async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse>;
            fn embedding_dimension(&self, model: &str) -> usize;
        }
    }

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
        let engine = MockSummaryEngine;
        let content = "Test content";
        let abstract_content = engine.generate_abstract(content);
        assert!(!abstract_content.is_empty());
    }

    #[test]
    fn test_token_limit_constants() {
        assert_eq!(ABSTRACT_TOKEN_LIMIT, 100);
        assert_eq!(OVERVIEW_TOKEN_LIMIT, 2000);
        const { assert!(ABSTRACT_TOKEN_LIMIT < OVERVIEW_TOKEN_LIMIT) };
    }

    /// ── 真实 SummaryEngine 测试 ─────────────────────────────────────────

    #[test]
    fn test_real_engine_constructor() {
        let chat = MockChatService::new();
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );
        assert_eq!(engine.model_name, "test-model");
    }

    #[tokio::test]
    async fn test_generate_abstract_with_mock() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("Test abstract summary")));
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );
        let result = engine.generate_abstract("Some content").await.unwrap();
        assert_eq!(result, "Test abstract summary");
    }

    #[tokio::test]
    async fn test_generate_overview_with_mock() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("Test overview content")));
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );
        let result = engine.generate_overview("Some content").await.unwrap();
        assert_eq!(result, "Test overview content");
    }

    #[tokio::test]
    async fn test_generate_summaries_with_mock() {
        let mut chat = MockChatService::new();
        let call_count = std::sync::Mutex::new(0usize);
        chat.expect_chat_completion().returning(move |_| {
            let mut count = call_count.lock().unwrap();
            *count += 1;
            if *count == 1 {
                Ok(mock_chat_response("Test abstract summary"))
            } else {
                Ok(mock_chat_response("Test overview content"))
            }
        });
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );
        let (abstract_result, overview_result) =
            engine.generate_summaries("Some content").await.unwrap();
        assert_eq!(abstract_result, "Test abstract summary");
        assert_eq!(overview_result, "Test overview content");
    }

    #[tokio::test]
    async fn test_generate_image_abstract_with_mock() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("A cat sitting on a chair")));
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );
        let result = engine.generate_image_abstract("Cat image").await.unwrap();
        assert_eq!(result, "A cat sitting on a chair");
    }

    #[tokio::test]
    async fn test_generate_image_overview() {
        let chat = MockChatService::new();
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );

        // description only — no elements, no ocr_text
        let result = engine
            .generate_image_overview("A cat", &[], None)
            .await
            .unwrap();
        assert!(
            result.contains("## 图片描述"),
            "should have description header"
        );
        assert!(result.contains("A cat"), "should contain description text");
        assert!(
            !result.contains("## 识别元素"),
            "should NOT have elements section"
        );
        assert!(
            !result.contains("## 图中文字"),
            "should NOT have OCR section"
        );

        // description + elements, no OCR
        let result = engine
            .generate_image_overview("A cat", &["cat".to_string(), "chair".to_string()], None)
            .await
            .unwrap();
        assert!(result.contains("## 图片描述"));
        assert!(result.contains("## 识别元素"));
        assert!(result.contains("- cat"));
        assert!(result.contains("- chair"));
        assert!(!result.contains("## 图中文字"));

        // description + ocr_text, no elements
        let result = engine
            .generate_image_overview("A sign", &[], Some("Hello World"))
            .await
            .unwrap();
        assert!(result.contains("## 图片描述"));
        assert!(!result.contains("## 识别元素"));
        assert!(result.contains("## 图中文字"));
        assert!(result.contains("Hello World"));
    }

    #[tokio::test]
    async fn test_generate_image_overview_full() {
        let chat = MockChatService::new();
        let emb = MockTestEmbeddingService::new();
        let engine = SummaryEngine::new(
            Arc::new(chat),
            Arc::new(emb),
            "test-model",
            "test-embed-model",
        );

        let result = engine
            .generate_image_overview(
                "A busy street",
                &[
                    "car".to_string(),
                    "pedestrian".to_string(),
                    "traffic light".to_string(),
                ],
                Some("STOP\nWALK"),
            )
            .await
            .unwrap();

        assert!(result.contains("## 图片描述"));
        assert!(result.contains("A busy street"));
        assert!(result.contains("## 识别元素"));
        assert!(result.contains("- car"));
        assert!(result.contains("- pedestrian"));
        assert!(result.contains("- traffic light"));
        assert!(result.contains("## 图中文字"));
        assert!(result.contains("STOP"));
        assert!(result.contains("WALK"));

        // Verify format order: description → elements → OCR
        let desc_pos = result.find("## 图片描述").unwrap();
        let elem_pos = result.find("## 识别元素").unwrap();
        let ocr_pos = result.find("## 图中文字").unwrap();
        assert!(
            desc_pos < elem_pos,
            "description should come before elements"
        );
        assert!(elem_pos < ocr_pos, "elements should come before OCR text");
    }
}
