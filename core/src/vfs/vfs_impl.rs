//! 虚拟文件系统实现。

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    ContentLevel, ContextNamespace, EntryMetadata, SearchResult, TianyanUri,
};
use crate::model::EmbeddingService;
use crate::vfs::backend::StorageBackend;
use crate::vfs::traits::{ContentMetadata, ContentStore, VfsCore, VfsSearch, VirtualFileSystem};
use crate::vfs::types::{ContextEntry, VectorPoint};
use crate::vfs::vector::VectorStorage;

use crate::config::StorageConfig;

/// 虚拟文件系统实现。
pub struct VirtualFileSystemImpl {
    storage: Arc<dyn StorageBackend>,
    vector_storage: Arc<dyn VectorStorage>,
    config: StorageConfig,
    // RwLock：支持运行时热更新（配置热重载时 set_embedding_provider 无需 &mut）
    embedding_provider: RwLock<Option<Arc<dyn EmbeddingService>>>,
    embedding_model: RwLock<Option<String>>,
}

impl VirtualFileSystemImpl {
    /// 创建新的虚拟文件系统。
    pub fn new(
        storage: Arc<dyn StorageBackend>,
        vector_storage: Arc<dyn VectorStorage>,
        config: StorageConfig,
    ) -> Self {
        Self {
            storage,
            vector_storage,
            config,
            embedding_provider: RwLock::new(None),
            embedding_model: RwLock::new(None),
        }
    }

    /// 设置嵌入服务（`model::EmbeddingService` 直接注入，无桥接层）。
    pub fn with_embedding_provider(
        self,
        provider: Arc<dyn EmbeddingService>,
        model: impl Into<String>,
    ) -> Self {
        *self
            .embedding_provider
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(provider);
        *self
            .embedding_model
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(model.into());
        self
    }

    /// 设置嵌入服务（运行时热更新；RwLock 内部可变，无需 &mut）。
    pub fn set_embedding_provider(
        &self,
        provider: Arc<dyn EmbeddingService>,
        model: impl Into<String>,
    ) {
        *self
            .embedding_provider
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(provider);
        *self
            .embedding_model
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(model.into());
    }

    /// 获取存储配置。
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// 从上下文条目生成向量点。
    fn create_vector_point(&self, entry: &ContextEntry) -> VectorPoint {
        VectorPoint::from_entry(entry)
    }

    /// 递归收集指定 URI 下的所有 URI。
    pub(crate) async fn collect_all_uris(&self, uri: &TianyanUri) -> Result<Vec<TianyanUri>> {
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

    /// 检查 URI 是否有效。
    pub(crate) fn validate_uri(uri: &TianyanUri) -> Result<()> {
        if uri.path().is_empty() {
            return Ok(());
        }

        for segment in uri.path() {
            if segment.is_empty() {
                return Err(TianyanError::Custom(format!(
                    "无效 URI 路径：URI 中存在空段：{uri}"
                )));
            }
            if segment.contains("..") || segment.contains('\\') || segment.contains('\0') {
                return Err(TianyanError::Custom(format!(
                    "无效 URI 路径：URI '{uri}' 中存在无效段 '{segment}'"
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

        // 若 vector_storage 初始化失败，明确记录错误并终止启动
        // storage.initialize() 通常仅创建目录，回滚反而复杂且危险
        // 让调用方感知初始化失败比部分初始化更安全
        if let Err(e) = self.vector_storage.initialize().await {
            tracing::error!(
                error = %e,
                "向量存储初始化失败 —— VFS 初始化未完成，请检查向量存储初始化（LanceDB 路径或权限）"
            );
            return Err(e);
        }

        for &ns in ContextNamespace::ALL {
            let uri = TianyanUri::new(ns, vec![]);
            if !self.storage.exists(&uri).await? {
                self.create_directory(&uri).await?;
                tracing::debug!("已创建 VFS 命名空间： {}", uri);
            }
        }

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
            return Err(TianyanError::Custom(format!("条目已存在：{uri}")));
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
            return Err(TianyanError::Custom(format!("条目已存在：{uri}")));
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
            return Err(TianyanError::not_found(uri));
        }

        // 先递归收集所有子 URI（包括目录自身），统一清理向量库
        let all_uris = self.collect_all_uris(uri).await?;
        for child_uri in &all_uris {
            let point_id = child_uri.to_point_id();
            if let Err(e) = self.vector_storage.delete_point(&point_id).await {
                tracing::warn!(error = %e, uri = %child_uri, "删除向量点失败");
            }
        }
        // 删除目录自身向量
        let point_id = uri.to_point_id();
        if let Err(e) = self.vector_storage.delete_point(&point_id).await {
            tracing::warn!(error = %e, uri = %uri, "删除目录向量点失败");
        }

        self.storage.delete_entry(uri).await?;
        tracing::debug!("已删除条目及其子条目： {}", uri);
        Ok(())
    }

    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        Self::validate_uri(uri)?;
        self.storage.list_directory(uri).await
    }

    async fn move_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()> {
        // 子条目守卫：move 仅迁移顶层条目，不支持移动目录。
        // 复用 delete 的子 URI 收集模式（collect_uris_recursive）的底层原语
        // list_directory 直接探测一级子条目——LocalFileBackend 下文件与目录同为
        // 磁盘目录（is_directory 不可靠），collect_all_uris 对含子条目的目录
        // 会返回空集，无法作为子条目探测器。存在子条目即拒绝，避免子条目被
        // 静默孤儿化（delete_entry 在两种后端下均为递归删除，但 move 只拷贝顶层条目）。
        if !self.storage.list_directory(source).await?.is_empty() {
            return Err(TianyanError::conflict(
                "vfs: move_entry: 源条目含子条目，暂不支持移动目录",
            ));
        }

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
        // 迁移向量点：克隆源点（保留 abstract/overview/visual 向量与 payload 元数据），
        // 强制 id = destination.to_point_id()——LanceDB 以 id 为主键 merge_insert，
        // 所有读写路径均按 to_point_id() 定位；VectorPoint::from_entry 会置空全部向量
        // 且 id 取 URI 字符串，导致目标点双重孤儿化，故不能直接使用。
        let src_point_id = source.to_point_id();
        let dst_point = match self.vector_storage.get_point(&src_point_id).await {
            Ok(Some(mut point)) => {
                point.id = destination.to_point_id();
                point.payload.uri = destination.clone();
                point.payload.touch();
                point
            }
            // 源点缺失（从未索引）：回退为 from_entry 重建，仍强制目标点 ID。
            _ => {
                let mut point = self.create_vector_point(&dst_entry);
                point.id = destination.to_point_id();
                point
            }
        };
        // 清理源向量
        if let Err(e) = self.vector_storage.delete_point(&src_point_id).await {
            tracing::warn!(error = %e, source = %source, "清理源向量点失败");
        }
        // 写入目标向量点
        if let Err(e) = self.vector_storage.upsert_point(&dst_point).await {
            tracing::warn!(error = %e, destination = %destination, "创建目标向量点失败");
        }
        tracing::debug!("已移动条目： {} -> {}", source, destination);
        Ok(())
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

/// 确保条目存在（幂等）：不存在时自动创建父目录 + 文件条目。
///
/// write/append 的建目录语义收敛于此（与 [`VfsCore::create_file`] 的区别：
/// 已存在时静默跳过而非报错——内容写入可重复执行，VFS 自带容错）。
impl VirtualFileSystemImpl {
    async fn ensure_entry_exists(&self, uri: &TianyanUri) -> Result<()> {
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
        Ok(())
    }
}

#[async_trait]
impl ContentStore for VirtualFileSystemImpl {
    async fn write(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()> {
        Self::validate_uri(uri)?;

        self.ensure_entry_exists(uri).await?;

        self.storage.write_content(uri, level, content).await?;

        tracing::trace!("已写入 {:?} 内容： {}", level, uri);
        Ok(())
    }

    async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        Self::validate_uri(uri)?;
        self.storage.read_content(uri, level).await
    }

    async fn append(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        Self::validate_uri(uri)?;

        self.ensure_entry_exists(uri).await?;

        let level = ContentLevel::Detail;
        self.storage.append_content(uri, level, content).await?;

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
        let embedding_provider = self
            .embedding_provider
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| {
                TianyanError::Custom("检索错误：VFS 未配置嵌入服务，无法进行向量搜索".to_string())
            })?;
        let model = self
            .embedding_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| "text-embedding-3-small".to_string());

        let embedding = embedding_provider.embed_single(&model, query).await?;
        let query_vector = embedding.vector;

        let category_filter = namespace.map(|ns| ns.to_string());
        // 内部请求 limit * 2 保证融合质量（每列独立搜索后融合），返回前截断到 limit。
        let results = self
            .vector_storage
            .search_abstract_and_overview(query_vector, limit * 2, category_filter.as_deref())
            .await?;

        let mut results: Vec<SearchResult> = results
            .into_iter()
            .map(|vsr| SearchResult {
                uri: vsr.payload.uri.clone(),
                score: vsr.score,
            })
            .collect();

        results.truncate(limit);
        Ok(results)
    }

    async fn update_summary_vectors(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<()> {
        let point_id = uri.to_point_id();
        // 保留已有的 visual_vector（图像搜索用），避免被 None 覆盖
        let existing_visual = self
            .vector_storage
            .get_point(&point_id)
            .await
            .ok()
            .and_then(|p| p.and_then(|pt| pt.visual_vector));

        let embedding_model = self
            .embedding_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| "text-embedding-3-small".to_string());
        let payload = EntryMetadata::new(uri.clone(), uri.namespace().to_string());
        self.index_entry(
            uri,
            abstract_content,
            overview_content,
            existing_visual,
            payload,
            &embedding_model,
        )
        .await
    }

    async fn index_entry(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
        visual_vector: Option<Vec<f32>>,
        payload: EntryMetadata,
        embedding_model: &str,
    ) -> Result<()> {
        let embedding_provider = self
            .embedding_provider
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| TianyanError::Custom("检索错误：VFS 未配置嵌入服务".to_string()))?;

        let abstract_embedding = embedding_provider
            .embed_single(embedding_model, abstract_content)
            .await?;
        let overview_embedding = embedding_provider
            .embed_single(embedding_model, overview_content)
            .await?;

        let point_id = uri.to_point_id();
        let point = VectorPoint {
            schema_version: crate::vfs::CURRENT_SCHEMA_VERSION,
            id: point_id,
            abstract_vector: Some(abstract_embedding.vector),
            overview_vector: Some(overview_embedding.vector),
            visual_vector,
            payload,
        };
        self.vector_storage.upsert_point(&point).await?;

        tracing::debug!("已创建索引条目：{}", uri);
        Ok(())
    }
}

impl VirtualFileSystem for VirtualFileSystemImpl {}

#[path = "vfs_builder.rs"]
pub mod builder;
#[cfg(test)]
#[path = "vfs_tests.rs"]
mod tests;

pub use builder::VirtualFileSystemBuilder;
