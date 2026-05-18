use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::common::error::Result;
use crate::common::types::Message;
use crate::model::traits::{ChatService, EmbeddingService, VlmService};
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest,
    EmbeddingResponse, VisionRequest, VisionResponse,
};

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

pub struct LoggedEmbeddingService<T: EmbeddingService>(pub T);

#[async_trait]
impl<T: EmbeddingService> EmbeddingService for LoggedEmbeddingService<T> {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        info!(
            model = %request.model,
            "embed called"
        );
        let result = self.0.embed(request).await;
        match &result {
            Ok(resp) => info!(
                model = %resp.model,
                count = resp.data.len(),
                "embed succeeded"
            ),
            Err(e) => error!(
                error = %e,
                "embed failed"
            ),
        }
        result
    }

    fn embedding_dimension(&self, model: &str) -> usize {
        self.0.embedding_dimension(model)
    }
}

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
