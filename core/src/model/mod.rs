//! 模型服务 trait 和实现。
//!
//! 本模块定义了与语言模型、嵌入服务和视觉语言模型交互的核心 trait。
//!
//! # 架构
//!
//! 模型服务层组织如下：
//!
//! - [`traits`]: 核心服务 trait（ChatService, EmbeddingService, VlmService, VisionEncoder）
//! - [`types`]: 请求和响应的类型定义
//! - [`config`]: 模型服务配置（ModelConfig）
//! - [`provider`]: 基于 async-openai 的 OpenAI 兼容 API 客户端实现
//! - [`services`]: 从配置构建一组已包装（日志）服务的简单容器
//!
//! # 示例
//!
//! ```rust,ignore
//! use tianyan::model::{AsyncOpenAIClient, ModelConfig, ChatService};
//! use tianyan::model::types::{ChatCompletionRequest, ModelProvider};
//! use tianyan::common::types::Message;
//!
//! async fn example() -> tianyan::common::error::Result<()> {
//!     let client = AsyncOpenAIClient::new(
//!         ModelConfig::new(ModelProvider::OpenAI, "your-api-key")
//!             .with_chat_model("gpt-4")
//!     )?;
//!
//!     let request = ChatCompletionRequest::new(
//!         "gpt-4",
//!         vec![Message::user("Hello, world!")]
//!     );
//!
//!     let response = client.chat_completion(request).await?;
//!     println!("响应：{:?}", response.choices[0].message.content);
//!     Ok(())
//! }
//! ```

mod config;
pub(crate) mod provider;
mod services;
mod traits;
pub mod types;

pub use traits::{ChatService, EmbeddingService, ServiceDiscovery, VisionEncoder, VlmService};

pub use types::{
    embedding_dimension, ApiError, ApiErrorResponse, ChatChoice, ChatCompletionChunk,
    ChatCompletionRequest, ChatCompletionResponse, ChunkChoice, ContentPart, DeltaContent,
    EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse, FunctionCall,
    FunctionDefinition, ImageUrl, ModelCapability, ModelInfo, ModelProvider, ModelType, ToolCall,
    ToolCallType, ToolChoice, ToolChoiceFunction, ToolDefinition, ToolType, VisionChoice,
    VisionContent, VisionMessage, VisionRequest, VisionResponse,
};

pub use config::ModelConfig;

pub use services::ModelServices;

/// 基于 async-openai 的 OpenAI 兼容客户端。
pub use provider::AsyncOpenAIClient;

/// 模型服务的共享引用类型别名。
pub type SharedChatService = std::sync::Arc<dyn ChatService>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _provider = ModelProvider::OpenAI;
    }
}
