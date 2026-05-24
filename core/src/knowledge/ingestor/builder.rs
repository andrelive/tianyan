use crate::common::error::{Result, TianyanError};
use crate::model::{ChatService, EmbeddingService, VisionEncoder, VlmService};
use crate::vfs::{StorageBackend, VectorStorage};

use super::{IngestorConfig, KnowledgeIngestor};

/// 创建知识导入器的构建器。
pub struct KnowledgeIngestorBuilder<M, E, V, VE, S, VS>
where
    M: ChatService,
    E: EmbeddingService,
    V: VlmService,
    VE: VisionEncoder,
    S: StorageBackend,
    VS: VectorStorage,
{
    config: IngestorConfig,
    model_service: Option<M>,
    embedding_service: Option<E>,
    vlm_service: Option<V>,
    vision_encoder: Option<VE>,
    storage: Option<S>,
    vector_storage: Option<VS>,
}

impl<M, E, V, VE, S, VS> KnowledgeIngestorBuilder<M, E, V, VE, S, VS>
where
    M: ChatService + 'static,
    E: EmbeddingService + 'static,
    V: VlmService,
    VE: VisionEncoder,
    S: StorageBackend,
    VS: VectorStorage,
{
    /// 创建新的构建器。
    pub fn new() -> Self {
        Self {
            config: IngestorConfig::default(),
            model_service: None,
            embedding_service: None,
            vlm_service: None,
            vision_encoder: None,
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
    pub fn with_model_service(mut self, service: M) -> Self {
        self.model_service = Some(service);
        self
    }

    /// 设置嵌入服务。
    pub fn with_embedding_service(mut self, service: E) -> Self {
        self.embedding_service = Some(service);
        self
    }

    /// 设置 VLM 服务。
    pub fn with_vlm_service(mut self, service: V) -> Self {
        self.vlm_service = Some(service);
        self
    }

    /// 设置视觉编码器。
    pub fn with_vision_encoder(mut self, encoder: VE) -> Self {
        self.vision_encoder = Some(encoder);
        self
    }

    /// 设置存储后端。
    pub fn with_storage(mut self, storage: S) -> Self {
        self.storage = Some(storage);
        self
    }

    /// 设置向量存储。
    pub fn with_vector_storage(mut self, storage: VS) -> Self {
        self.vector_storage = Some(storage);
        self
    }

    /// 构建导入器。
    pub fn build(self) -> Result<KnowledgeIngestor<M, E, V, VE, S, VS>> {
        let model_service = self
            .model_service
            .ok_or_else(|| TianyanError::Config("模型服务是必需的".to_string()))?;
        let embedding_service = self
            .embedding_service
            .ok_or_else(|| TianyanError::Config("嵌入服务是必需的".to_string()))?;
        let vlm_service = self
            .vlm_service
            .ok_or_else(|| TianyanError::Config("VLM 服务是必需的".to_string()))?;
        let vision_encoder = self
            .vision_encoder
            .ok_or_else(|| TianyanError::Config("视觉编码器是必需的".to_string()))?;
        let storage = self
            .storage
            .ok_or_else(|| TianyanError::Config("存储后端是必需的".to_string()))?;
        let vector_storage = self
            .vector_storage
            .ok_or_else(|| TianyanError::Config("向量存储是必需的".to_string()))?;

        KnowledgeIngestor::new(
            self.config,
            model_service,
            embedding_service,
            vlm_service,
            vision_encoder,
            storage,
            vector_storage,
        )
    }
}

impl<M, E, V, VE, S, VS> Default for KnowledgeIngestorBuilder<M, E, V, VE, S, VS>
where
    M: ChatService + 'static,
    E: EmbeddingService + 'static,
    V: VlmService,
    VE: VisionEncoder,
    S: StorageBackend,
    VS: VectorStorage,
{
    fn default() -> Self {
        Self::new()
    }
}
