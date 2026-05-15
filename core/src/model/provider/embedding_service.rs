use async_openai::types::embeddings::{
    CreateEmbeddingRequestArgs, EmbeddingInput as OaEmbeddingInput,
};
use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::error::TianyanError;
use crate::common::types::TokenUsage;
use crate::model::traits::EmbeddingService;
use crate::model::types::{EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse};

use super::client::AsyncOpenAIClient;

#[async_trait]
impl EmbeddingService for AsyncOpenAIClient {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        let oa_input = match request.input {
            EmbeddingInput::Single(text) => OaEmbeddingInput::String(text),
            EmbeddingInput::Multiple(texts) => OaEmbeddingInput::StringArray(texts),
        };

        let mut builder = CreateEmbeddingRequestArgs::default();
        builder.model(request.model);
        builder.input(oa_input);
        if let Some(dimensions) = request.dimensions {
            builder.dimensions(dimensions as u32);
        }

        let oa_request = builder
            .build()
            .map_err(|e| TianyanError::EmbeddingService(format!("构建嵌入请求失败: {}", e)))?;

        let response = self
            .client
            .embeddings()
            .create(oa_request)
            .await
            .map_err(|e| TianyanError::EmbeddingService(format!("嵌入请求失败: {}", e)))?;

        let data = response
            .data
            .into_iter()
            .map(|d| EmbeddingData {
                index: d.index as usize,
                embedding: d.embedding,
                object: d.object,
            })
            .collect();

        let prompt_tokens = response.usage.prompt_tokens as usize;
        let total_tokens = response.usage.total_tokens as usize;
        let usage = TokenUsage::new(prompt_tokens, total_tokens - prompt_tokens);

        Ok(EmbeddingResponse {
            object: response.object,
            data,
            model: response.model,
            usage,
        })
    }

    fn embedding_dimension(&self, model: &str) -> usize {
        crate::model::types::embedding_dimension(model)
    }

    fn service_name(&self) -> &str {
        &self.service_name
    }
}
