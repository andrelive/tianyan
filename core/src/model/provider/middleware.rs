use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::common::error::Result;
use crate::common::types::{Embedding, Message};
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

    async fn embed_single(&self, model: &str, text: &str) -> Result<Embedding> {
        info!(
            model = %model,
            text_len = text.len(),
            "embed_single called"
        );
        let result = self.0.embed_single(model, text).await;
        match &result {
            Ok(_) => info!(model = %model, "embed_single succeeded"),
            Err(e) => error!(error = %e, "embed_single failed"),
        }
        result
    }

    async fn embed_single_with_dimensions(
        &self,
        model: &str,
        text: &str,
        dimensions: usize,
    ) -> Result<Embedding> {
        info!(
            model = %model,
            dimensions = dimensions,
            text_len = text.len(),
            "embed_single_with_dimensions called"
        );
        let result = self.0.embed_single_with_dimensions(model, text, dimensions).await;
        match &result {
            Ok(_) => info!(model = %model, "embed_single_with_dimensions succeeded"),
            Err(e) => error!(error = %e, "embed_single_with_dimensions failed"),
        }
        result
    }

    async fn embed_batch(&self, model: &str, texts: Vec<String>) -> Result<Vec<Embedding>> {
        info!(
            model = %model,
            count = texts.len(),
            "embed_batch called"
        );
        let result = self.0.embed_batch(model, texts).await;
        match &result {
            Ok(embeddings) => info!(
                model = %model,
                count = embeddings.len(),
                "embed_batch succeeded"
            ),
            Err(e) => error!(error = %e, "embed_batch failed"),
        }
        result
    }

    async fn embed_image(&self, model: &str, image_data: &[u8]) -> Result<Embedding> {
        info!(
            model = %model,
            image_len = image_data.len(),
            "embed_image called"
        );
        let result = self.0.embed_image(model, image_data).await;
        match &result {
            Ok(_) => info!(model = %model, "embed_image succeeded"),
            Err(e) => error!(error = %e, "embed_image failed"),
        }
        result
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
