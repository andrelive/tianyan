//! 模型服务 trait 和实现。
//!
//! 本模块定义了与语言模型、嵌入服务和视觉语言模型交互的核心 trait。
//!
//! # 架构
//!
//! 模型服务层组织如下：
//!
//! - [`traits`]: 核心服务 trait（ModelService, EmbeddingService, VlmService, VisionEncoder）
//! - [`types`][]: 请求、响应和配置的类型定义
//! - [`provider`][]: 基于 async-openai 的 OpenAI 兼容 API 客户端实现
//! - [`services`]: 从配置构建一组已包装（日志）服务的简单容器
//!
//! # 示例
//!
//! ```rust,ignore
//! use tianyan::model::{AsyncOpenAIClient, ModelConfig, ModelService, ChatCompletionRequest};
//! use tianyan::model::ModelProvider;
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

mod traits;
mod types;

pub mod provider;
mod services;

pub use traits::{EmbeddingService, ModelService, ServiceDiscovery, VisionEncoder, VlmService};

pub use types::*;

pub use provider::AsyncOpenAIClient;
pub use services::ModelServices;

/// 模型服务的共享引用类型别名。
pub type SharedModelService = std::sync::Arc<dyn ModelService>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _provider = ModelProvider::OpenAI;
    }
}
