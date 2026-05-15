use async_trait::async_trait;

use crate::common::error::Result;
use crate::model::traits::ServiceDiscovery;
use crate::model::types::{ModelCapability, ModelInfo, ModelType};

use super::client::AsyncOpenAIClient;

const MODEL_PREFIXES: &[&str] = &["gpt", "text-embedding", "claude", "deepseek", "qwen"];
const DEFAULT_CHAT_CONTEXT_LENGTH: usize = 8192;

#[async_trait]
impl ServiceDiscovery for AsyncOpenAIClient {
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let response = self.client.models().list().await.map_err(|e| {
            crate::common::error::TianyanError::ModelService(format!("列出模型失败：{}", e))
        })?;

        let models: Vec<ModelInfo> = response
            .data
            .into_iter()
            .filter(|m| MODEL_PREFIXES.iter().any(|prefix| m.id.starts_with(prefix)))
            .map(|m| {
                let model_type = if m.id.contains("embedding") {
                    ModelType::Embedding
                } else if m.id.contains("vision") || m.id.contains("gpt-4") {
                    ModelType::Vision
                } else {
                    ModelType::Chat
                };

                let capabilities = match model_type {
                    ModelType::Chat => {
                        vec![ModelCapability::Chat, ModelCapability::Streaming]
                    }
                    ModelType::Embedding => vec![],
                    ModelType::Vision => vec![
                        ModelCapability::Chat,
                        ModelCapability::Vision,
                        ModelCapability::Streaming,
                    ],
                };

                ModelInfo {
                    id: m.id.clone(),
                    name: m.id,
                    provider: self.service_name.clone(),
                    model_type,
                    max_context_length: DEFAULT_CHAT_CONTEXT_LENGTH,
                    capabilities,
                }
            })
            .collect();

        Ok(models)
    }

    async fn is_available(&self) -> bool {
        self.client.models().list().await.is_ok()
    }

    fn service_name(&self) -> &str {
        &self.service_name
    }
}
