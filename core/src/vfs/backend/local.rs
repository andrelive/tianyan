//! 本地文件系统存储后端实现。

use async_trait::async_trait;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::vfs::traits::StorageBackend;
use crate::vfs::types::{
    CategoryStats, ContextEntry, DirectoryIndex, DirectoryStats, IndexEntry, StorageStats,
};

use crate::config::StorageConfig;
use crate::vfs::uri_mapper::UriMapper;

/// 本地文件系统存储后端。
pub struct LocalStorageBackend {
    config: StorageConfig,
    mapper: UriMapper,
    /// 文件路径级别的写入锁，防止并发追加导致内容交错。
    write_locks: Arc<Mutex<std::collections::HashMap<std::path::PathBuf, Arc<Mutex<()>>>>>,
}

impl LocalStorageBackend {
    /// 创建新的本地存储后端。
    pub fn new(config: StorageConfig) -> Self {
        let mapper = UriMapper::new(config.clone());
        Self {
            config,
            mapper,
            write_locks: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 创建具有默认配置的本地存储后端。
    pub fn with_defaults() -> Self {
        Self::new(StorageConfig::default())
    }

    /// 确保目录存在，如需要则创建。
    fn ensure_dir(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            fs::create_dir_all(path).map_err(|e| {
                TianyanError::StorageBackend(format!("创建目录 {:?} 失败: {}", path, e))
            })?;
        }
        Ok(())
    }

    /// 读取文件内容为字符串。
    fn read_file(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path).map_err(|e| {
            TianyanError::StorageBackend(format!("打开文件 {:?} 失败: {}", path, e))
        })?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| {
            TianyanError::StorageBackend(format!("读取文件 {:?} 失败: {}", path, e))
        })?;
        Ok(content)
    }

    /// 将字符串内容写入文件。
    fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent)?;
        }
        let mut file = fs::File::create(path).map_err(|e| {
            TianyanError::StorageBackend(format!("创建文件 {:?} 失败: {}", path, e))
        })?;
        file.write_all(content.as_bytes()).map_err(|e| {
            TianyanError::StorageBackend(format!("写入文件 {:?} 失败: {}", path, e))
        })?;
        Ok(())
    }

    /// 获取指定路径的写入锁。
    async fn acquire_write_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.write_locks.lock().await;
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// 追加内容到文件（如文件不存在则创建）。
    async fn append_file_locked(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent)?;
        }

        let lock = self.acquire_write_lock(path).await;
        let _guard = lock.lock().await;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| {
                TianyanError::StorageBackend(format!("打开文件 {:?} 用于追加失败: {}", path, e))
            })?;
        file.write_all(content.as_bytes()).map_err(|e| {
            TianyanError::StorageBackend(format!("追加写入文件 {:?} 失败: {}", path, e))
        })?;
        Ok(())
    }

    /// 删除文件或目录。
    fn delete_path(&self, path: &Path) -> Result<()> {
        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|e| {
                TianyanError::StorageBackend(format!("删除目录 {:?} 失败: {}", path, e))
            })?;
        } else if path.exists() {
            fs::remove_file(path).map_err(|e| {
                TianyanError::StorageBackend(format!("删除文件 {:?} 失败: {}", path, e))
            })?;
        }
        Ok(())
    }

    /// 同步读取内容（内部辅助方法）。
    fn read_content_sync(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        let path = match level {
            ContentLevel::Abstract => self.mapper.get_abstract_path(uri),
            ContentLevel::Overview => self.mapper.get_overview_path(uri),
            ContentLevel::Detail => self.mapper.get_detail_path(uri),
        };

        if !path.exists() {
            return Err(TianyanError::EntryNotFound(format!(
                "{} 无 {:?} 层级内容",
                uri, level
            )));
        }

        self.read_file(&path)
    }

    /// 从文件系统生成目录索引。
    fn generate_directory_index_sync(&self, uri: &TianyanUri) -> Result<DirectoryIndex> {
        let path = self.mapper.uri_to_path(uri);
        let mut entries = Vec::new();
        let mut total_size: u64 = 0;

        if path.exists() && path.is_dir() {
            for entry in fs::read_dir(&path).map_err(|e| {
                TianyanError::StorageBackend(format!("读取目录 {:?} 失败: {}", path, e))
            })? {
                let entry = entry.map_err(|e| {
                    TianyanError::StorageBackend(format!("读取目录条目失败: {}", e))
                })?;

                let name = entry.file_name().to_string_lossy().to_string();

                if UriMapper::is_special_file(&entry.path()) {
                    continue;
                }

                let metadata = entry.metadata().map_err(|e| {
                    TianyanError::StorageBackend(format!(
                        "读取 {:?} 的元数据失败: {}",
                        entry.path(),
                        e
                    ))
                })?;

                let is_dir = metadata.is_dir();
                let size = metadata.len();
                total_size += size;

                let child_uri = uri.append(&name);
                let abstract_summary = self
                    .read_content_sync(&child_uri, ContentLevel::Abstract)
                    .ok();

                entries.push(IndexEntry {
                    name,
                    entry_type: if is_dir { "directory" } else { "file" }.to_string(),
                    abstract_summary,
                    tags: Vec::new(),
                    importance: 0.5,
                });
            }
        }

        let total_entries = entries.len();
        Ok(DirectoryIndex {
            schema_version: crate::vfs::types::CURRENT_SCHEMA_VERSION,
            updated_at: chrono::Utc::now(),
            entries,
            stats: DirectoryStats {
                total_entries,
                total_size,
            },
        })
    }

    /// 计算分类目录的统计信息。
    fn calculate_category_stats(&self, path: &Path) -> Result<CategoryStats> {
        let mut count = 0;
        let mut size: u64 = 0;

        fn walk_dir(path: &Path, count: &mut usize, size: &mut u64) -> Result<()> {
            if !path.exists() {
                return Ok(());
            }

            for entry in fs::read_dir(path)
                .map_err(|e| TianyanError::StorageBackend(format!("读取目录失败: {}", e)))?
            {
                let entry = entry
                    .map_err(|e| TianyanError::StorageBackend(format!("读取条目失败: {}", e)))?;

                let metadata = entry
                    .metadata()
                    .map_err(|e| TianyanError::StorageBackend(format!("读取元数据失败：{}", e)))?;

                if metadata.is_dir() {
                    walk_dir(&entry.path(), count, size)?;
                } else {
                    *count += 1;
                    *size += metadata.len();
                }
            }
            Ok(())
        }

        walk_dir(path, &mut count, &mut size)?;

        Ok(CategoryStats { count, size })
    }
}

#[async_trait]
impl StorageBackend for LocalStorageBackend {
    async fn initialize(&self) -> Result<()> {
        self.ensure_dir(&self.config.data_dir)?;

        for &namespace in ContextNamespace::ALL {
            let uri = TianyanUri::new(namespace, vec![]);
            let path = self.mapper.uri_to_path(&uri);
            self.ensure_dir(&path)?;
        }

        tracing::info!("已在 {:?} 初始化本地存储", self.config.data_dir);
        Ok(())
    }

    async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
        let path = self.mapper.uri_to_path(uri);
        Ok(path.exists())
    }

    async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        let path = self.mapper.uri_to_path(uri);

        if !path.exists() {
            return Err(TianyanError::EntryNotFound(uri.to_string()));
        }

        let is_directory = path.is_dir();

        let abstract_content = self.read_content_sync(uri, ContentLevel::Abstract).ok();
        let overview_content = self.read_content_sync(uri, ContentLevel::Overview).ok();
        let detail_content = self.read_content_sync(uri, ContentLevel::Detail).ok();

        let mut metadata = crate::common::types::EntryMetadata::new(uri.clone(), "unknown");
        metadata.is_directory = is_directory;

        Ok(ContextEntry {
            schema_version: crate::vfs::types::CURRENT_SCHEMA_VERSION,
            abstract_content,
            overview_content,
            detail_content,
            metadata,
        })
    }

    async fn write_entry(&self, entry: &ContextEntry) -> Result<()> {
        let path = self.mapper.uri_to_path(entry.uri());

        self.ensure_dir(&path)?;

        if let Some(ref content) = entry.abstract_content {
            self.write_content(entry.uri(), ContentLevel::Abstract, content)
                .await?;
        }

        if let Some(ref content) = entry.overview_content {
            self.write_content(entry.uri(), ContentLevel::Overview, content)
                .await?;
        }

        if let Some(ref content) = entry.detail_content {
            self.write_content(entry.uri(), ContentLevel::Detail, content)
                .await?;
        }

        tracing::debug!("已在 {:?} 写入条目", path);
        Ok(())
    }

    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()> {
        let path = self.mapper.uri_to_path(uri);

        if !path.exists() {
            return Err(TianyanError::EntryNotFound(uri.to_string()));
        }

        self.delete_path(&path)?;

        tracing::debug!("已删除 {:?} 处的条目", path);
        Ok(())
    }

    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        let path = self.mapper.uri_to_path(uri);

        if !path.exists() || !path.is_dir() {
            return Err(TianyanError::DirectoryNotFound(path));
        }

        let mut entries = Vec::new();

        for entry in fs::read_dir(&path)
            .map_err(|e| TianyanError::StorageBackend(format!("读取目录 {:?} 失败: {}", path, e)))?
        {
            let entry = entry
                .map_err(|e| TianyanError::StorageBackend(format!("读取目录条目失败: {}", e)))?;

            let name = entry.file_name().to_string_lossy().to_string();

            if UriMapper::is_special_file(&entry.path()) {
                continue;
            }

            let child_uri = uri.append(&name);
            if let Ok(child_entry) = self.read_entry(&child_uri).await {
                entries.push(child_entry);
            }
        }

        Ok(entries)
    }

    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        self.read_content_sync(uri, level)
    }

    async fn write_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()> {
        let path = match level {
            ContentLevel::Abstract => self.mapper.get_abstract_path(uri),
            ContentLevel::Overview => self.mapper.get_overview_path(uri),
            ContentLevel::Detail => self.mapper.get_detail_path(uri),
        };

        self.write_file(&path, content)?;
        tracing::trace!("已写入 {} 的 {:?} 内容", uri, level);
        Ok(())
    }

    async fn append_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()> {
        let path = match level {
            ContentLevel::Abstract => self.mapper.get_abstract_path(uri),
            ContentLevel::Overview => self.mapper.get_overview_path(uri),
            ContentLevel::Detail => self.mapper.get_detail_path(uri),
        };

        self.append_file_locked(&path, content).await?;
        tracing::trace!("已追加 {} 的 {:?} 内容", uri, level);
        Ok(())
    }

    async fn get_directory_index(&self, uri: &TianyanUri) -> Result<DirectoryIndex> {
        self.generate_directory_index_sync(uri)
    }

    async fn update_directory_index(
        &self,
        _uri: &TianyanUri,
        _index: &DirectoryIndex,
    ) -> Result<()> {
        Ok(())
    }

    async fn get_stats(&self) -> Result<StorageStats> {
        let mut stats = StorageStats::default();

        for &namespace in ContextNamespace::ALL {
            let uri = TianyanUri::new(namespace, vec![]);
            let path = self.mapper.uri_to_path(&uri);

            if path.exists() {
                let category_stats = self.calculate_category_stats(&path)?;
                stats
                    .by_category
                    .insert(namespace.to_string(), category_stats);
            }
        }

        for category_stats in stats.by_category.values() {
            stats.total_entries += category_stats.count;
            stats.total_directories += category_stats.count;
            stats.total_files += category_stats.count;
            stats.total_size += category_stats.size;
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_storage_initialize() {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig::default();
        config.data_dir = dir.path().into();
        let storage = LocalStorageBackend::new(config);

        storage.initialize().await.unwrap();

        assert!(dir.path().join("user").exists());
        assert!(dir.path().join("session").exists());
        assert!(dir.path().join("memory").exists());
        assert!(dir.path().join("knowledge").exists());
        assert!(dir.path().join("agent").exists());
        assert!(dir.path().join("skill").exists());
    }

    #[tokio::test]
    async fn test_local_storage_write_read_entry() {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig::default();
        config.data_dir = dir.path().into();
        let storage = LocalStorageBackend::new(config);
        storage.initialize().await.unwrap();

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "basic_info".to_string()],
        );

        let mut entry = ContextEntry::new_file(uri.clone());
        entry.abstract_content = Some("测试摘要".to_string());
        entry.overview_content = Some("测试概览".to_string());
        entry.detail_content = Some("测试详细内容".to_string());

        storage.write_entry(&entry).await.unwrap();

        let read_entry = storage.read_entry(&uri).await.unwrap();
        assert_eq!(read_entry.metadata.uri, uri);
        assert_eq!(read_entry.abstract_content, Some("测试摘要".to_string()));
        assert_eq!(read_entry.overview_content, Some("测试概览".to_string()));
        assert_eq!(read_entry.detail_content, Some("测试详细内容".to_string()));
    }

    #[tokio::test]
    async fn test_local_storage_delete_entry() {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig::default();
        config.data_dir = dir.path().into();
        let storage = LocalStorageBackend::new(config);
        storage.initialize().await.unwrap();

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "test".to_string()],
        );

        let entry = ContextEntry::new_file(uri.clone());
        storage.write_entry(&entry).await.unwrap();

        assert!(storage.exists(&uri).await.unwrap());

        storage.delete_entry(&uri).await.unwrap();
        assert!(!storage.exists(&uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_storage_list_directory() {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig::default();
        config.data_dir = dir.path().into();
        let storage = LocalStorageBackend::new(config);
        storage.initialize().await.unwrap();

        for i in 0..3 {
            let uri = TianyanUri::new(
                ContextNamespace::User,
                vec!["profile".to_string(), format!("entry_{}", i)],
            );
            let entry = ContextEntry::new_file(uri);
            storage.write_entry(&entry).await.unwrap();
        }

        let parent_uri = TianyanUri::new(ContextNamespace::User, vec!["profile".to_string()]);
        let entries = storage.list_directory(&parent_uri).await.unwrap();

        assert_eq!(entries.len(), 3);
    }
}
