//! 模型服务的契约与数据（**实现已出 core**，ADR-049 期 2 序 6）。
//!
//! # 架构
//!
//! 本模块只留「内核必须认识的东西」：
//!
//! - [traits](crate::model::traits)：S 级服务契约（`ChatService` / `EmbeddingService` /
//!   `VlmService` / `ServiceDiscovery`）——内核、通道层、其它插件都以 `Arc<dyn …>` 消费；
//! - [types](crate::model::types)：请求 / 响应载荷（出现在 S 级 trait 签名里）；
//! - [spec](crate::model::spec)：模型规格与上下文窗口（内核动态 `max_tokens`、
//!   `policy-context` 压缩窗口、通道层配置面板共用）；
//! - [retry](crate::model::retry)：请求重试策略与可重试谓词（两个 provider 插件共用，
//!   ADR-014 的统一约定）；
//! - [assembly](crate::model::assembly)：provider 插件的**装配原语**（能力集合 +
//!   连通性探测契约）；
//! - `unconfigured`：能力未配置时的降级服务（未装配插件的库单测路径与插件内部共用同一
//!   句文案）；
//! - `usage`：嵌入用量上报契约（实现方在通道层、消费方在 provider-embed 插件）。
//!
//! # 实现去了哪里
//!
//! | 原位置 | 现位置 | 为什么 |
//! |---|---|---|
//! | `model/provider/{chat,vision,discovery,middleware}.rs` | `plugins/provider-llm` | 请求构造 / 流式解析 / 思考参数 / reasoning 回传全是 B 级策略：改这些不该重编内核 |
//! | `model/provider/embedding.rs`（+ 嵌入侧 middleware） | `plugins/provider-embed` | 同上（嵌入请求体与宽容 usage 解析） |
//! | `model/provider/client.rs` | `plugins/provider-client` | OpenAI 兼容客户端 + 状态码语义分类，被上面两个插件共用（一次构造、全路径生效） |
//! | `model/services.rs`（`ModelServices` 容器 + 构造器） | `server/src/model_services.rs` | core 内零生产使用者；按 D5 属装配职责，三种能力今天分属两个插件插槽 |
//! | `model/provider/vision.rs::infer_mime_type` | `knowledge/image/analyzer.rs` | 唯一消费者是知识导入（provider 侧从不调用），随消费者留 core 才不会让内核反向依赖插件 |
//!
//! 详见 [ADR-049](../../docs/architecture/decisions/049-microkernel-plugin-boundaries.md)
//! 「实施记录（期 2 序 6：provider-llm + provider-embed）」。

/// provider 插件的装配原语（能力集合 + 连通性探测契约，ADR-049 期 2 序 6）。
pub mod assembly;
/// Provider 请求重试策略（指数退避 + 最大重试次数，全局内置默认）。
pub mod retry;
/// 模型上下文规格与内置规格表。
pub mod spec;
mod traits;
/// 模型类型定义（请求/响应/工具）。
pub mod types;
/// 「模型能力未配置」的降级服务（两条装配路径共用的兜底）。
mod unconfigured;
/// 嵌入用量上报契约（实现方在通道层、消费方在插件）。
mod usage;

pub use traits::{ChatService, EmbeddingService, ServiceDiscovery, VlmService};

pub use assembly::{ConnectionProbe, ProviderEmbedServices, ProviderLlmServices};

/// 嵌入用量上报契约与装配层注入槽位（嵌入不走 AgentLoop，其 token 用量需单独入账）。
pub use usage::{EmbeddingUsageHandle, EmbeddingUsageSink};

/// 「模型能力未配置」的降级服务（未装配插件 / 配置缺能力时共用同一句错误文案）。
pub use unconfigured::{
    UnconfiguredChatService, UnconfiguredEmbeddingService, UnconfiguredVlmService,
};

pub use types::{
    embedding_dimension, ApiError, ApiErrorResponse, ChatChoice, ChatCompletionChunk,
    ChatCompletionRequest, ChatCompletionResponse, ChunkChoice, ContentPart, DeltaContent,
    EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse, FunctionCall,
    FunctionDefinition, ImageUrl, ModelCapability, ModelInfo, ModelProvider, ModelType, ToolCall,
    ToolCallType, ToolChoice, ToolChoiceFunction, ToolDefinition, ToolType, VisionChoice,
    VisionContent, VisionMessage, VisionRequest, VisionResponse,
};

/// 聊天服务的共享引用类型别名。
pub type SharedChatService = std::sync::Arc<dyn ChatService>;

// 测试替身：`cfg(test)`（core 单测）或 `test-utils` 特性（插件 crate 的单元测试，
// 见 `crate::test_utils` 的说明）。
#[cfg(any(test, feature = "test-utils"))]
pub use traits::MockChatService;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _provider = ModelProvider::OpenAI;
    }
}
