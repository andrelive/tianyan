//! 模型服务 trait 和实现。
//!
//! 本模块定义了与语言模型、嵌入服务和视觉语言模型交互的核心 trait。
//!
//! # 架构
//!
//! 模型服务层组织如下：
//!
//! - [traits](crate::model::traits): 核心服务 trait（ChatService, EmbeddingService, VlmService）
//! - [types](crate::model::types): 请求和响应的类型定义
//! - [config](crate::config): 模型服务配置（ModelConfig）
//! - [provider](crate::model::provider): 基于 async-openai 的 OpenAI 兼容 API 客户端实现
//! - [services](crate::model::services): 从配置构建一组已包装（日志）服务的简单容器
//!
//! # 示例
//!
//! ```rust,ignore
//! use tianyan::model::{AsyncOpenAIClient, ChatService};
//! use tianyan::config::ProviderConfig;
//! use tianyan::model::types::ChatCompletionRequest;
//! use tianyan::common::types::Message;
//!
//! async fn example() -> tianyan::common::error::Result<()> {
//!     let client = AsyncOpenAIClient::new(
//!         "openai",
//!         "https://api.openai.com/v1",
//!         "your-api-key",
//!         60,
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

pub(crate) mod provider;
/// 模型上下文规格与内置规格表。
/// Provider 请求重试策略（指数退避 + 最大重试次数，全局内置默认）。
pub mod retry;
mod services;
pub mod spec;
mod traits;
/// 模型类型定义（请求/响应/工具）。
pub mod types;

pub use traits::{ChatService, EmbeddingService, ServiceDiscovery, VlmService};

/// 嵌入用量上报契约与装配层注入槽位（嵌入不走 AgentLoop，其 token 用量需单独入账）。
pub use provider::middleware::{EmbeddingUsageHandle, EmbeddingUsageSink};

pub use types::{
    embedding_dimension, ApiError, ApiErrorResponse, ChatChoice, ChatCompletionChunk,
    ChatCompletionRequest, ChatCompletionResponse, ChunkChoice, ContentPart, DeltaContent,
    EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse, FunctionCall,
    FunctionDefinition, ImageUrl, ModelCapability, ModelInfo, ModelProvider, ModelType, ToolCall,
    ToolCallType, ToolChoice, ToolChoiceFunction, ToolDefinition, ToolType, VisionChoice,
    VisionContent, VisionMessage, VisionRequest, VisionResponse,
};

pub use services::ModelServices;

/// 基于 async-openai 的 OpenAI 兼容客户端。
pub use provider::AsyncOpenAIClient;

/// 聊天服务的共享引用类型别名。
pub type SharedChatService = std::sync::Arc<dyn ChatService>;

#[cfg(test)]
pub use traits::MockChatService;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _provider = ModelProvider::OpenAI;
    }
}
