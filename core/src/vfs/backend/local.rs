//! 本地文件系统存储后端实现。

use std::path::Path;
use std::sync::Arc;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, TianyanUri};
use crate::vfs::types::ContextEntry;

use crate::config::StorageConfig;
use crate::vfs::uri_mapper::UriMapper;

/// 本地文件系统存储后端。
pub struct LocalFileBackend {
    config: StorageConfig,
    mapper: UriMapper,
    /// 文件路径级别的写入锁，防止并发追加导致内容交错。
    write_locks: Arc<Mutex<std::collections::HashMap<std::path::PathBuf, Arc<Mutex<()>>>>>,
}

impl LocalFileBackend {
    /// 创建新的本地存储后端。
    pub fn new(config: StorageConfig) -> Self {
        let mapper = UriMapper::new(config.clone());
        Self {
            config,
            mapper,
            write_locks: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 确保目录存在，如需要则创建。
    async fn ensure_dir(&self, path: &Path) -> Result<()> {
        if !fs::try_exists(path).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：检查目录 {path:?} 失败: {e}"))
        })? {
            fs::create_dir_all(path).await.map_err(|e| {
                TianyanError::Custom(format!("存储后端错误：创建目录 {path:?} 失败: {e}"))
            })?;
        }
        Ok(())
    }

    /// 读取文件内容为字符串。
    async fn read_file(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：打开文件 {path:?} 失败: {e}"))
        })?;
        let mut content = String::new();
        file.read_to_string(&mut content).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：读取文件 {path:?} 失败: {e}"))
        })?;
        Ok(content)
    }

    /// 将字符串内容写入文件（使用写入锁防止并发写入交错）。
    async fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent).await?;
        }
        let lock = self.acquire_write_lock(path).await;
        let _guard = lock.lock().await;
        let mut file = fs::File::create(path).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：创建文件 {path:?} 失败: {e}"))
        })?;
        file.write_all(content.as_bytes()).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：写入文件 {path:?} 失败: {e}"))
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

    /// 追加内容到文件。
    ///
    /// 自动创建父目录和文件（如不存在），无需调用方预检查。
    async fn append_file_locked(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent).await?;
        }

        let lock = self.acquire_write_lock(path).await;
        let _guard = lock.lock().await;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| {
                TianyanError::Custom(format!("存储后端错误：打开文件 {path:?} 用于追加失败: {e}"))
            })?;
        file.write_all(content.as_bytes()).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：追加写入文件 {path:?} 失败: {e}"))
        })?;
        Ok(())
    }

    /// 删除文件或目录。
    async fn delete_path(&self, path: &Path) -> Result<()> {
        match fs::metadata(path).await {
            Ok(m) if m.is_dir() => {
                fs::remove_dir_all(path).await.map_err(|e| {
                    TianyanError::Custom(format!("存储后端错误：删除目录 {path:?} 失败: {e}"))
                })?;
            }
            Ok(_) => {
                fs::remove_file(path).await.map_err(|e| {
                    TianyanError::Custom(format!("存储后端错误：删除文件 {path:?} 失败: {e}"))
                })?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(TianyanError::Custom(format!(
                    "存储后端错误：访问 {path:?} 失败: {e}"
                )));
            }
        }
        Ok(())
    }

    /// 读取内容（内部辅助方法）。
    async fn read_content_sync(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        let path = match level {
            ContentLevel::Abstract => self.mapper.get_abstract_path(uri),
            ContentLevel::Overview => self.mapper.get_overview_path(uri),
            ContentLevel::Detail => self.mapper.get_detail_path(uri),
        };

        if !fs::try_exists(&path)
            .await
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：访问 {path:?} 失败: {e}")))?
        {
            return Err(TianyanError::Custom(format!(
                "条目未找到：{} 无 {:?} 层级内容",
                uri, level
            )));
        }

        self.read_file(&path).await
    }
}

impl LocalFileBackend {
    /// 初始化存储后端，确保数据目录存在。
    pub async fn initialize(&self) -> Result<()> {
        self.ensure_dir(&self.config.data_dir).await?;
        tracing::info!("已在 {:?} 初始化本地存储", self.config.data_dir);
        Ok(())
    }

    /// 检查指定 URI 的条目是否存在。
    pub async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
        let path = self.mapper.uri_to_path(uri);
        Ok(fs::try_exists(path).await.unwrap_or(false))
    }

    /// 读取指定 URI 的完整条目（含 L0/L1/L2 三层内容）。
    pub async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        let path = self.mapper.uri_to_path(uri);

        let meta = fs::metadata(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TianyanError::Custom(format!("条目未找到：{uri}"))
            } else {
                TianyanError::Custom(format!("存储后端错误：访问 {path:?} 失败: {e}"))
            }
        })?;

        let is_directory = meta.is_dir();

        let abstract_content = self
            .read_content_sync(uri, ContentLevel::Abstract)
            .await
            .ok();
        let overview_content = self
            .read_content_sync(uri, ContentLevel::Overview)
            .await
            .ok();
        let detail_content = self.read_content_sync(uri, ContentLevel::Detail).await.ok();

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

    /// 写入完整的条目内容（按层级分别持久化）。
    pub async fn write_entry(&self, entry: &ContextEntry) -> Result<()> {
        let path = self.mapper.uri_to_path(entry.uri());

        self.ensure_dir(&path).await?;

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

    /// 删除指定 URI 的条目（文件或目录）。
    pub async fn delete_entry(&self, uri: &TianyanUri) -> Result<()> {
        let path = self.mapper.uri_to_path(uri);

        if !fs::try_exists(&path)
            .await
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：访问 {path:?} 失败: {e}")))?
        {
            return Err(TianyanError::Custom(format!("条目未找到：{uri}")));
        }

        self.delete_path(&path).await?;

        tracing::debug!("已删除 {:?} 处的条目", path);
        Ok(())
    }

    /// 列出指定 URI 下的一级子条目。
    pub async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        let path = self.mapper.uri_to_path(uri);

        let meta = fs::metadata(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TianyanError::Custom(format!("目录未找到：{}", path.display()))
            } else {
                TianyanError::Custom(format!("存储后端错误：访问 {path:?} 失败: {e}"))
            }
        })?;

        if !meta.is_dir() {
            return Err(TianyanError::Custom(format!(
                "目录未找到：{}",
                path.display()
            )));
        }

        let mut read_dir = fs::read_dir(&path).await.map_err(|e| {
            TianyanError::Custom(format!("存储后端错误：读取目录 {path:?} 失败: {e}"))
        })?;

        let mut entries = Vec::new();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：读取目录条目失败: {e}")))?
        {
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

    /// 读取指定 URI 的某一层级内容（L0/L1/L2）。
    pub async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        self.read_content_sync(uri, level).await
    }

    /// 写入指定 URI 的某一层级内容（覆盖写入）。
    pub async fn write_content(
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

        self.write_file(&path, content).await?;
        tracing::trace!("已写入 {} 的 {:?} 内容", uri, level);
        Ok(())
    }

    /// 追加内容到指定 URI 的某一层级（自动创建文件）。
    pub async fn append_content(
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_storage_initialize() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("data");
        let config = StorageConfig {
            data_dir: sub.clone(),
            ..Default::default()
        };
        let storage = LocalFileBackend::new(config);

        assert!(!fs::try_exists(&sub).await.unwrap_or(false));
        storage.initialize().await.unwrap();
        assert!(fs::try_exists(&sub).await.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_local_storage_write_read_entry() {
        let dir = tempdir().unwrap();
        let config = StorageConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        };
        let storage = LocalFileBackend::new(config);
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
        let config = StorageConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        };
        let storage = LocalFileBackend::new(config);
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
        let config = StorageConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        };
        let storage = LocalFileBackend::new(config);
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
