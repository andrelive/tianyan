//! 核心模型服务 trait。

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::common::error::Result;
use crate::common::types::{Embedding, Message};

use super::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest,
    EmbeddingResponse, ModelInfo, VisionRequest, VisionResponse,
};

/// 聊天补全服务 trait。
///
/// 此 trait 定义了与支持聊天补全的大语言模型
/// （如 OpenAI GPT-4、Claude、DeepSeek）交互的接口。
#[async_trait]
pub trait ChatService: Send + Sync {
    /// 发送聊天补全请求。
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse>;

    /// 发送聊天补全请求（流式）。
    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>>;

    /// 发送带消息的简单聊天请求。
    ///
    /// 注意：当模型返回 `tool_calls` 而非文本内容时，此方法返回空字符串。
    /// 如需处理工具调用，请直接使用 [`chat_completion`] 方法。
    async fn chat(&self, model: &str, messages: Vec<Message>) -> Result<String> {
        let request = ChatCompletionRequest::new(model, messages);
        let response = self.chat_completion(request).await?;
        Ok(response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mockall::mock! {
    /// ChatService 的 mockall 自动生成 mock。
    /// 使用 `MockChatService::new()` 创建，`expect_*().returning()` 配置行为。
    pub ChatService {}

    #[async_trait]
    impl ChatService for ChatService {
        async fn chat_completion(
            &self,
            request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse>;

        async fn chat_completion_stream(
            &self,
            request: ChatCompletionRequest,
        ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>>;
    }
}

/// 服务发现 trait。
///
/// 提供模型列表查询、可用性检查和名称获取的基础设施能力。
/// 独立于具体业务 trait（ChatService / EmbeddingService / VlmService），
/// 允许调用方只依赖此轻量 trait 做健康检查和服务发现。
#[async_trait]
pub trait ServiceDiscovery: Send + Sync {
    /// 获取可用模型列表。
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    /// 检查服务是否可用。
    async fn is_available(&self) -> bool;

    /// 获取服务名称。
    fn service_name(&self) -> &str;
}

/// 文本嵌入服务 trait。
///
/// 此 trait 定义了将文本转换为向量表示的嵌入服务接口。
#[async_trait]
pub trait EmbeddingService: Send + Sync {
    /// 为给定文本生成嵌入向量。
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse>;

    /// 为单个文本生成嵌入向量。
    async fn embed_single(&self, model: &str, text: &str) -> Result<Embedding> {
        let request = EmbeddingRequest::new(model, text);
        let response = self.embed(request).await?;
        response
            .data
            .first()
            .map(|d| Embedding::new(d.embedding.clone()))
            .ok_or_else(|| {
                crate::common::error::TianyanError::Custom("嵌入服务错误：未返回嵌入向量".to_string())
            })
    }

    /// 为单个文本生成嵌入向量（指定维度）。
    async fn embed_single_with_dimensions(
        &self,
        model: &str,
        text: &str,
        dimensions: usize,
    ) -> Result<Embedding> {
        let request = EmbeddingRequest::new(model, text).with_dimensions(dimensions);
        let response = self.embed(request).await?;
        response
            .data
            .first()
            .map(|d| Embedding::new(d.embedding.clone()))
            .ok_or_else(|| {
                crate::common::error::TianyanError::Custom("嵌入服务错误：未返回嵌入向量".to_string())
            })
    }

    /// 为多个文本生成嵌入向量。
    async fn embed_batch(&self, model: &str, texts: Vec<String>) -> Result<Vec<Embedding>> {
        let request = EmbeddingRequest::new_batch(model, texts);
        let response = self.embed(request).await?;
        let embeddings: Vec<_> = response
            .data
            .iter()
            .map(|d| Embedding::new(d.embedding.clone()))
            .collect();
        Ok(embeddings)
    }

    /// 获取模型的默认嵌入维度。
    fn embedding_dimension(&self, model: &str) -> usize;

    /// 将图像编码为嵌入向量（用于视觉相似性搜索）。
    ///
    /// 默认实现将图像编码为 base64 data URL，通过多模态嵌入模型生成向量。
    /// 适用于支持图像输入的嵌入模型（如 CLIP-based API）。
    async fn embed_image(&self, model: &str, image_data: &[u8]) -> Result<Embedding> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let base64_data = STANDARD.encode(image_data);
        let data_url = format!("data:image/jpeg;base64,{}", base64_data);
        self.embed_single(model, &data_url).await
    }
}

/// 视觉语言模型（VLM）服务 trait。
///
/// 此 trait 定义了能够理解图像并生成文本描述的视觉语言模型接口。
#[async_trait]
pub trait VlmService: Send + Sync {
    /// 分析图像并生成描述。
    async fn analyze_image(&self, request: VisionRequest) -> Result<VisionResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    use crate::common::error::Result;
    use crate::common::types::Message;
    use crate::model::types::{
        ChatChoice, ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse,
        EmbeddingRequest, EmbeddingResponse, ModelCapability, ModelInfo, ModelType, VisionChoice,
        VisionContent, VisionMessage, VisionRequest, VisionResponse,
    };

    /// MockVlmService: 用于测试的 VlmService 实现。
    pub(crate) struct MockVlmService;

    #[async_trait]
    impl VlmService for MockVlmService {
        async fn analyze_image(&self, _request: VisionRequest) -> Result<VisionResponse> {
            Ok(VisionResponse {
                id: "mock-vision".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock-vision".to_string(),
                choices: vec![VisionChoice {
                    index: 0,
                    message: VisionMessage {
                        role: "assistant".to_string(),
                        content: VisionContent::Text("模拟图像描述".to_string()),
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: crate::common::types::TokenUsage::default(),
            })
        }
    }

    /// MockServiceDiscovery: 用于测试的 ServiceDiscovery 实现。
    pub(crate) struct MockServiceDiscovery;

    #[async_trait]
    impl ServiceDiscovery for MockServiceDiscovery {
        async fn list_models(&self) -> Result<Vec<ModelInfo>> {
            Ok(vec![ModelInfo {
                id: "mock-model".to_string(),
                name: "Mock Model".to_string(),
                provider: "mock".to_string(),
                model_type: ModelType::Chat,
                max_context_length: 4096,
                capabilities: vec![ModelCapability::Chat, ModelCapability::Streaming],
            }])
        }

        async fn is_available(&self) -> bool {
            true
        }

        fn service_name(&self) -> &str {
            "mock-discovery"
        }
    }
}
