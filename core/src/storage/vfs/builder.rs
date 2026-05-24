//! VFS 构建器、初始化函数和测试。

use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::config::StorageConfig;
use crate::model::EmbeddingService;
use crate::storage::traits::{StorageBackend, VectorStorage, VfsCore, VirtualFileSystem};

use super::VirtualFileSystemImpl;

/// 用于创建虚拟文件系统实例的构建器。
pub struct VirtualFileSystemBuilder {
    config: Option<StorageConfig>,
    storage: Option<Arc<dyn StorageBackend>>,
    vector_storage: Option<Arc<dyn VectorStorage>>,
    embedding_service: Option<Arc<dyn EmbeddingService>>,
    embedding_model: Option<String>,
}

impl VirtualFileSystemBuilder {
    /// 创建新的构建器。
    pub fn new() -> Self {
        Self {
            config: None,
            storage: None,
            vector_storage: None,
            embedding_service: None,
            embedding_model: None,
        }
    }

    /// 设置存储配置。
    pub fn with_config(mut self, config: StorageConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// 设置存储后端。
    pub fn with_storage(mut self, storage: Arc<dyn StorageBackend>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// 设置向量存储。
    pub fn with_vector_storage(mut self, vector_storage: Arc<dyn VectorStorage>) -> Self {
        self.vector_storage = Some(vector_storage);
        self
    }

    /// 设置嵌入服务。
    pub fn with_embedding_service(
        mut self,
        service: Arc<dyn EmbeddingService>,
        model: impl Into<String>,
    ) -> Self {
        self.embedding_service = Some(service);
        self.embedding_model = Some(model.into());
        self
    }

    /// 构建虚拟文件系统。
    pub fn build(self) -> Result<VirtualFileSystemImpl> {
        let config = self.config.unwrap_or_default();
        let storage = self
            .storage
            .ok_or_else(|| TianyanError::Internal("需要存储后端".to_string()))?;
        let vector_storage = self
            .vector_storage
            .ok_or_else(|| TianyanError::Internal("需要向量存储".to_string()))?;

        let mut vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);

        if let (Some(service), Some(model)) = (self.embedding_service, self.embedding_model) {
            vfs.set_embedding_service(service, model);
        }

        Ok(vfs)
    }
}

impl Default for VirtualFileSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// 初始化 VFS 并确保目录结构存在。
pub async fn initialize_vfs(
    storage: Arc<dyn StorageBackend>,
    vector_storage: Arc<dyn VectorStorage>,
    config: StorageConfig,
) -> Result<Arc<dyn VirtualFileSystem>> {
    let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);

    vfs.initialize().await?;
    ensure_vfs_structure(&vfs).await?;

    Ok(Arc::new(vfs))
}

/// 确保 VFS 目录结构和默认文件存在。
pub async fn ensure_vfs_structure(vfs: &dyn VirtualFileSystem) -> Result<()> {
    let namespaces = [
        ContextNamespace::User,
        ContextNamespace::Session,
        ContextNamespace::Memory,
        ContextNamespace::Knowledge,
        ContextNamespace::Agent,
        ContextNamespace::Skill,
    ];

    for ns in namespaces {
        let uri = TianyanUri::new(ns, vec![]);
        if !vfs.exists(&uri).await? {
            vfs.create_directory(&uri).await?;
            tracing::debug!("已创建 VFS 命名空间： {}", uri);
        }
    }

    let soul_uri = crate::common::types::AgentPath::Soul.uri();
    if !vfs
        .has_content(&soul_uri, ContentLevel::Detail)
        .await
        .unwrap_or(false)
    {
        let default_prompt = include_str!("../../agent/default_soul.md");
        vfs.create_file(&soul_uri).await?;
        vfs.write(&soul_uri, ContentLevel::Detail, default_prompt)
            .await?;
        tracing::info!("已创建默认核心提示词： tianyan://agent/soul");
    }

    let learned_uri = crate::common::types::AgentPath::Learned.uri();
    if !vfs.exists(&learned_uri).await? {
        vfs.create_directory(&learned_uri).await?;
        tracing::info!("已创建学习规则目录： tianyan://agent/learned");
    }

    let user_uri = TianyanUri::new(ContextNamespace::User, vec![]);
    if !vfs
        .has_content(&user_uri, ContentLevel::Detail)
        .await
        .unwrap_or(false)
    {
        vfs.write(&user_uri, ContentLevel::Detail, "# 用户档案\n\n")
            .await?;
        tracing::info!("已创建默认用户档案： tianyan://user");
    }

    tracing::info!("VFS 目录结构初始化完成");
    Ok(())
}
