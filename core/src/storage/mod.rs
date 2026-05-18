//! Tianyan 代理系统的存储模块。
//!
//! 本模块提供统一上下文存储系统的存储后端和虚拟文件系统实现。
//!
//! # 架构
//!
//! 存储系统由以下几个组件组成：
//!
//! - **StorageConfig**: 存储后端配置（定义在 `crate::config`）
//! - **UriMapper**: URI 到文件系统路径的映射
//! - **StorageBackend**: 存储后端 trait（本地文件系统、云存储等）
//! - **VectorStorage**: 向量存储后端 trait（仅支持 Qdrant。
//! - **VirtualFileSystem**: 统一的上下文存储接口
//! - **SummaryEngine**: 分层摘要生成
//!
//! # 示例
//!
//! ```no_run
//! use std::sync::Arc;
//! use tianyan::config::StorageConfig;
//! use tianyan::storage::{LocalStorageBackend, QdrantVectorStore, VirtualFileSystemImpl};
//!
//! async fn setup_storage() {
//!     let config = StorageConfig::default();
//!     let storage = Arc::new(LocalStorageBackend::new(config.clone()));
//!     let vector_storage = Arc::new(QdrantVectorStore::new(&config).unwrap());
//!     let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);
//! }
//! ```

mod extractor;
mod local;
mod qdrant;
mod summary;
mod summary_service;
mod traits;
mod types;
mod uri_mapper;
mod vfs;

// 重新导出公共 API
pub use crate::config::StorageConfig;
pub use extractor::{ExtractionConfig, MemoryExtractionService, MemoryExtractionTrait};
pub use local::LocalStorageBackend;
pub use qdrant::{QdrantVectorStore, QdrantVectorStoreBuilder};
pub use summary::{
    MockSummaryEngine, SummaryEngine, SummaryGenerator, SummaryLevel, TokenCounter,
    ABSTRACT_TOKEN_LIMIT, OVERVIEW_TOKEN_LIMIT,
};
pub use summary_service::{SummaryService, SummaryServiceConfig};
pub use traits::{
    ContentLoader, ContentMetadata, ContentStore, StorageBackend, VectorStorage, VfsCore,
    VfsFacade, VfsMetadata, VfsSearch, VirtualFileSystem,
};
pub use types::{
    CategoryStats, ContextEntry, DirectoryIndex, DirectoryStats, IndexEntry, StorageStats,
    TokenCounts, VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType,
    CURRENT_SCHEMA_VERSION,
};
pub use uri_mapper::UriMapper;
pub use vfs::{
    ensure_vfs_structure, initialize_vfs, VirtualFileSystemBuilder, VirtualFileSystemImpl,
};

/// 虚拟文件系统的共享引用类型别名。
///
/// 出现 ≥2 次，按项目规范提取。
pub type SharedVfs = std::sync::Arc<dyn VirtualFileSystem>;

#[cfg(test)]
pub use tests::MockVectorStorage;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
    use crate::storage::types::{VectorPoint, VectorSearchQuery, VectorSearchResult};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    /// Mock 向量存储实现，用于单元测试。
    pub struct MockVectorStorage;

    impl MockVectorStorage {
        /// 创建新的 Mock 向量存储。
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

    /// 创建测试 VFS 的辅助函数。
    async fn create_test_vfs() -> VirtualFileSystemImpl {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig::default();
        config.data_dir = dir.path().into();
        let storage = Arc::new(LocalStorageBackend::new(config.clone()));
        let vector_storage: Arc<dyn VectorStorage> = Arc::new(MockVectorStorage::new());
        VirtualFileSystemImpl::new(storage, vector_storage, config)
    }

    #[tokio::test]
    async fn test_full_workflow() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        // 创建文件
        let uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), "test_doc".to_string()],
        );
        let entry = vfs.create_file(&uri).await.unwrap();
        assert!(!entry.is_directory());

        // write_content 现在固定写入 Detail 层级
        vfs.write_content(&uri, "测试详细内容").await.unwrap();

        // 读取内容
        let detail_content = vfs.read_content(&uri, ContentLevel::Detail).await.unwrap();
        assert_eq!(detail_content, "测试详细内容");

        // 获取条目
        let entry = vfs.get_entry(&uri).await.unwrap();
        assert!(entry.has_content(ContentLevel::Detail));

        // 删除
        vfs.delete(&uri).await.unwrap();
        assert!(!vfs.exists(&uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_directory_operations() {
        let vfs = create_test_vfs().await;
        vfs.initialize().await.unwrap();

        // 创建嵌套目录
        let parent_uri = TianyanUri::new(ContextNamespace::User, vec!["preferences".to_string()]);
        vfs.create_directory(&parent_uri).await.unwrap();

        // 在目录中创建文件
        for i in 0..3 {
            let child_uri = parent_uri.append(&format!("pref_{}", i));
            vfs.create_file(&child_uri).await.unwrap();
            vfs.write_content(&child_uri, &format!("偏好设置 {}", i))
                .await
                .unwrap();
        }

        // 列出目录
        let entries = vfs.list(&parent_uri).await.unwrap();
        assert_eq!(entries.len(), 3);

        // 删除目录（应删除所有子条目）
        vfs.delete(&parent_uri).await.unwrap();
        assert!(!vfs.exists(&parent_uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_uri_mapper() {
        let mut config = StorageConfig::default();
        config.data_dir = PathBuf::from("/test");
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), "api_spec".to_string()],
        );

        // 测试路径映射 - 使用平台无关的比较
        let path = mapper.uri_to_path(&uri);
        let path_str = path.to_str().unwrap();
        // 注意：Windows 上路径使用反斜杠，在 Unix 上使用正斜杠
        assert!(
            path_str.contains("test")
                && path_str.contains("knowledge")
                && path_str.contains("documents")
                && path_str.contains("api_spec")
        );

        // 测试内容路径
        let abstract_path = mapper.get_abstract_path(&uri);
        assert!(abstract_path.ends_with("abstract.md"));
        assert!(abstract_path.to_str().unwrap().contains(".meta"));

        let overview_path = mapper.get_overview_path(&uri);
        assert!(overview_path.ends_with("overview.md"));
        assert!(overview_path.to_str().unwrap().contains(".meta"));

        let detail_path = mapper.get_detail_path(&uri);
        assert!(detail_path.ends_with("content.md"));
    }

    #[test]
    fn test_token_counter() {
        let counter = TokenCounter::new().unwrap();

        let text = "Hello, world! This is a test.";
        let count = counter.count_tokens(text);
        assert!(count > 0);

        // 测试截断
        let long_text = "word ".repeat(200);
        let truncated = counter.truncate_to_limit(&long_text, 50);
        assert!(counter.count_tokens(&truncated) <= 50);
    }

    #[test]
    fn test_context_entry() {
        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "basic".to_string()],
        );

        let mut entry = ContextEntry::new_file(uri.clone());
        assert!(!entry.is_directory());

        entry.set_content(ContentLevel::Abstract, "摘要".to_string());
        assert!(entry.has_content(ContentLevel::Abstract));
        assert_eq!(entry.get_content(ContentLevel::Abstract), Some("摘要"));

        let dir_entry = ContextEntry::new_directory(uri);
        assert!(dir_entry.is_directory());
    }

    #[test]
    fn test_vector_point() {
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["test".to_string()]);

        let entry = ContextEntry::new_file(uri.clone());
        let point = VectorPoint::from_entry(&entry);

        assert_eq!(point.uri().as_str(), uri.as_str());
        assert!(!point.is_directory());
    }
}
