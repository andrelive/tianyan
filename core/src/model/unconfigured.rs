//! 「模型能力未配置」的降级服务（ADR-049 期 2 序 6：从 `model::services` 提到 `model` 根）。
//!
//! 语义：任何调用返回 [`TianyanError::config`]（“应用未配置。请先完成配置向导。”）——
//! 服务端仍能启动（前端显示配置向导），只有真正调用缺失能力时才报错。
//!
//! ## 为什么这三件留 **core**
//!
//! 它们**不是** provider 的实现细节，而是「能力缺失」这一状态在**两条装配路径**上的
//! 共同兜底：
//!
//! 1. **未注入 provider 插件**：`create_app` / 嵌入式 / 库单测（约 60 处入口）走
//!    `AssemblyServices::default()`，`ModelServices` 仍需三件可用的服务对象；
//! 2. **插件已装配但配置里没有该能力**：首次启动、用户删了 provider、只配了聊天没配视觉。
//!
//! 两条路径必须给出**同一句文案、同一个错误类别**：前端向导的判定读的就是它，文案分叉
//! 会让「未配置」在两条路径上表现为两种可观测行为。搬进插件则路径 1 拿不到（插件与
//! 通道层互不相识，D2-1），复制一份则两份口径同时漂移。
//!
//! 依赖为零（只拼一个错误），因此留在 core 不会把任何 provider 实现拖回内核。

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::common::error::{Result, TianyanError};
use crate::model::traits::{ChatService, EmbeddingService, VlmService};
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest,
    EmbeddingResponse, VisionRequest, VisionResponse,
};

/// 未配置时的统一错误（文案是前端向导的判定依据，改动需同步 gui）。
fn unconfigured_error() -> TianyanError {
    TianyanError::config("应用未配置。请先完成配置向导。")
}

/// 未配置时的聊天服务（调用即报错）。
pub struct UnconfiguredChatService;

#[async_trait]
impl ChatService for UnconfiguredChatService {
    async fn chat_completion(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        Err(unconfigured_error())
    }

    async fn chat_completion_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        Err(unconfigured_error())
    }
}

/// 未配置时的嵌入服务（调用即报错）。
pub struct UnconfiguredEmbeddingService;

#[async_trait]
impl EmbeddingService for UnconfiguredEmbeddingService {
    async fn embed(&self, _request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        Err(unconfigured_error())
    }

    fn embedding_dimension(&self, _model: &str) -> usize {
        0
    }
}

/// 未配置时的视觉服务（调用即报错）。
pub struct UnconfiguredVlmService;

#[async_trait]
impl VlmService for UnconfiguredVlmService {
    async fn analyze_image(&self, _request: VisionRequest) -> Result<VisionResponse> {
        Err(unconfigured_error())
    }
}
