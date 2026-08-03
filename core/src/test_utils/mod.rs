//! 测试工具（仅在 `cfg(test)` 时编译）。
//!
//! 提供共享的 Mock 实现，避免在各个测试模块中重复定义。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    ContentLevel, ContextNamespace, EntryMetadata, SearchResult, TianyanUri,
};
use crate::model::types::{EmbeddingData, EmbeddingRequest, EmbeddingResponse};
use crate::model::EmbeddingService;
use crate::vfs::{
    ContentMetadata, ContentStore, ContextEntry, VectorPoint, VectorSearchQuery,
    VectorSearchResult, VectorStorage, VectorType, VfsCore, VfsSearch, VirtualFileSystem,
    CURRENT_SCHEMA_VERSION,
};

// ── MockVfs ───────────────────────────────────────────────────────────────

/// 线程安全的 mock VFS，支持可配置的内容、条目和搜索结果。
///
/// 用法：
/// ```rust,ignore
/// let vfs = Arc::new(MockVfs::new());
/// vfs.set_content(&uri, ContentLevel::Detail, "data");
/// vfs.add_entry(&dir_uri, &entry_uri);
/// ```
pub struct MockVfs {
    /// (uri_string, ContentLevel) → content string
    content: RwLock<HashMap<(String, ContentLevel), String>>,
    /// uri_string → Vec<ContextEntry>
    entries: RwLock<HashMap<String, Vec<ContextEntry>>>,
    /// uri_string → exists() bool
    exists: RwLock<HashMap<String, bool>>,
    /// 可配置的搜索结果
    search_results: RwLock<Vec<SearchResult>>,
    /// 搜索错误（Some = 模拟搜索失败）
    search_error: RwLock<Option<String>>,
}

impl MockVfs {
    /// 创建一个新的空 MockVfs。
    pub fn new() -> Self {
        Self {
            content: RwLock::new(HashMap::new()),
            entries: RwLock::new(HashMap::new()),
            exists: RwLock::new(HashMap::new()),
            search_results: RwLock::new(vec![]),
            search_error: RwLock::new(None),
        }
    }

    /// 创建一个带预置内容的 MockVfs（与 `with_*` 方法连用）。
    pub fn builder() -> MockVfsBuilder {
        MockVfsBuilder::new()
    }

    /// 为指定 URI 设置内容。
    pub fn set_content(&self, uri: &TianyanUri, level: ContentLevel, text: &str) {
        self.content
            .write()
            .unwrap()
            .insert((uri.to_string(), level), text.to_string());
    }

    /// 在指定目录下添加条目。
    pub fn add_entry(&self, dir_uri: &TianyanUri, entry_uri: &TianyanUri) {
        self.entries
            .write()
            .unwrap()
            .entry(dir_uri.to_string())
            .or_default()
            .push(ContextEntry::new_file(entry_uri.clone()));
    }

    /// 设置指定 URI 的 exists() 返回值。
    pub fn set_exists(&self, uri: &TianyanUri, val: bool) {
        self.exists.write().unwrap().insert(uri.to_string(), val);
    }

    /// 设置搜索结果。
    pub fn set_search_results(&self, results: Vec<SearchResult>) {
        *self.search_results.write().unwrap() = results;
    }

    /// 设置搜索错误（设为 Some 后 search() 会返回错误）。
    pub fn set_search_error(&self, msg: Option<String>) {
        *self.search_error.write().unwrap() = msg;
    }
}

impl Default for MockVfs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VfsCore for MockVfs {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
        Ok(self
            .exists
            .read()
            .unwrap()
            .get(&uri.to_string())
            .copied()
            .unwrap_or(self.entries.read().unwrap().contains_key(&uri.to_string())))
    }

    async fn get_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_file(uri.clone()))
    }

    async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        self.entries
            .write()
            .unwrap()
            .entry(uri.to_string())
            .or_default();
        Ok(ContextEntry::new_directory(uri.clone()))
    }

    async fn create_file(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_file(uri.clone()))
    }

    async fn delete(&self, uri: &TianyanUri) -> Result<()> {
        self.entries.write().unwrap().remove(&uri.to_string());
        self.content
            .write()
            .unwrap()
            .retain(|(u, _), _| !u.starts_with(&uri.to_string()));
        self.exists.write().unwrap().remove(&uri.to_string());
        Ok(())
    }

    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        let prefix = uri.to_string();
        let entries = self
            .entries
            .read()
            .unwrap()
            .get(&prefix)
            .cloned()
            .unwrap_or_default();
        Ok(entries)
    }

    async fn move_entry(&self, _src: &TianyanUri, _dst: &TianyanUri) -> Result<()> {
        Ok(())
    }

    async fn update_metadata(
        &self,
        _uri: &TianyanUri,
        _imp: f32,
        _custom: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        Ok(())
    }

    async fn get_all_content_metadata(
        &self,
        _uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>> {
        Ok(HashMap::new())
    }
}

#[async_trait]
impl ContentStore for MockVfs {
    async fn write(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()> {
        self.content
            .write()
            .unwrap()
            .insert((uri.to_string(), level), content.to_string());
        Ok(())
    }

    async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        self.content
            .read()
            .unwrap()
            .get(&(uri.to_string(), level))
            .cloned()
            .ok_or_else(|| TianyanError::Custom(format!("条目未找到：{}", uri)))
    }

    async fn append(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        let mut map = self.content.write().unwrap();
        let key = (uri.to_string(), ContentLevel::Detail);
        let existing = map.get(&key).cloned().unwrap_or_default();
        map.insert(key, existing + content);
        Ok(())
    }

    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool> {
        Ok(self
            .content
            .read()
            .unwrap()
            .contains_key(&(uri.to_string(), level)))
    }
}

#[async_trait]
impl VfsSearch for MockVfs {
    async fn search(
        &self,
        _q: &str,
        _l: usize,
        _ns: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>> {
        if let Some(ref e) = *self.search_error.read().unwrap() {
            Err(TianyanError::Custom(format!("内部错误：{}", e)))
        } else {
            Ok(self.search_results.read().unwrap().clone())
        }
    }

    async fn search_by_visual(&self, _v: &[f32], _k: usize) -> Result<Vec<SearchResult>> {
        Ok(vec![])
    }

    async fn update_summary_vectors(&self, _u: &TianyanUri, _a: &str, _o: &str) -> Result<()> {
        Ok(())
    }
}

impl VirtualFileSystem for MockVfs {}

// ── MockVfsBuilder ───────────────────────────────────────────────────────

/// 用于链式构建 MockVfs 的构建器。
pub struct MockVfsBuilder {
    vfs: MockVfs,
}

impl MockVfsBuilder {
    fn new() -> Self {
        Self {
            vfs: MockVfs::new(),
        }
    }

    /// 添加内容到指定 URI。
    pub fn with_content(self, uri: &TianyanUri, level: ContentLevel, text: &str) -> Self {
        self.vfs.set_content(uri, level, text);
        self
    }

    /// 添加条目到指定目录。
    pub fn with_entries(self, dir_uri: &TianyanUri, entries: Vec<ContextEntry>) -> Self {
        self.vfs
            .entries
            .write()
            .unwrap()
            .insert(dir_uri.to_string(), entries);
        self
    }

    /// 设置搜索结果。
    pub fn with_search_results(self, results: Vec<SearchResult>) -> Self {
        self.vfs.set_search_results(results);
        self
    }

    /// 设置搜索错误。
    pub fn with_search_error(self, msg: &str) -> Self {
        self.vfs.set_search_error(Some(msg.to_string()));
        self
    }

    /// 构建 MockVfs。
    pub fn build(self) -> MockVfs {
        self.vfs
    }
}

// ── MockChatService ──────────────────────────────────────────────────────

/// 由 mockall 自动生成的 ChatService mock，重新导出供测试使用。
///
/// 使用方式：
/// ```ignore
/// let mut mock = MockChatService::new();
/// mock.expect_chat_completion()
///     .returning(|_| Ok(ChatCompletionResponse { ... }));
/// ```
pub use crate::model::MockChatService;

// ── MockEmbeddingService ─────────────────────────────────────────────────

/// Mock embedding service for testing — always returns a fixed embedding.
pub struct MockEmbeddingService;

#[async_trait]
impl EmbeddingService for MockEmbeddingService {
    async fn embed(&self, _request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        Ok(EmbeddingResponse {
            object: "list".to_string(),
            data: vec![EmbeddingData {
                object: "embedding".to_string(),
                embedding: vec![0.1; 768],
                index: 0,
            }],
            model: "test".to_string(),
            usage: crate::common::types::TokenUsage::default(),
        })
    }

    fn embedding_dimension(&self, _model: &str) -> usize {
        768
    }
}

// ── cosine_similarity ────────────────────────────────────────────────────

/// Calculate cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

// ── InMemoryVectorStorage ────────────────────────────────────────────────

/// Mock in-memory vector storage for testing.
///
/// Performs real cosine-similarity vector search in memory,
/// unlike `MockVfs` which returns pre-set results.
pub struct InMemoryVectorStorage {
    points: Arc<tokio::sync::RwLock<HashMap<String, VectorPoint>>>,
}

impl InMemoryVectorStorage {
    /// 创建新的内存向量存储。
    pub fn new() -> Self {
        Self {
            points: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryVectorStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VectorStorage for InMemoryVectorStorage {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn upsert_point(&self, point: &VectorPoint) -> Result<()> {
        self.points
            .write()
            .await
            .insert(point.id.clone(), point.clone());
        Ok(())
    }

    async fn delete_point(&self, id: &str) -> Result<()> {
        self.points.write().await.remove(id);
        Ok(())
    }

    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>> {
        let points = self.points.read().await;
        let mut results: Vec<VectorSearchResult> = Vec::new();
        for point in points.values() {
            let pv = match query.vector_type {
                VectorType::Abstract => &point.abstract_vector,
                VectorType::Overview => &point.overview_vector,
                VectorType::Visual => &point.visual_vector,
            };
            if let Some(pv) = pv {
                let score = cosine_similarity(&query.vector, pv);
                results.push(VectorSearchResult {
                    id: point.id.clone(),
                    score,
                    payload: point.payload.clone(),
                });
            }
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(query.limit);
        Ok(results)
    }

    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>> {
        Ok(self.points.read().await.get(id).cloned())
    }

    async fn update_vector(
        &self,
        uri: &TianyanUri,
        vector_type: VectorType,
        vector: &[f32],
    ) -> Result<()> {
        let id = uri.to_point_id();
        let mut guard = self.points.write().await;
        let mut point = guard.get(&id).cloned().unwrap_or_else(|| VectorPoint {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: id.clone(),
            abstract_vector: None,
            overview_vector: None,
            visual_vector: None,
            payload: EntryMetadata::new(uri.clone(), "unknown"),
        });
        match vector_type {
            VectorType::Abstract => point.abstract_vector = Some(vector.to_vec()),
            VectorType::Overview => point.overview_vector = Some(vector.to_vec()),
            VectorType::Visual => point.visual_vector = Some(vector.to_vec()),
        }
        guard.insert(id, point);
        Ok(())
    }

    async fn count_points(&self) -> Result<usize> {
        Ok(self.points.read().await.len())
    }

    async fn clear(&self) -> Result<()> {
        self.points.write().await.clear();
        Ok(())
    }
}

// ── TestVfs ──────────────────────────────────────────────────────────────

/// Test VFS that delegates vector operations to in-memory vector storage.
///
/// Used by retriever tests that need real vector search behavior.
pub struct TestVfs {
    pub(super) vector_storage: Arc<InMemoryVectorStorage>,
    pub(super) _embedding_service: Arc<MockEmbeddingService>,
}

impl TestVfs {
    /// 创建新的测试 VFS（使用内存向量存储与 mock 嵌入服务）。
    pub fn new(
        vector_storage: Arc<InMemoryVectorStorage>,
        embedding_service: Arc<MockEmbeddingService>,
    ) -> Self {
        Self {
            vector_storage,
            _embedding_service: embedding_service,
        }
    }
}

#[async_trait]
impl VfsCore for TestVfs {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn exists(&self, _uri: &TianyanUri) -> Result<bool> {
        Ok(false)
    }

    async fn get_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_file(uri.clone()))
    }

    async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_directory(uri.clone()))
    }

    async fn create_file(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_file(uri.clone()))
    }

    async fn delete(&self, _uri: &TianyanUri) -> Result<()> {
        Ok(())
    }

    async fn list(&self, _uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        Ok(vec![])
    }

    async fn move_entry(&self, _src: &TianyanUri, _dst: &TianyanUri) -> Result<()> {
        Ok(())
    }

    async fn update_metadata(
        &self,
        _uri: &TianyanUri,
        _imp: f32,
        _custom: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        Ok(())
    }

    async fn get_all_content_metadata(
        &self,
        _uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>> {
        Ok(HashMap::new())
    }
}

#[async_trait]
impl ContentStore for TestVfs {
    async fn write(&self, _uri: &TianyanUri, _level: ContentLevel, _content: &str) -> Result<()> {
        Ok(())
    }

    async fn read(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<String> {
        Ok(String::new())
    }

    async fn append(&self, _uri: &TianyanUri, _content: &str) -> Result<()> {
        Ok(())
    }

    async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> {
        Ok(false)
    }
}

#[async_trait]
impl VfsSearch for TestVfs {
    async fn search(
        &self,
        query_text: &str,
        top_k: usize,
        _namespace: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>> {
        let query_embedding = self
            ._embedding_service
            .embed(EmbeddingRequest::new(
                "test".to_string(),
                query_text.to_string(),
            ))
            .await?
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .unwrap_or_default();

        let vs_query = VectorSearchQuery {
            vector: query_embedding,
            vector_type: VectorType::Abstract,
            limit: top_k,
            category_filter: None,
            min_score: None,
        };

        let results: Vec<VectorSearchResult> = self.vector_storage.search(vs_query).await?;
        Ok(results
            .into_iter()
            .map(|r| SearchResult {
                uri: TianyanUri::parse(&r.id).unwrap_or_else(|_| {
                    TianyanUri::new(ContextNamespace::Knowledge, vec![r.id.clone()])
                }),
                score: r.score,
                matched_level: ContentLevel::Abstract,
                content: None,
            })
            .collect())
    }

    async fn search_by_visual(&self, _v: &[f32], _k: usize) -> Result<Vec<SearchResult>> {
        Ok(vec![])
    }

    async fn update_summary_vectors(
        &self,
        _uri: &TianyanUri,
        _abstract_content: &str,
        _overview_content: &str,
    ) -> Result<()> {
        Ok(())
    }
}

impl VirtualFileSystem for TestVfs {}

// ── create_test_retriever ────────────────────────────────────────────────

use crate::context::retrieval::DualLayerRetriever;

/// Create a DualLayerRetriever backed by in-memory vector storage for testing.
pub async fn create_test_retriever() -> DualLayerRetriever {
    let vector_storage = Arc::new(InMemoryVectorStorage::new());
    vector_storage.initialize().await.unwrap();
    let vfs: Arc<dyn VirtualFileSystem> =
        Arc::new(TestVfs::new(vector_storage, Arc::new(MockEmbeddingService)));

    DualLayerRetriever::new(vfs).with_embedding_service(Arc::new(MockEmbeddingService))
}
