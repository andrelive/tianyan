//! 聊天 / 视觉服务的日志装饰（原 `core/src/model/provider/middleware.rs` 的上半部分）。
//!
//! 迁移时**按能力拆文件**：嵌入侧的装饰（`LoggedEmbeddingService` + 查询缓存）随
//! `embedding.rs` 进 `tianyan-provider-embed`，用量上报契约（`EmbeddingUsageSink`）提到
//! core 的 `model::usage`（实现方在通道层、消费方在插件）。本文件只剩聊天与视觉
//! 两条日志装饰，逻辑逐字未改（日志字段与文案与迁移前一致）。

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info};

use tianyan::common::error::Result;
use tianyan::common::types::Message;
use tianyan::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, VisionRequest,
    VisionResponse,
};
use tianyan::model::{ChatService, VlmService};

/// 带日志记录的聊天服务包装。
pub struct LoggedService<T: ChatService>(pub T);

#[async_trait]
impl<T: ChatService> ChatService for LoggedService<T> {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        info!(
            model = %request.model,
            "chat_completion called"
        );
        let result = self.0.chat_completion(request).await;
        match &result {
            Ok(resp) => info!(
                model = %resp.model,
                choices = resp.choices.len(),
                "chat_completion succeeded"
            ),
            Err(e) => error!(
                error = %e,
                "chat_completion failed"
            ),
        }
        result
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        info!(
            model = %request.model,
            "chat_completion_stream called"
        );
        self.0.chat_completion_stream(request).await
    }

    async fn chat(&self, model: &str, messages: Vec<Message>) -> Result<String> {
        info!(
            model = %model,
            "chat called"
        );
        self.0.chat(model, messages).await
    }
}

/// 带日志记录的 VLM 服务包装。
pub struct LoggedVlmService<T: VlmService>(pub T);

#[async_trait]
impl<T: VlmService> VlmService for LoggedVlmService<T> {
    async fn analyze_image(&self, request: VisionRequest) -> Result<VisionResponse> {
        info!(
            model = %request.model,
            "analyze_image called"
        );
        let result = self.0.analyze_image(request).await;
        match &result {
            Ok(resp) => info!(
                model = %resp.model,
                choices = resp.choices.len(),
                "analyze_image succeeded"
            ),
            Err(e) => error!(
                error = %e,
                "analyze_image failed"
            ),
        }
        result
    }
}
