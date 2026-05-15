use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::common::error::Result;
use crate::common::types::Message;
use crate::model::traits::{ModelService, ServiceDiscovery};
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
};

/// 日志装饰器：为每次调用添加 tracing 日志。
pub struct LoggedService<T: ModelService>(pub T);

#[async_trait]
impl<T: ModelService> ServiceDiscovery for LoggedService<T> {
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        self.0.list_models().await
    }

    async fn is_available(&self) -> bool {
        self.0.is_available().await
    }

    fn service_name(&self) -> &str {
        self.0.service_name()
    }
}

#[async_trait]
impl<T: ModelService> ModelService for LoggedService<T> {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        info!(
            service_name = %self.0.service_name(),
            model = %request.model,
            "chat_completion called"
        );
        let result = self.0.chat_completion(request).await;
        match &result {
            Ok(resp) => info!(
                service_name = %self.0.service_name(),
                choices = resp.choices.len(),
                "chat_completion succeeded"
            ),
            Err(e) => error!(
                service_name = %self.0.service_name(),
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
            service_name = %self.0.service_name(),
            model = %request.model,
            "chat_completion_stream called"
        );
        self.0.chat_completion_stream(request).await
    }

    async fn chat(&self, model: &str, messages: Vec<Message>) -> Result<String> {
        info!(
            service_name = %self.0.service_name(),
            model = %model,
            "chat called"
        );
        self.0.chat(model, messages).await
    }
}
