//! 核心模型服务 trait。

use async_trait::async_trait;
use tokio::sync::mpsc;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::common::error::Result;
use crate::common::types::{Embedding, Message};

use super::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest,
    EmbeddingResponse, ModelInfo, VisionRequest, VisionResponse,
};

/// 聊天补全模型服务 trait。
///
/// 此 trait 定义了与支持聊天补全的大语言模型
/// （如 OpenAI GPT-4、Claude、DeepSeek）交互的接口。
///
/// 继承 [`ServiceDiscovery`] 以提供模型列表、可用性检查和名称获取能力。
#[async_trait]
pub trait ModelService: ServiceDiscovery {
    /// 发送聊天补全请求。
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse>;

    /// 发送聊天补全请求（流式）。
    /// 默认实现使用非流式 API，然后在内部转换为流式输出。
    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        let response = self.chat_completion(request).await?;
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let content = response
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default();

            let chunk = ChatCompletionChunk {
                id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                object: "chat.completion.chunk".to_string(),
                created: chrono::Utc::now().timestamp(),
                model: response.model,
                choices: vec![super::types::ChunkChoice {
                    index: 0,
                    delta: super::types::DeltaContent {
                        role: Some(crate::common::types::MessageRole::Assistant),
                        content: Some(content),
                    },
                    finish_reason: Some("stop".to_string()),
                }],
            };

            let _ = tx.send(Ok(chunk)).await;
        });

        Ok(rx)
    }

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

/// 服务发现 trait。
///
/// 提供模型列表查询、可用性检查和名称获取的基础设施能力。
/// 独立于具体业务 trait（ModelService / EmbeddingService / VlmService），
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
                crate::common::error::TianyanError::EmbeddingService("未返回嵌入向量".to_string())
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
                crate::common::error::TianyanError::EmbeddingService("未返回嵌入向量".to_string())
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

    /// 获取服务名称。
    fn service_name(&self) -> &str;
}

/// 视觉语言模型（VLM）服务 trait。
///
/// 此 trait 定义了能够理解图像并生成文本描述的视觉语言模型接口。
#[async_trait]
pub trait VlmService: Send + Sync {
    /// 分析图像并生成描述。
    async fn analyze_image(&self, request: VisionRequest) -> Result<VisionResponse>;

    /// 分析图像。
    async fn analyze_image_base64(
        &self,
        model: &str,
        image_data: &[u8],
        prompt: &str,
    ) -> Result<String> {
        let base64_data = STANDARD.encode(image_data);
        // 根据图片字节推断 MIME 类型
        let mime_type = infer_mime_type(image_data);
        let data_url = format!("data:{};base64,{}", mime_type, base64_data);

        let message = super::types::VisionMessage {
            role: "user".to_string(),
            content: super::types::VisionContent::MultiPart(vec![
                super::types::ContentPart {
                    content_type: "text".to_string(),
                    text: Some(prompt.to_string()),
                    image_url: None,
                },
                super::types::ContentPart {
                    content_type: "image_url".to_string(),
                    text: None,
                    image_url: Some(super::types::ImageUrl {
                        url: data_url,
                        detail: None,
                    }),
                },
            ]),
        };

        let request = VisionRequest::new(model, vec![message]);
        let response = self.analyze_image(request).await?;
        Ok(response
            .choices
            .first()
            .map(|c| match &c.message.content {
                super::types::VisionContent::Text(text) => text.clone(),
                super::types::VisionContent::MultiPart(parts) => parts
                    .iter()
                    .filter_map(|p| p.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
            .unwrap_or_default())
    }

    /// 获取服务名称。
    fn service_name(&self) -> &str;
}

/// 视觉编码器 trait。
///
/// 此 trait 定义了将图像转换为向量表示以进行相似度搜索的视觉编码器接口。
#[async_trait]
pub trait VisionEncoder: Send + Sync {
    /// 将图像编码为向量表示。
    async fn encode_image(&self, image_data: &[u8]) -> Result<Embedding>;

    /// 将多个图像编码为向量表示。
    async fn encode_images(&self, images: &[&[u8]]) -> Result<Vec<Embedding>> {
        let mut embeddings = Vec::with_capacity(images.len());
        for image in images {
            embeddings.push(self.encode_image(image).await?);
        }
        Ok(embeddings)
    }

    /// 获取输出嵌入的维度。
    fn embedding_dimension(&self) -> usize;

    /// 获取编码器名称。
    fn encoder_name(&self) -> &str;
}

/// 根据图片字节数据推断 MIME 类型
fn infer_mime_type(data: &[u8]) -> &str {
    if data.len() >= 3 && &data[0..3] == b"\xFF\xD8\xFF" {
        "image/jpeg"
    } else if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1A\n" {
        "image/png"
    } else if data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
        "image/gif"
    } else if data.len() >= 2 && &data[0..2] == b"BM" {
        "image/bmp"
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png" // safe fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    struct MockModelService;

    #[async_trait]
    impl ServiceDiscovery for MockModelService {
        async fn list_models(&self) -> Result<Vec<ModelInfo>> {
            Ok(vec![])
        }

        async fn is_available(&self) -> bool {
            true
        }

        fn service_name(&self) -> &str {
            "mock"
        }
    }

    #[async_trait]
    impl ModelService for MockModelService {
        async fn chat_completion(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                id: "test".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "test-model".to_string(),
                choices: vec![],
                usage: crate::common::types::TokenUsage::default(),
            })
        }
    }
}
