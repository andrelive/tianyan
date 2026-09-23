use super::*;
use crate::common::types::{ContentLevel, ContextNamespace, SearchResult, TianyanUri};
use crate::model::EmbeddingService;
use crate::test_utils::{cosine_similarity, MockEmbeddingService};
use crate::vfs::{
    ContentMetadata, ContentStore, ContextEntry, VectorPoint, VectorSearchQuery,
    VectorSearchResult, VectorStorage, VectorType, VfsCore, VfsSearch,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Mock in-memory vector storage for testing.
pub struct InMemoryVectorStorage {
    points: Arc<tokio::sync::RwLock<HashMap<String, VectorPoint>>>,
}

impl InMemoryVectorStorage {
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
    fn embedding_dim(&self) -> usize {
        8
    }

    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn upsert_point(&self, point: &VectorPoint) -> Result<()> {
        let mut points = self.points.write().await;
        points.insert(point.id.clone(), point.clone());
        Ok(())
    }

    async fn delete_point(&self, id: &str) -> Result<()> {
        let mut points = self.points.write().await;
        points.remove(id);
        Ok(())
    }

    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>> {
        let points = self.points.read().await;

        let mut results: Vec<VectorSearchResult> = points
            .values()
            .filter_map(|point| {
                let vector = match query.vector_type {
                    VectorType::Abstract => point.abstract_vector.as_ref()?,
                    VectorType::Overview => point.overview_vector.as_ref()?,
                    VectorType::Visual => point.visual_vector.as_ref()?,
                };

                if let Some(ref filter) = query.category_filter {
                    if point.payload.category.as_ref() != Some(filter) {
                        return None;
                    }
                }

                let score = cosine_similarity(&query.vector, vector);

                if let Some(min_score) = query.min_score {
                    if score < min_score {
                        return None;
                    }
                }

                Some(VectorSearchResult {
                    id: point.id.clone(),
                    score,
                    payload: point.payload.clone(),
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results.truncate(query.limit);

        Ok(results)
    }

    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>> {
        let points = self.points.read().await;
        Ok(points.get(id).cloned())
    }

    async fn search_fused(
        &self,
        query_vector: Vec<f32>,
        vector_types: &[VectorType],
        top_k: usize,
        category_filter: Option<&str>,
        min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>> {
        // 基本融合：逐列搜索，按点去重（保留最高分），按分数降序截断。
        let mut fused: Vec<VectorSearchResult> = Vec::new();
        for &vector_type in vector_types {
            let query = VectorSearchQuery {
                vector: query_vector.clone(),
                vector_type,
                limit: top_k,
                category_filter: category_filter.map(ToString::to_string),
                min_score,
            };
            for result in self.search(query).await? {
                match fused.iter_mut().find(|r| r.id == result.id) {
                    Some(existing) if result.score > existing.score => {
                        existing.score = result.score;
                        existing.payload = result.payload;
                    }
                    Some(_) => {}
                    None => fused.push(result),
                }
            }
        }
        fused.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        fused.truncate(top_k);
        Ok(fused)
    }
}

/// Test VFS that delegates to in-memory vector storage.
struct TestVfs {
    vector_storage: Arc<InMemoryVectorStorage>,
    embedding_service: Arc<MockEmbeddingService>,
}

impl TestVfs {
    fn new(
        vector_storage: Arc<InMemoryVectorStorage>,
        embedding_service: Arc<MockEmbeddingService>,
    ) -> Self {
        Self {
            vector_storage,
            embedding_service,
        }
    }
}

#[async_trait]
impl VfsCore for TestVfs {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }
    async fn exists(&self, _uri: &TianyanUri) -> Result<bool> {
        Ok(true)
    }
    async fn get_entry(&self, _uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_file(_uri.clone()))
    }
    async fn create_directory(&self, _uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_directory(_uri.clone()))
    }
    async fn create_file(&self, _uri: &TianyanUri) -> Result<ContextEntry> {
        Ok(ContextEntry::new_file(_uri.clone()))
    }
    async fn delete(&self, _uri: &TianyanUri) -> Result<()> {
        Ok(())
    }
    async fn list(&self, _uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        Ok(vec![])
    }
    async fn move_entry(&self, _source: &TianyanUri, _destination: &TianyanUri) -> Result<()> {
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
    async fn read(&self, _uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        Ok(match level {
            ContentLevel::Abstract => "Abstract content".to_string(),
            ContentLevel::Overview => "Overview content".to_string(),
            ContentLevel::Detail => "Detail content".to_string(),
        })
    }
    async fn append(&self, _uri: &TianyanUri, _content: &str) -> Result<()> {
        Ok(())
    }
    async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> {
        Ok(true)
    }
}

#[async_trait]
impl VfsSearch for TestVfs {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>> {
        let embedding = self.embedding_service.embed_single("test", query).await?;
        let query_obj = VectorSearchQuery {
            vector: embedding.vector,
            vector_type: VectorType::Abstract,
            limit,
            category_filter: namespace.map(|ns| ns.dir_name().to_string()),
            min_score: None,
        };
        let results = self.vector_storage.search(query_obj).await?;
        Ok(results
            .into_iter()
            .map(|r| SearchResult {
                uri: r.payload.uri.clone(),
                score: r.score,
            })
            .collect())
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

async fn create_test_retriever() -> DualLayerRetriever {
    let vector_storage = Arc::new(InMemoryVectorStorage::new());
    vector_storage.initialize().await.unwrap();
    let vfs: Arc<dyn VirtualFileSystem> =
        Arc::new(TestVfs::new(vector_storage, Arc::new(MockEmbeddingService)));

    DualLayerRetriever::new(vfs)
}

async fn create_test_retriever_with_data() -> (DualLayerRetriever, Arc<InMemoryVectorStorage>) {
    let vector_storage = Arc::new(InMemoryVectorStorage::new());
    vector_storage.initialize().await.unwrap();

    for i in 0..5 {
        let uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), format!("doc_{}", i)],
        );
        let payload = crate::common::types::EntryMetadata::new(uri.clone(), "unknown")
            .with_category("knowledge")
            .with_importance(0.5 + (i as f32 * 0.1))
            .with_tags(vec![format!("tag_{}", i)]);
        let point = VectorPoint {
            schema_version: crate::vfs::CURRENT_SCHEMA_VERSION,
            id: format!("doc_{}", i),
            abstract_vector: Some(vec![0.1 + (i as f32 * 0.01); 768]),
            overview_vector: Some(vec![0.2 + (i as f32 * 0.01); 768]),
            visual_vector: None,
            payload,
        };
        vector_storage.upsert_point(&point).await.unwrap();
    }

    let vfs: Arc<dyn VirtualFileSystem> = Arc::new(TestVfs::new(
        vector_storage.clone(),
        Arc::new(MockEmbeddingService),
    ));

    let retriever = DualLayerRetriever::new(vfs);

    (retriever, vector_storage)
}

// ==================== RetrievalResult Tests ====================

#[test]
fn test_retrieval_result() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc".to_string()]);
    let result = RetrievalResult::new(uri.clone(), 0.9);
    assert_eq!(result.uri, uri);
    assert_eq!(result.score, 0.9);
    assert!(!result.has_content());
}

#[test]
fn test_retrieval_result_with_content() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc".to_string()]);
    let result = RetrievalResult::new(uri, 0.9)
        .with_content("Test content".to_string(), ContentLevel::Overview);
    assert!(result.has_content());
    assert!(result.token_count > 0);
}

#[test]
fn test_retrieval_result_content_levels() {
    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);

    let result_abstract = RetrievalResult::new(uri.clone(), 0.8)
        .with_content("Abstract".to_string(), ContentLevel::Abstract);
    assert_eq!(result_abstract.content_level, ContentLevel::Abstract);

    let result_overview = RetrievalResult::new(uri.clone(), 0.9)
        .with_content("Overview".to_string(), ContentLevel::Overview);
    assert_eq!(result_overview.content_level, ContentLevel::Overview);

    let result_detail =
        RetrievalResult::new(uri, 0.95).with_content("Detail".to_string(), ContentLevel::Detail);
    assert_eq!(result_detail.content_level, ContentLevel::Detail);
}

// ==================== DualLayerRetriever Tests ====================

#[tokio::test]
async fn test_dual_layer_retriever_builder() {
    let retriever = create_test_retriever().await;
    assert_eq!(retriever.memory_bias, 1.15);
}

#[tokio::test]
async fn test_dual_layer_retriever_builder_with_options() {
    let vector_storage = Arc::new(InMemoryVectorStorage::new());
    vector_storage.initialize().await.unwrap();
    let vfs: Arc<dyn VirtualFileSystem> =
        Arc::new(TestVfs::new(vector_storage, Arc::new(MockEmbeddingService)));

    let retriever = DualLayerRetriever::new(vfs).with_memory_bias(2.0);

    assert_eq!(retriever.memory_bias, 2.0);
}

#[tokio::test]
async fn test_analyze_intent() {
    let retriever = create_test_retriever().await;
    let intent = retriever.analyze_intent("find my documents").await.unwrap();
    assert_eq!(intent.original_query, "find my documents");
    // 意图分析不做嵌入（避免与 VFS 内部嵌入重复）
    assert!(intent.target_scope.is_some());
}

#[tokio::test]
async fn test_fused_search_empty() {
    let retriever = create_test_retriever().await;
    let intent = retriever.analyze_intent("test").await.unwrap();
    let results = retriever
        .fused_search(&intent.original_query, 10, &intent)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_fused_search_with_data() {
    let (retriever, _) = create_test_retriever_with_data().await;
    let intent = retriever.analyze_intent("documents").await.unwrap();
    let results = retriever
        .fused_search(&intent.original_query, 10, &intent)
        .await
        .unwrap();

    assert!(!results.is_empty());
    assert!(results.len() <= 10);
}

#[tokio::test]
async fn test_fused_search_with_category_filter() {
    let (retriever, _) = create_test_retriever_with_data().await;

    let intent = Intent::new("documents").with_scope(
        crate::context::retrieval::intent::TargetScope::new(ContextNamespace::Knowledge),
    );

    let results = retriever
        .fused_search(&intent.original_query, 10, &intent)
        .await
        .unwrap();

    for result in &results {
        assert_eq!(result.uri.namespace().to_string(), "knowledge");
    }
}

#[tokio::test]
async fn test_retrieve_empty_storage() {
    let retriever = create_test_retriever().await;
    let results = retriever.retrieve("test query", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_retrieve_with_data() {
    let (retriever, _) = create_test_retriever_with_data().await;
    let results = retriever.retrieve("documents", 5).await.unwrap();

    for result in &results {
        assert!(result.has_content());
    }
}

/// 回归测试：高相似度结果必须能加载 Overview 层级内容（ADR-001 修订后
/// **L2 不再自动加载**——命中即 L1：简介 + 目录）。
///
/// 曾因融合搜索暴露排名倒数分数（RRF，≈0.016）而 `ContentLoadStrategy`
/// 阈值按余弦相似度标定（0.6），导致所有结果永远停留在 Abstract。
#[tokio::test]
async fn test_retrieve_loads_overview_or_detail_for_high_scores() {
    let (retriever, _) = create_test_retriever_with_data().await;
    let results = retriever.retrieve("documents", 5).await.unwrap();

    assert!(!results.is_empty());
    let levels: Vec<ContentLevel> = results.iter().map(|r| r.content_level).collect();
    assert!(
        levels.iter().any(|l| *l != ContentLevel::Abstract),
        "高相似度结果的内容层级全部为 Abstract，L1/L2 渐进加载失效: {:?}",
        levels
    );
}

/// T1-4（主回归）：显式 namespace 检索不得被意图推断短路。
///
/// 判别力：旧实现先走 `retrieve()`（"search for documents" 的意图推断 →
/// Knowledge 范围）再在结果上过滤 Memory——只搜了 Knowledge，过滤后必空；
/// 修复后 namespace 直接下推 VFS 搜索，命中 Memory 条目。
#[tokio::test]
async fn test_retrieve_by_namespace_bypasses_intent_scope() {
    let (retriever, storage) = create_test_retriever_with_data().await;
    // 追加一条 Memory 条目（知识库数据之外）
    let uri = TianyanUri::new(
        ContextNamespace::Memory,
        vec!["preferences".to_string(), "p1".to_string()],
    );
    let payload = crate::common::types::EntryMetadata::new(uri.clone(), "unknown")
        .with_category("memory")
        .with_importance(0.9);
    let point = VectorPoint {
        schema_version: crate::vfs::CURRENT_SCHEMA_VERSION,
        id: "mem_p1".to_string(),
        abstract_vector: Some(vec![0.1; 768]),
        overview_vector: Some(vec![0.2; 768]),
        visual_vector: None,
        payload,
    };
    storage.upsert_point(&point).await.unwrap();

    let results = retriever
        .retrieve_by_namespace("search for documents", 5, ContextNamespace::Memory)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "显式 Memory 检索不得为空（T1-4：旧行为被 Knowledge 意图短路后过滤为空）"
    );
    assert_eq!(
        results[0].uri.namespace().to_string(),
        "memory",
        "结果必须来自 Memory 命名空间"
    );
}

/// P0-7 回归：系统/运维路径（归档/评审）不得作为检索结果。
///
/// 存量污染形态：历史索引里残留归档规则向量点（新内容已由摘要与索引采集
/// 侧拦截，此处兜底过滤）。
#[tokio::test]
async fn test_retrieve_filters_system_paths() {
    let (retriever, storage) = create_test_retriever_with_data().await;

    let live = TianyanUri::new(
        ContextNamespace::Agent,
        vec!["learned".to_string(), "live-rule".to_string()],
    );
    let archived = TianyanUri::new(
        ContextNamespace::Agent,
        vec![
            "learned".to_string(),
            "archive".to_string(),
            "stale-rule".to_string(),
        ],
    );
    for uri in [&live, &archived] {
        let payload =
            crate::common::types::EntryMetadata::new(uri.clone(), "unknown").with_category("agent");
        let point = VectorPoint {
            schema_version: crate::vfs::CURRENT_SCHEMA_VERSION,
            id: uri.to_point_id(),
            abstract_vector: Some(vec![0.1; 768]),
            overview_vector: Some(vec![0.2; 768]),
            visual_vector: None,
            payload,
        };
        storage.upsert_point(&point).await.unwrap();
    }

    let results = retriever
        .retrieve_by_namespace("rule", 20, ContextNamespace::Agent)
        .await
        .unwrap();
    let uris: Vec<&str> = results.iter().map(|r| r.uri.as_str()).collect();
    assert!(
        uris.iter().any(|u| u.contains("live-rule")),
        "活跃规则应可检索: {uris:?}"
    );
    assert!(
        !uris.iter().any(|u| u.contains("archive")),
        "归档路径不得作为检索结果: {uris:?}"
    );
}

// ==================== Builder Removed ====================
// DualLayerRetrieverBuilder removed — used only in these tests, never in production.
// DualLayerRetriever::new() + with_*() methods provide the same functionality directly.

// ==================== Token Estimation Tests ====================

#[test]
fn test_estimate_tokens_basic() {
    assert!(estimate_tokens("hello") > 0);
    assert!(estimate_tokens("hello world!") > 0);
}

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_long() {
    let long_text = "a".repeat(100);
    let tokens = estimate_tokens(&long_text);
    assert!(tokens > 0);
}
