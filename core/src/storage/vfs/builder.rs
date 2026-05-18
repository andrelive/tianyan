//! VFS 构建器、初始化函数和测试。

use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::config::StorageConfig;
use crate::model::EmbeddingService;
#[allow(unused_imports)]
use crate::storage::traits::{ContentLoader, StorageBackend, VectorStorage, VirtualFileSystem};

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
        vfs.write_content(&soul_uri, default_prompt).await?;
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
        vfs.write_content(&user_uri, "# 用户档案\n\n").await?;
        tracing::info!("已创建默认用户档案： tianyan://user");
    }

    tracing::info!("VFS 目录结构初始化完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;
    use crate::storage::local::LocalStorageBackend;
    use crate::storage::traits::VectorStorage;
    use crate::storage::types::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType};
    use tempfile::tempdir;

    pub struct MockVectorStorage;

    impl MockVectorStorage {
        pub fn new() -> Self {
            Self
        }
    }

    #[async_trait::async_trait]
    impl VectorStorage for MockVectorStorage {
        async fn initialize(&self) -> crate::common::error::Result<()> {
            Ok(())
        }

        async fn upsert_point(&self, _point: &VectorPoint) -> crate::common::error::Result<()> {
            Ok(())
        }

        async fn delete_point(&self, _id: &str) -> crate::common::error::Result<()> {
            Ok(())
        }

        async fn search(
            &self,
            _query: VectorSearchQuery,
        ) -> crate::common::error::Result<Vec<VectorSearchResult>> {
            Ok(vec![])
        }

        async fn get_point(&self, _id: &str) -> crate::common::error::Result<Option<VectorPoint>> {
            Ok(None)
        }

        async fn update_vector(
            &self,
            _uri: &TianyanUri,
            _vector_type: VectorType,
            _vector: &[f32],
        ) -> crate::common::error::Result<()> {
            Ok(())
        }

        async fn count_points(&self) -> crate::common::error::Result<usize> {
            Ok(0)
        }

        async fn clear(&self) -> crate::common::error::Result<()> {
            Ok(())
        }
    }

    async fn create_test_vfs() -> VirtualFileSystemImpl {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig::default();
        config.data_dir = dir.path().into();
        let storage = Arc::new(LocalStorageBackend::new(config.clone()));
        let vector_storage: Arc<dyn VectorStorage> = Arc::new(MockVectorStorage::new());
        VirtualFileSystemImpl::new(storage, vector_storage, config)
    }

    #[tokio::test]
    async fn test_vfs_initialize() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();
    }

    #[tokio::test]
    async fn test_vfs_create_directory() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let uri = TianyanUri::new(ContextNamespace::User, vec!["test_dir".to_string()]);
        let entry = vfs.create_directory(&uri).await.unwrap();

        assert!(entry.is_directory());
        assert!(vfs.exists(&uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_vfs_create_file() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["test_dir".to_string(), "test_file".to_string()],
        );
        let entry = vfs.create_file(&uri).await.unwrap();

        assert!(!entry.is_directory());
        assert!(vfs.exists(&uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_vfs_write_read_content() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
        vfs.create_file(&uri).await.unwrap();

        vfs.write_content(&uri, "测试详情").await.unwrap();

        let detail_content = vfs.read_content(&uri, ContentLevel::Detail).await.unwrap();
        assert_eq!(detail_content, "测试详情");
    }

    #[tokio::test]
    async fn test_vfs_delete() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
        vfs.create_file(&uri).await.unwrap();

        assert!(vfs.exists(&uri).await.unwrap());

        vfs.delete(&uri).await.unwrap();
        assert!(!vfs.exists(&uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_vfs_list() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let parent_uri = TianyanUri::new(ContextNamespace::User, vec!["parent".to_string()]);
        vfs.create_directory(&parent_uri).await.unwrap();

        for i in 0..3 {
            let child_uri = parent_uri.append(&format!("child_{}", i));
            vfs.create_file(&child_uri).await.unwrap();
        }

        let entries = vfs.list(&parent_uri).await.unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn test_vfs_move() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let source = TianyanUri::new(ContextNamespace::User, vec!["source".to_string()]);
        let dest = TianyanUri::new(ContextNamespace::User, vec!["dest".to_string()]);

        vfs.create_file(&source).await.unwrap();
        vfs.write_content(&source, "测试内容").await.unwrap();

        vfs.move_entry(&source, &dest).await.unwrap();

        assert!(!vfs.exists(&source).await.unwrap());
        assert!(vfs.exists(&dest).await.unwrap());

        let content = vfs.read_content(&dest, ContentLevel::Detail).await.unwrap();
        assert_eq!(content, "测试内容");
    }

    #[tokio::test]
    async fn test_vfs_copy() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let source = TianyanUri::new(ContextNamespace::User, vec!["source".to_string()]);
        let dest = TianyanUri::new(ContextNamespace::User, vec!["dest".to_string()]);

        vfs.create_file(&source).await.unwrap();
        vfs.write_content(&source, "测试内容").await.unwrap();

        vfs.copy_entry(&source, &dest).await.unwrap();

        assert!(vfs.exists(&source).await.unwrap());
        assert!(vfs.exists(&dest).await.unwrap());

        let content = vfs.read_content(&dest, ContentLevel::Detail).await.unwrap();
        assert_eq!(content, "测试内容");
    }

    #[tokio::test]
    async fn test_content_loader() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
        vfs.create_file(&uri).await.unwrap();
        vfs.write_content(&uri, "测试内容").await.unwrap();

        let content_loader: &dyn ContentLoader = &vfs;
        let content = content_loader
            .load_content(&uri, ContentLevel::Detail)
            .await
            .unwrap();
        assert_eq!(content, "测试内容");

        let has_content = content_loader
            .has_content(&uri, ContentLevel::Detail)
            .await
            .unwrap();
        assert!(has_content);
    }

    #[test]
    fn test_validate_uri() {
        let valid_uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
        assert!(VirtualFileSystemImpl::validate_uri(&valid_uri).is_ok());

        let root_uri = TianyanUri::new(ContextNamespace::User, vec![]);
        assert!(VirtualFileSystemImpl::validate_uri(&root_uri).is_ok());
    }

    #[tokio::test]
    async fn test_search_by_namespace() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let session_uri = TianyanUri::new(ContextNamespace::Session, vec!["session1".to_string()]);
        vfs.create_file(&session_uri).await.unwrap();
        vfs.write_content(&session_uri, "会话测试内容")
            .await
            .unwrap();

        let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec!["memory1".to_string()]);
        vfs.create_file(&memory_uri).await.unwrap();
        vfs.write_content(&memory_uri, "记忆测试内容")
            .await
            .unwrap();

        let knowledge_uri =
            TianyanUri::new(ContextNamespace::Knowledge, vec!["knowledge1".to_string()]);
        vfs.create_file(&knowledge_uri).await.unwrap();
        vfs.write_content(&knowledge_uri, "知识测试内容")
            .await
            .unwrap();

        let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["skill1".to_string()]);
        vfs.create_file(&skill_uri).await.unwrap();
        vfs.write_content(&skill_uri, "技能测试内容").await.unwrap();

        let session_results = vfs
            .search_by_namespace(ContextNamespace::Session, "测试", 10)
            .await
            .unwrap();
        assert!(!session_results.is_empty());
        assert!(session_results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Session));

        let memory_results = vfs
            .search_by_namespace(ContextNamespace::Memory, "测试", 10)
            .await
            .unwrap();
        assert!(!memory_results.is_empty());
        assert!(memory_results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Memory));

        let knowledge_results = vfs
            .search_by_namespace(ContextNamespace::Knowledge, "测试", 10)
            .await
            .unwrap();
        assert!(!knowledge_results.is_empty());
        assert!(knowledge_results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Knowledge));

        let skill_results = vfs
            .search_by_namespace(ContextNamespace::Skill, "测试", 10)
            .await
            .unwrap();
        assert!(!skill_results.is_empty());
        assert!(skill_results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Skill));
    }

    #[tokio::test]
    async fn test_search_session() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let session_uri =
            TianyanUri::new(ContextNamespace::Session, vec!["session_test".to_string()]);
        vfs.create_file(&session_uri).await.unwrap();
        vfs.write_content(&session_uri, "会话搜索测试")
            .await
            .unwrap();

        let content = vfs
            .read_content(&session_uri, ContentLevel::Detail)
            .await
            .unwrap();
        assert_eq!(content, "会话搜索测试");

        let results = vfs.search_session("搜索", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Session));
    }

    #[tokio::test]
    async fn test_search_memory() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec!["memory_test".to_string()]);
        vfs.create_file(&memory_uri).await.unwrap();
        vfs.write_content(&memory_uri, "记忆搜索测试")
            .await
            .unwrap();

        let results = vfs.search_memory("搜索", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Memory));
    }

    #[tokio::test]
    async fn test_search_knowledge() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let knowledge_uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["knowledge_test".to_string()],
        );
        vfs.create_file(&knowledge_uri).await.unwrap();
        vfs.write_content(&knowledge_uri, "知识搜索测试")
            .await
            .unwrap();

        let results = vfs.search_knowledge("搜索", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Knowledge));
    }

    #[tokio::test]
    async fn test_search_skill() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["skill_test".to_string()]);
        vfs.create_file(&skill_uri).await.unwrap();
        vfs.write_content(&skill_uri, "技能搜索测试").await.unwrap();

        let results = vfs.search_skill("搜索", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .any(|r| r.uri.namespace() == ContextNamespace::Skill));
    }
}
