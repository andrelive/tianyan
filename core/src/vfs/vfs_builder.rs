//! VFS 构建器、初始化函数和测试。

use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::config::StorageConfig;
use crate::model::EmbeddingService;
use crate::vfs::backend::StorageBackend;
use crate::vfs::vector::VectorStorage;

use super::VirtualFileSystemImpl;

/// 用于创建虚拟文件系统实例的构建器。
pub struct VirtualFileSystemBuilder {
    config: Option<StorageConfig>,
    storage: Option<Arc<dyn StorageBackend>>,
    vector_storage: Option<Arc<dyn VectorStorage>>,
    embedding_provider: Option<Arc<dyn EmbeddingService>>,
    embedding_model: Option<String>,
}

impl VirtualFileSystemBuilder {
    /// 创建新的构建器。
    pub fn new() -> Self {
        Self {
            config: None,
            storage: None,
            vector_storage: None,
            embedding_provider: None,
            embedding_model: None,
        }
    }

    /// 设置存储配置。
    pub fn with_config(mut self, config: StorageConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// 设置存储后端（LocalFileBackend / SqliteBackend 等 adapter）。
    pub fn with_storage(mut self, storage: Arc<dyn StorageBackend>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// 设置向量存储。
    pub fn with_vector_storage(mut self, vector_storage: Arc<dyn VectorStorage>) -> Self {
        self.vector_storage = Some(vector_storage);
        self
    }

    /// 设置嵌入服务（`model::EmbeddingService` 直接注入，无桥接层）。
    pub fn with_embedding_provider(
        mut self,
        provider: Arc<dyn EmbeddingService>,
        model: impl Into<String>,
    ) -> Self {
        self.embedding_provider = Some(provider);
        self.embedding_model = Some(model.into());
        self
    }

    /// 构建虚拟文件系统。
    pub fn build(self) -> Result<VirtualFileSystemImpl> {
        let config = self.config.unwrap_or_default();
        let storage = self
            .storage
            .ok_or_else(|| TianyanError::Custom("内部错误：需要存储后端".to_string()))?;
        let vector_storage = self
            .vector_storage
            .ok_or_else(|| TianyanError::Custom("内部错误：需要向量存储".to_string()))?;

        let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);

        if let (Some(provider), Some(model)) = (self.embedding_provider, self.embedding_model) {
            vfs.set_embedding_provider(provider, model);
        }

        Ok(vfs)
    }
}

impl Default for VirtualFileSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}
