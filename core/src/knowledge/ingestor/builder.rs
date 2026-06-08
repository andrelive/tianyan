use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::model::{ChatService, EmbeddingService, VlmService};
use crate::vfs::{backend::StorageBackend, VectorStorage};

use super::{IngestorConfig, KnowledgeIngestor};

/// 创建知识导入器的构建器。
pub struct KnowledgeIngestorBuilder {
    config: IngestorConfig,
    model_service: Option<Arc<dyn ChatService>>,
    embedding_service: Option<Arc<dyn EmbeddingService>>,
    vlm_service: Option<Arc<dyn VlmService>>,
    storage: Option<Arc<dyn StorageBackend>>,
    vector_storage: Option<Arc<dyn VectorStorage>>,
}

impl KnowledgeIngestorBuilder {
    /// 创建新的构建器。
    pub fn new() -> Self {
        Self {
            config: IngestorConfig::default(),
            model_service: None,
            embedding_service: None,
            vlm_service: None,
            storage: None,
            vector_storage: None,
        }
    }

    /// 设置配置。
    pub fn with_config(mut self, config: IngestorConfig) -> Self {
        self.config = config;
        self
    }

    /// 设置模型服务。
    pub fn with_model_service(mut self, service: Arc<dyn ChatService>) -> Self {
        self.model_service = Some(service);
        self
    }

    /// 设置嵌入服务。
    pub fn with_embedding_service(mut self, service: Arc<dyn EmbeddingService>) -> Self {
        self.embedding_service = Some(service);
        self
    }

    /// 设置 VLM 服务。
    pub fn with_vlm_service(mut self, service: Arc<dyn VlmService>) -> Self {
        self.vlm_service = Some(service);
        self
    }

    /// 设置存储后端。
    pub fn with_storage(mut self, storage: Arc<dyn StorageBackend>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// 设置向量存储。
    pub fn with_vector_storage(mut self, storage: Arc<dyn VectorStorage>) -> Self {
        self.vector_storage = Some(storage);
        self
    }

    /// 构建导入器。
    pub fn build(self) -> Result<KnowledgeIngestor> {
        let model_service = self
            .model_service
            .ok_or_else(|| TianyanError::Config("模型服务是必需的".to_string()))?;
        let embedding_service = self
            .embedding_service
            .ok_or_else(|| TianyanError::Config("嵌入服务是必需的".to_string()))?;
        let vlm_service = self
            .vlm_service
            .ok_or_else(|| TianyanError::Config("VLM 服务是必需的".to_string()))?;
        let storage = self
            .storage
            .ok_or_else(|| TianyanError::Config("存储后端是必需的".to_string()))?;
        let vector_storage = self
            .vector_storage
            .ok_or_else(|| TianyanError::Config("向量存储是必需的".to_string()))?;

        Ok(KnowledgeIngestor::new(
            self.config,
            model_service,
            embedding_service,
            vlm_service,
            storage,
            vector_storage,
        ))
    }
}

impl Default for KnowledgeIngestorBuilder {
    fn default() -> Self {
        Self::new()
    }
}
