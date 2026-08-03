//! 测试工具（仅在 `cfg(test)` 时编译）。
//!
//! 提供共享的 `MockVectorStorage` 和 `create_test_vfs()`，
//! 避免在 `mod.rs` 和 `vfs/builder.rs` 之间重复定义。

use std::sync::Arc;

use tempfile::tempdir;

use crate::common::types::TianyanUri;
use crate::config::StorageConfig;
use crate::vfs::backend::LocalFileBackend;
use crate::vfs::types::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType};
use crate::vfs::vector::VectorStorage;
use crate::vfs::vfs_impl::VirtualFileSystemImpl;

/// Mock 向量存储实现，用于单元测试。
pub struct MockVectorStorage;

impl MockVectorStorage {
    /// 创建 Mock 向量存储实例。
    pub fn new() -> Self {
        Self
    }
}

impl Default for MockVectorStorage {
    fn default() -> Self {
        Self::new()
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

    async fn upsert_points(&self, _points: &[VectorPoint]) -> crate::common::error::Result<()> {
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
pub async fn create_test_vfs() -> VirtualFileSystemImpl {
    let dir = tempdir().unwrap();
    let config = StorageConfig {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let storage = Arc::new(LocalFileBackend::new(config.clone()));
    let vector_storage: Arc<dyn VectorStorage> = Arc::new(MockVectorStorage::new());
    VirtualFileSystemImpl::new(storage, vector_storage, config)
}
