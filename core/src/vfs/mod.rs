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
//! - **LocalFileBackend**: 本地文件系统存储后端
//! - **VectorStorage**: 向量存储后端 trait（支持 LanceDB 嵌入式向量存储。
//! - **VirtualFileSystem**: 统一的上下文存储接口
//! - **SummaryEngine**: 分层摘要生成
//!
//! # 示例
//!
//! ```no_run
//! use std::sync::Arc;
//! use tianyan::config::StorageConfig;
//! use tianyan::vfs::{LanceDbVectorStore, LocalFileBackend, VirtualFileSystemImpl};
//!
//! async fn setup_storage() {
//!     let config = StorageConfig::default();
//!     let storage = Arc::new(LocalFileBackend::new(config.clone()));
//!     let vector_storage = Arc::new(LanceDbVectorStore::new(&config).await.unwrap());
//!     let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);
//! }
//! ```

pub mod backend;
mod summary;
mod traits;
mod types;
mod uri_mapper;
mod vector;
mod vfs_impl;

#[cfg(test)]
mod test_utils;

// 重新导出公共 API
pub use crate::config::StorageConfig;
pub use backend::LocalFileBackend;
#[cfg(test)]
pub use summary::MockSummaryEngine;
pub use summary::{SummaryEngine, SummaryLevel, ABSTRACT_TOKEN_LIMIT, OVERVIEW_TOKEN_LIMIT};
pub use traits::{ContentMetadata, ContentStore, VfsCore, VfsSearch, VirtualFileSystem};
pub use types::{
    CategoryStats, ContextEntry, DirectoryIndex, DirectoryStats, IndexEntry, StorageStats,
    VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType, CURRENT_SCHEMA_VERSION,
};
pub use uri_mapper::UriMapper;
pub use vector::{LanceDbVectorStore, VectorStorage};
pub use vfs_impl::{VirtualFileSystemBuilder, VirtualFileSystemImpl};

/// 虚拟文件系统的共享引用类型别名。
///
/// 出现 ≥2 次，按项目规范提取。
pub type SharedVfs = std::sync::Arc<dyn VirtualFileSystem>;

#[cfg(test)]
pub use test_utils::MockVectorStorage;

#[cfg(test)]
mod tests {
    use super::test_utils::{create_test_vfs, MockVectorStorage};
    use super::*;
    use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
    use crate::vfs::types::{VectorPoint, VectorSearchQuery, VectorSearchResult};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

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
