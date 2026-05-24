//! 虚拟文件系统实现。

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    ContentLevel, ContextNamespace, EntryMetadata, SearchResult, TianyanUri,
};
use crate::model::EmbeddingService;
use crate::vfs::backend::LocalFileBackend;
use crate::vfs::traits::{
    ContentMetadata, ContentStore, VectorStorage, VfsCore, VfsMetadata, VfsSearch,
    VirtualFileSystem,
};
use crate::vfs::types::{ContextEntry, VectorPoint, VectorSearchQuery, VectorType};

use crate::config::StorageConfig;

/// 虚拟文件系统实现。
pub struct VirtualFileSystemImpl {
    storage: Arc<LocalFileBackend>,
    vector_storage: Arc<dyn VectorStorage>,
    config: StorageConfig,
    embedding_service: Option<Arc<dyn EmbeddingService>>,
    embedding_model: Option<String>,
}

impl VirtualFileSystemImpl {
    /// 创建新的虚拟文件系统。
    pub fn new(
        storage: Arc<LocalFileBackend>,
        vector_storage: Arc<dyn VectorStorage>,
        config: StorageConfig,
    ) -> Self {
        Self {
            storage,
            vector_storage,
            config,
            embedding_service: None,
            embedding_model: None,
        }
    }

    /// 使用默认配置创建虚拟文件系统。
    pub fn with_defaults(
        storage: Arc<LocalFileBackend>,
        vector_storage: Arc<dyn VectorStorage>,
    ) -> Self {
        Self::new(storage, vector_storage, StorageConfig::default())
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

    /// 设置嵌入服务（可变引用版本）。
    pub fn set_embedding_service(
        &mut self,
        service: Arc<dyn EmbeddingService>,
        model: impl Into<String>,
    ) {
        self.embedding_service = Some(service);
        self.embedding_model = Some(model.into());
    }

    /// 获取存储配置。
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// 从上下文条目生成向量点。
    fn create_vector_point(&self, entry: &ContextEntry) -> VectorPoint {
        VectorPoint::from_entry(entry)
    }

    /// 为内容生成嵌入向量并存储到向量数据库。
    ///
    /// 此方法会调用嵌入服务为指定内容生成向量，然后更新向量存储。
    /// 如果嵌入服务不可用，会记录警告但不会失败。
    async fn generate_and_store_embedding(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()> {
        if content.trim().is_empty() {
            tracing::trace!("内容为空，跳过向量生成 {} {:?}", uri, level);
            return Ok(());
        }

        let (Some(service), Some(model)) = (&self.embedding_service, &self.embedding_model) else {
            tracing::warn!("嵌入服务未配置，跳过向量生成: {} {:?}", uri, level);
            return Ok(());
        };

        let vector_type = match level {
            ContentLevel::Abstract => VectorType::Abstract,
            ContentLevel::Overview => VectorType::Overview,
            ContentLevel::Detail => {
                tracing::trace!("Detail 层级不生成向量 {}", uri);
                return Ok(());
            }
        };

        tracing::debug!("开始为 {} {:?} 生成向量", uri, level);

        let dimensions = self.config.vector.vector_dimension;

        match service
            .embed_single_with_dimensions(model, content, dimensions)
            .await
        {
            Ok(embedding) => {
                let vector = embedding.vector.clone();

                if let Err(e) = self
                    .vector_storage
                    .update_vector(uri, vector_type, &vector)
                    .await
                {
                    tracing::error!("更新向量失败: {} {:?} - {}", uri, level, e);
                    return Err(e);
                }

                tracing::info!("向量生成成功: {} {:?} (维度: {})", uri, level, vector.len());
                Ok(())
            }
            Err(e) => {
                tracing::warn!("向量生成失败: {} {:?} - {}", uri, level, e);
                Err(e)
            }
        }
    }

    /// 将条目同步到向量存储。
    async fn sync_to_vector_storage(&self, entry: &ContextEntry) -> Result<()> {
        let point = self.create_vector_point(entry);
        self.vector_storage.upsert_point(&point).await
    }

    /// 从向量存储中删除条目。
    async fn remove_from_vector_storage(&self, uri: &TianyanUri) -> Result<()> {
        let id = uri.to_point_id();
        self.vector_storage.delete_point(&id).await
    }

    /// 通过视觉嵌入搜索（用于图像相似度搜索）。
    pub async fn find_by_visual_embedding(
        &self,
        query_embedding: &[f32],
        scope: Option<&TianyanUri>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        let query = VectorSearchQuery {
            vector: query_embedding.to_vec(),
            vector_type: VectorType::Visual,
            limit: top_k,
            category_filter: scope.map(|s| s.namespace().to_string()),
            min_score: None,
        };

        let results = self.vector_storage.search(query).await?;

        Ok(results
            .into_iter()
            .map(|r| SearchResult {
                uri: r.payload.uri.clone(),
                score: r.score,
                matched_level: ContentLevel::Abstract,
                content: None,
            })
            .collect())
    }

    /// 递归收集指定 URI 下的所有 URI。
    pub async fn collect_all_uris(&self, uri: &TianyanUri) -> Result<Vec<TianyanUri>> {
        let mut uris = Vec::new();
        self.collect_uris_recursive(uri, &mut uris).await?;
        Ok(uris)
    }

    async fn collect_uris_recursive(
        &self,
        uri: &TianyanUri,
        uris: &mut Vec<TianyanUri>,
    ) -> Result<()> {
        if !self.storage.exists(uri).await? {
            return Ok(());
        }

        let entry = self.storage.read_entry(uri).await?;
        if entry.is_directory() {
            let children = self.storage.list_directory(uri).await?;
            for child in children {
                Box::pin(self.collect_uris_recursive(child.uri(), uris)).await?;
            }
        } else {
            uris.push(uri.clone());
        }

        Ok(())
    }

    /// 获取条目的总大小（包括目录的子条目）。
    pub async fn get_entry_size(&self, uri: &TianyanUri) -> Result<u64> {
        let entry = self.storage.read_entry(uri).await?;
        if entry.is_directory() {
            let children = self.storage.list_directory(uri).await?;
            let mut total_size = 0u64;
            for child in children {
                total_size += Box::pin(self.get_entry_size(child.uri())).await?;
            }
            Ok(total_size)
        } else {
            Ok(entry.metadata.file_size.unwrap_or(0))
        }
    }

    /// 检查 URI 是否有效。
    pub fn validate_uri(uri: &TianyanUri) -> Result<()> {
        if uri.path().is_empty() {
            return Ok(());
        }

        for segment in uri.path() {
            if segment.is_empty() {
                return Err(TianyanError::InvalidUri(format!(
                    "URI 中存在空段： {}",
                    uri
                )));
            }
            if segment.contains("..") || segment.contains('\\') || segment.contains('\0') {
                return Err(TianyanError::InvalidUri(format!(
                    "URI '{}' 中存在无效段 '{}'",
                    uri, segment
                )));
            }
        }

        Ok(())
    }
}

#[async_trait]
impl VfsCore for VirtualFileSystemImpl {
    async fn initialize(&self) -> Result<()> {
        self.storage.initialize().await?;
        self.vector_storage.initialize().await?;
        tracing::info!("虚拟文件系统已初始化");
        Ok(())
    }

    async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
        self.storage.exists(uri).await
    }

    async fn get_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Self::validate_uri(uri)?;
        self.storage.read_entry(uri).await
    }

    async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Self::validate_uri(uri)?;

        if self.storage.exists(uri).await? {
            return Err(TianyanError::EntryAlreadyExists(uri.to_string()));
        }

        if let Some(parent) = uri.parent() {
            if !self.storage.exists(&parent).await? {
                self.create_directory(&parent).await?;
            }
        }

        let entry = ContextEntry::new_directory(uri.clone());
        self.storage.write_entry(&entry).await?;

        tracing::debug!("已创建目录： {}", uri);
        Ok(entry)
    }

    async fn create_file(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Self::validate_uri(uri)?;

        if self.storage.exists(uri).await? {
            return Err(TianyanError::EntryAlreadyExists(uri.to_string()));
        }

        if let Some(parent) = uri.parent() {
            if !self.storage.exists(&parent).await? {
                self.create_directory(&parent).await?;
            }
        }

        let entry = ContextEntry::new_file(uri.clone());
        self.storage.write_entry(&entry).await?;

        tracing::debug!("已创建文件： {}", uri);
        Ok(entry)
    }

    async fn delete(&self, uri: &TianyanUri) -> Result<()> {
        Self::validate_uri(uri)?;
        if !self.storage.exists(uri).await? {
            return Err(TianyanError::EntryNotFound(uri.to_string()));
        }
        self.storage.delete_entry(uri).await?;
        let point_id = uri.to_string().replace("://", "_").replace('/', "_");
        let _ = self.vector_storage.delete_point(&point_id).await;
        tracing::debug!("已删除条目： {}", uri);
        Ok(())
    }

    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        Self::validate_uri(uri)?;
        self.storage.list_directory(uri).await
    }

    async fn move_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()> {
        for level in &[
            ContentLevel::Abstract,
            ContentLevel::Overview,
            ContentLevel::Detail,
        ] {
            if let Ok(content) = self.storage.read_content(source, *level).await {
                self.storage
                    .write_content(destination, *level, &content)
                    .await?;
            }
        }
        let src_entry = self.storage.read_entry(source).await?;
        let mut dst_entry = src_entry.clone();
        dst_entry.metadata.uri = destination.clone();
        dst_entry.metadata.touch();
        self.storage.write_entry(&dst_entry).await?;
        self.storage.delete_entry(source).await?;
        let point_id = source.to_string().replace("://", "_").replace('/', "_");
        let _ = self.vector_storage.delete_point(&point_id).await;
        tracing::debug!("已移动条目： {} -> {}", source, destination);
        Ok(())
    }
}

#[async_trait]
impl ContentStore for VirtualFileSystemImpl {
    async fn write(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()> {
        Self::validate_uri(uri)?;

        if !self.storage.exists(uri).await? {
            if let Some(parent) = uri.parent() {
                if !self.storage.exists(&parent).await? {
                    self.storage
                        .write_entry(&ContextEntry::new_directory(parent.clone()))
                        .await?;
                }
            }
            self.storage
                .write_entry(&ContextEntry::new_file(uri.clone()))
                .await?;
        }

        self.storage.write_content(uri, level, content).await?;

        let mut entry = self.storage.read_entry(uri).await?;
        entry.metadata.touch();
        entry.set_content(level, content.to_string());
        self.storage.write_entry(&entry).await?;

        tracing::trace!("已写入 {:?} 内容： {}", level, uri);
        Ok(())
    }

    async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        Self::validate_uri(uri)?;
        self.storage.read_content(uri, level).await
    }

    async fn append(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        Self::validate_uri(uri)?;

        if !self.storage.exists(uri).await? {
            if let Some(parent) = uri.parent() {
                if !self.storage.exists(&parent).await? {
                    self.storage
                        .write_entry(&ContextEntry::new_directory(parent.clone()))
                        .await?;
                }
            }
            self.storage
                .write_entry(&ContextEntry::new_file(uri.clone()))
                .await?;
        }

        let level = ContentLevel::Detail;
        self.storage.append_content(uri, level, content).await?;

        let mut entry = self.storage.read_entry(uri).await?;
        entry.metadata.touch();
        let existing = entry.get_content(level).unwrap_or("").to_string();
        let combined = format!("{}{}", existing, content);
        entry.set_content(level, combined);
        self.storage.write_entry(&entry).await?;

        tracing::trace!("已追加内容： {}", uri);
        Ok(())
    }

    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool> {
        let entry = self.storage.read_entry(uri).await?;
        Ok(entry.has_content(level))
    }
}

#[async_trait]
impl VfsSearch for VirtualFileSystemImpl {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>> {
        let embedding_service = self.embedding_service.as_ref().ok_or_else(|| {
            TianyanError::Retrieval("VFS 未配置嵌入服务，无法进行向量搜索".to_string())
        })?;

        let model = self
            .embedding_model
            .as_deref()
            .unwrap_or("text-embedding-3-small");

        let embedding = embedding_service.embed_single(model, query).await?;
        let query_vector = embedding.vector;

        let category_filter = namespace.map(|ns| ns.to_string());
        let results = self
            .vector_storage
            .search_abstract_and_overview(query_vector, limit * 2, category_filter.as_deref())
            .await?;

        let results: Vec<SearchResult> = results
            .into_iter()
            .map(|vsr| SearchResult {
                uri: vsr.payload.uri.clone(),
                score: vsr.score,
                matched_level: ContentLevel::Abstract,
                content: None,
            })
            .collect();

        Ok(results)
    }

    async fn search_by_visual(
        &self,
        visual_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        let query = VectorSearchQuery {
            vector: visual_vector.to_vec(),
            vector_type: VectorType::Visual,
            limit: top_k,
            category_filter: None,
            min_score: None,
        };
        let results = self.vector_storage.search(query).await?;
        let results: Vec<SearchResult> = results
            .into_iter()
            .map(|vsr| SearchResult {
                uri: vsr.payload.uri.clone(),
                score: vsr.score,
                matched_level: ContentLevel::Abstract,
                content: None,
            })
            .collect();
        Ok(results)
    }

    async fn update_summary_vectors(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<()> {
        let embedding_service = self
            .embedding_service
            .as_ref()
            .ok_or_else(|| TianyanError::Retrieval("VFS 未配置嵌入服务".to_string()))?;

        let model = self
            .embedding_model
            .as_deref()
            .unwrap_or("text-embedding-3-small");

        let abstract_embedding = embedding_service
            .embed_single(model, abstract_content)
            .await?;
        let overview_embedding = embedding_service
            .embed_single(model, overview_content)
            .await?;

        let point_id = uri.to_string().replace("://", "_").replace('/', "_");
        let payload = EntryMetadata::new(uri.clone(), uri.namespace().to_string());
        let point = VectorPoint {
            schema_version: crate::vfs::CURRENT_SCHEMA_VERSION,
            id: point_id,
            abstract_vector: Some(abstract_embedding.vector),
            overview_vector: Some(overview_embedding.vector),
            visual_vector: None,
            payload,
        };
        self.vector_storage.upsert_point(&point).await?;

        tracing::debug!("已更新向量：{}", uri);
        Ok(())
    }
}

#[async_trait]
impl VfsMetadata for VirtualFileSystemImpl {
    async fn update_metadata(
        &self,
        uri: &TianyanUri,
        importance: f32,
        custom: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        let point_id = uri.to_string().replace("://", "_").replace('/', "_");

        let point = match self.vector_storage.get_point(&point_id).await? {
            Some(mut p) => {
                p.payload.importance = importance;
                for (k, v) in custom {
                    p.payload.custom.insert(k, v);
                }
                p
            }
            None => {
                let payload = EntryMetadata::new(uri.clone(), uri.namespace().to_string())
                    .with_importance(importance);
                VectorPoint {
                    schema_version: crate::vfs::CURRENT_SCHEMA_VERSION,
                    id: point_id,
                    abstract_vector: None,
                    overview_vector: None,
                    visual_vector: None,
                    payload,
                }
            }
        };

        self.vector_storage.upsert_point(&point).await
    }

    async fn get_all_content_metadata(
        &self,
        uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>> {
        let entry = self.storage.read_entry(uri).await?;
        let mut result = HashMap::new();

        for level in [
            ContentLevel::Abstract,
            ContentLevel::Overview,
            ContentLevel::Detail,
        ] {
            if entry.has_content(level) {
                if let Ok(content) = self.storage.read_content(uri, level).await {
                    result.insert(
                        level,
                        ContentMetadata {
                            size: content.len() as u64,
                            created_at: entry.metadata.created_at,
                            updated_at: entry.metadata.updated_at,
                        },
                    );
                }
            }
        }

        Ok(result)
    }
}

#[async_trait]
impl VirtualFileSystem for VirtualFileSystemImpl {
    async fn copy_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()> {
        Self::validate_uri(source)?;
        Self::validate_uri(destination)?;

        if !self.storage.exists(source).await? {
            return Err(TianyanError::EntryNotFound(source.to_string()));
        }

        if self.storage.exists(destination).await? {
            return Err(TianyanError::EntryAlreadyExists(destination.to_string()));
        }

        let entry = self.storage.read_entry(source).await?;

        let mut new_entry = ContextEntry::new_file(destination.clone());
        new_entry.metadata = entry.metadata.clone();
        new_entry.metadata.uri = destination.clone();
        new_entry.metadata.created_at = chrono::Utc::now();
        new_entry.metadata.updated_at = chrono::Utc::now();
        new_entry.abstract_content = entry.abstract_content;
        new_entry.overview_content = entry.overview_content;
        new_entry.detail_content = entry.detail_content;

        self.storage.write_entry(&new_entry).await?;

        if let Ok(content) = self
            .storage
            .read_content(source, ContentLevel::Detail)
            .await
        {
            self.storage
                .write_content(destination, ContentLevel::Detail, &content)
                .await?;
        }

        self.sync_to_vector_storage(&new_entry).await?;

        tracing::debug!("已将 {} 复制到 {}", source, destination);
        Ok(())
    }
}

#[path = "vfs_builder.rs"]
pub mod builder;
#[cfg(test)]
#[path = "vfs_tests.rs"]
mod tests;

pub use builder::{ensure_vfs_structure, initialize_vfs, VirtualFileSystemBuilder};
