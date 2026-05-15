//! 双层检索器实现。
//!
//! 本模块实现基于多向量融合的检索系统：
//! - 使用 RRF (Reciprocal Rank Fusion) 算法融合 abstract 和 overview 向量搜索结果

use std::sync::Arc;

use tracing::{debug, info, instrument};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, TianyanUri};
use crate::context::compression::estimate_tokens;
use crate::context::types::{compute_freshness, RetrievalResult, DEFAULT_FRESHNESS_HALF_LIFE_DAYS};
use crate::model::EmbeddingService;
use crate::storage::{
    ContentLoader, VectorSearchQuery, VectorSearchResult, VectorStorage, VectorType,
};

use super::intent::{Intent, IntentAnalyzer};
use super::loader::{ContentLoadStrategy, TokenBudget};
use super::trace::RetrievalTraceBuilder;

/// 融合语义相关性分数与新鲜度分数的组合评分。
///
/// - `semantic_score`: 原始向量检索/偏置分数
/// - `freshness_score`: 内容新鲜度
/// - `freshness_weight`: 新鲜度权重（默认 0.2）
const DEFAULT_FRESHNESS_WEIGHT: f32 = 0.2;

fn compute_combined_score(semantic_score: f32, freshness_score: f32) -> f32 {
    semantic_score * (1.0 - DEFAULT_FRESHNESS_WEIGHT) + freshness_score * DEFAULT_FRESHNESS_WEIGHT
}

impl RetrievalResult {
    /// 从向量搜索结果创建。
    pub fn from_search_result(result: VectorSearchResult) -> Self {
        let uri = result.payload.uri.clone();
        let category = uri.namespace().to_string();
        let source_updated_at = Some(result.payload.updated_at);
        let age_days = (chrono::Utc::now() - result.payload.updated_at).num_days();
        let freshness_score = compute_freshness(age_days, DEFAULT_FRESHNESS_HALF_LIFE_DAYS);
        Self {
            uri,
            score: result.score,
            content: None,
            content_level: ContentLevel::Abstract,
            token_count: 0,
            category,
            tags: result.payload.tags.clone(),
            source_updated_at,
            freshness_score,
        }
    }

    /// 设置内容。
    pub fn with_content(mut self, content: String, level: ContentLevel) -> Self {
        self.token_count = estimate_tokens(&content);
        self.content = Some(content);
        self.content_level = level;
        self
    }
}

/// 上下文检索器的抽象 trait。
///
/// 使 Agent 和 ContextPipeline 可以依赖 trait 而非具体 `DualLayerRetriever` 类型，
/// 便于单元测试 mock 和未来的检索器替代实现。
#[async_trait::async_trait]
pub trait ContextRetriever: Send + Sync {
    /// 获取默认 Token 预算。
    fn default_token_budget(&self) -> usize;

    /// 带 Token 预算的检索。
    async fn retrieve_with_budget(
        &self,
        query: &str,
        top_k: usize,
        budget: usize,
    ) -> Result<Vec<RetrievalResult>>;

    /// 按命名空间过滤检索。
    async fn retrieve_by_namespace(
        &self,
        query: &str,
        top_k: usize,
        namespace: crate::common::types::ContextNamespace,
    ) -> Result<Vec<RetrievalResult>>;
}

/// 用于基于向量的内容检索的融合检索器。
///
/// 此检索器使用 RRF 算法融合多个命名向量的搜索结果，
/// 提供更准确和全面的检索能力。
pub struct DualLayerRetriever {
    /// 用于相似性搜索的向量存储。
    vector_storage: Arc<dyn VectorStorage>,
    /// 用于查询向量化的嵌入服务。
    embedding_service: Option<Arc<dyn EmbeddingService>>,
    /// 用于加载结果内容的内容加载器。
    content_loader: Option<Arc<dyn ContentLoader>>,
    /// 意图分析器。
    intent_analyzer: IntentAnalyzer,
    /// 使用的嵌入模型。
    embedding_model: String,
    /// 默认 Token 预算。
    default_token_budget: usize,
    /// Memory 命名空间的分数偏置倍数。
    memory_bias: f32,
    /// 记忆衰减率（每日，0.0-1.0），越旧的记忆偏置越低。
    memory_decay_rate: f32,
}

impl DualLayerRetriever {
    /// 创建新的融合检索器。
    pub fn new(vector_storage: Arc<dyn VectorStorage>) -> Self {
        Self {
            vector_storage,
            embedding_service: None,
            content_loader: None,
            intent_analyzer: IntentAnalyzer::new(),
            embedding_model: "text-embedding-3-small".to_string(),
            default_token_budget: 4096,
            memory_bias: 1.15,
            memory_decay_rate: 0.01,
        }
    }

    /// 设置嵌入服务。
    pub fn with_embedding_service(mut self, service: Arc<dyn EmbeddingService>) -> Self {
        self.embedding_service = Some(service.clone());
        self.intent_analyzer =
            IntentAnalyzer::new().with_embedding_service(service, &self.embedding_model);
        self
    }

    /// 设置内容加载器。
    pub fn with_content_loader(mut self, loader: Arc<dyn ContentLoader>) -> Self {
        self.content_loader = Some(loader);
        self
    }

    /// 设置嵌入模型。
    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = model.into();
        self
    }

    /// 设置默认 Token 预算。
    pub fn with_token_budget(mut self, budget: usize) -> Self {
        self.default_token_budget = budget;
        self
    }

    /// 设置 Memory 命名空间的分数偏置倍数。
    pub fn with_memory_bias(mut self, bias: f32) -> Self {
        self.memory_bias = bias;
        self
    }

    /// 设置记忆衰减率（每日）。
    pub fn with_memory_decay_rate(mut self, rate: f32) -> Self {
        self.memory_decay_rate = rate;
        self
    }

    /// 检索查询的内容。
    #[instrument(skip(self), fields(query = %query))]
    pub async fn retrieve(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalResult>> {
        let mut trace_builder = RetrievalTraceBuilder::new(query);

        // 步骤 1：意图分。
        let intent = self.analyze_intent(query).await?;
        trace_builder.add_intent_analysis(intent.token_count);

        // 步骤 2：获取查询向。
        let query_vector = self.get_query_vector(&intent).await?;

        // 步骤 3：融合搜索（abstract + overview 向量。
        let results = self.fused_search(&query_vector, top_k, &intent).await?;
        debug!("Fused search returned {} results", results.len());

        for result in &results {
            trace_builder.add_l1_search(result.uri.clone(), result.score, 0);
        }

        // 步骤 4：加载内。
        let results = self.load_content_for_results(results).await?;
        for result in &results {
            if result.has_content() {
                trace_builder.add_content_load(result.uri.clone(), result.token_count);
            }
        }

        // 构建最终追踪记。
        let trace = trace_builder.build();
        info!(
            query = %query,
            total_time_ms = trace.total_time_ms,
            total_tokens = trace.total_tokens,
            result_count = results.len(),
            "Retrieval completed"
        );

        Ok(results)
    }

    /// 使用预计算的意图进行检索。
    pub async fn retrieve_with_intent(
        &self,
        intent: &Intent,
        top_k: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let query_vector = self.get_query_vector(intent).await?;
        let results = self.fused_search(&query_vector, top_k, intent).await?;
        self.load_content_for_results(results).await
    }

    /// 带有 Token 预算的检索。
    pub async fn retrieve_with_budget(
        &self,
        query: &str,
        top_k: usize,
        budget: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let intent = self.analyze_intent(query).await?;
        let query_vector = self.get_query_vector(&intent).await?;
        let results = self.fused_search(&query_vector, top_k, &intent).await?;

        // 带预算加载内。
        self.load_content_with_budget(results, budget).await
    }

    /// 分析查询意图。
    async fn analyze_intent(&self, query: &str) -> Result<Intent> {
        self.intent_analyzer.analyze(query).await
    }

    /// 获取查询向量。
    async fn get_query_vector(&self, intent: &Intent) -> Result<Vec<f32>> {
        if let Some(ref vector) = intent.query_vector {
            return Ok(vector.clone());
        }

        if let Some(ref service) = self.embedding_service {
            let embedding = service
                .embed_single(&self.embedding_model, &intent.original_query)
                .await?;
            return Ok(embedding.vector);
        }

        Err(TianyanError::Retrieval(
            "没有可用的嵌入服务且无预计算向量".to_string(),
        ))
    }

    /// 执行融合搜索（abstract + overview 向量）。
    ///
    /// 使用 RRF 算法融合 abstract 和 overview 两个向量的搜索结果。
    async fn fused_search(
        &self,
        query_vector: &[f32],
        top_k: usize,
        intent: &Intent,
    ) -> Result<Vec<RetrievalResult>> {
        let results = self
            .vector_storage
            .search_abstract_and_overview(query_vector.to_vec(), top_k, intent.category_filter())
            .await?;

        let mut results: Vec<RetrievalResult> = results
            .into_iter()
            .map(RetrievalResult::from_search_result)
            .collect();

        // Memory namespace 结果获得分数偏置，确保记忆优先于通用知识
        // 偏置随记忆年龄衰减（来自 MemoryConfig.decay_rate）
        for result in &mut results {
            if result.category == "memory" {
                let bias = self.memory_bias;
                result.score *= bias;
            }
        }

        // 将新鲜度评分融入最终分数
        for result in &mut results {
            result.score = compute_combined_score(result.score, result.freshness_score);
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    /// 为结果加载内容。
    async fn load_content_for_results(
        &self,
        results: Vec<RetrievalResult>,
    ) -> Result<Vec<RetrievalResult>> {
        let Some(loader) = &self.content_loader else {
            return Ok(results);
        };

        let mut loaded_results = Vec::with_capacity(results.len());

        for mut result in results {
            let strategy = ContentLoadStrategy::from_score(result.score);
            let level = strategy.to_content_level();

            match loader.load_content(&result.uri, level).await {
                Ok(content) => {
                    result.content = Some(content.clone());
                    result.content_level = level;
                    result.token_count = estimate_tokens(&content);
                }
                Err(e) => {
                    tracing::warn!(uri = %result.uri, error = %e, "加载内容失败");
                    // 尝试加载摘要作为回退
                    if level != ContentLevel::Abstract {
                        if let Ok(content) = loader
                            .load_content(&result.uri, ContentLevel::Abstract)
                            .await
                        {
                            result.content = Some(content.clone());
                            result.content_level = ContentLevel::Abstract;
                            result.token_count = estimate_tokens(&content);
                        }
                    }
                }
            }

            loaded_results.push(result);
        }

        Ok(loaded_results)
    }

    /// 带有 Token 预算加载内容。
    async fn load_content_with_budget(
        &self,
        results: Vec<RetrievalResult>,
        budget: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let Some(loader) = &self.content_loader else {
            return Ok(results);
        };

        let mut token_budget = TokenBudget::new(budget);
        let mut loaded_results = Vec::new();

        for mut result in results {
            let strategy = ContentLoadStrategy::from_score_and_budget(result.score, &token_budget);
            let level = strategy.to_content_level();

            match loader.load_content(&result.uri, level).await {
                Ok(content) => {
                    let tokens = estimate_tokens(&content);
                    if token_budget.can_afford(tokens) {
                        token_budget.use_tokens(tokens)?;
                        result.content = Some(content);
                        result.content_level = level;
                        result.token_count = tokens;
                    }
                }
                Err(e) => {
                    tracing::warn!(uri = %result.uri, error = %e, "加载内容失败");
                }
            }

            loaded_results.push(result);
        }

        Ok(loaded_results)
    }

    /// 加载特定 URI 的内容。
    pub async fn load_content(&self, uri: &TianyanUri) -> Result<String> {
        let Some(loader) = &self.content_loader else {
            return Err(TianyanError::Retrieval("内容加载器不可用".to_string()));
        };

        loader.load_content(uri, ContentLevel::Overview).await
    }

    /// 加载特定层级的内容。
    pub async fn load_content_at_level(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
    ) -> Result<String> {
        let Some(loader) = &self.content_loader else {
            return Err(TianyanError::Retrieval("内容加载器不可用".to_string()));
        };

        loader.load_content(uri, level).await
    }

    /// 通过视觉嵌入搜索（用于图像相似性搜索）。
    pub async fn search_by_visual(
        &self,
        visual_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<RetrievalResult>> {
        let query = VectorSearchQuery {
            vector: visual_vector.to_vec(),
            vector_type: VectorType::Visual,
            limit: top_k,
            category_filter: None,
            min_score: None,
        };

        let results = self.vector_storage.search(query).await?;
        let results = results
            .into_iter()
            .map(RetrievalResult::from_search_result)
            .collect();

        self.load_content_for_results(results).await
    }

    /// 获取向量存储。
    pub fn vector_storage(&self) -> &dyn VectorStorage {
        self.vector_storage.as_ref()
    }

    /// 获取默认 Token 预算。
    pub fn default_token_budget(&self) -> usize {
        self.default_token_budget
    }

    /// 使用命名空间过滤器检索内容。
    ///
    /// 先执行标准检索，然后过滤结果只保留满足条件的命名空间。
    ///
    /// - `query` - 查询内容
    /// - `top_k` - 返回数量限制
    /// - `filter` - 命名空间过滤函数
    pub async fn retrieve_with_namespace_filter<F>(
        &self,
        query: &str,
        top_k: usize,
        filter: F,
    ) -> Result<Vec<RetrievalResult>>
    where
        F: Fn(crate::common::types::ContextNamespace) -> bool,
    {
        let results = self.retrieve(query, top_k * 2).await?;
        let filtered: Vec<_> = results
            .into_iter()
            .filter(|r| filter(r.uri.namespace()))
            .take(top_k)
            .collect();
        Ok(filtered)
    }
}

#[async_trait::async_trait]
impl ContextRetriever for DualLayerRetriever {
    fn default_token_budget(&self) -> usize {
        self.default_token_budget
    }

    async fn retrieve_with_budget(
        &self,
        query: &str,
        top_k: usize,
        budget: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieve_with_budget(query, top_k, budget).await
    }

    async fn retrieve_by_namespace(
        &self,
        query: &str,
        top_k: usize,
        namespace: crate::common::types::ContextNamespace,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieve_with_namespace_filter(query, top_k, |ns| ns == namespace)
            .await
    }
}

/// 用于创建融合检索器实例的构建器。
pub struct DualLayerRetrieverBuilder {
    vector_storage: Option<Arc<dyn VectorStorage>>,
    embedding_service: Option<Arc<dyn EmbeddingService>>,
    content_loader: Option<Arc<dyn ContentLoader>>,
    embedding_model: Option<String>,
    default_token_budget: Option<usize>,
}

impl DualLayerRetrieverBuilder {
    /// 创建新的构建器。
    pub fn new() -> Self {
        Self {
            vector_storage: None,
            embedding_service: None,
            content_loader: None,
            embedding_model: None,
            default_token_budget: None,
        }
    }

    /// 设置向量存储（必需）。
    pub fn with_vector_storage(mut self, storage: Arc<dyn VectorStorage>) -> Self {
        self.vector_storage = Some(storage);
        self
    }

    /// 设置嵌入服务。
    pub fn with_embedding_service(mut self, service: Arc<dyn EmbeddingService>) -> Self {
        self.embedding_service = Some(service);
        self
    }

    /// 设置内容加载器。
    pub fn with_content_loader(mut self, loader: Arc<dyn ContentLoader>) -> Self {
        self.content_loader = Some(loader);
        self
    }

    /// 设置嵌入模型。
    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = Some(model.into());
        self
    }

    /// 设置默认 Token 预算。
    pub fn with_token_budget(mut self, budget: usize) -> Self {
        self.default_token_budget = Some(budget);
        self
    }

    /// 构建检索器。
    pub fn build(self) -> Result<DualLayerRetriever> {
        let vector_storage = self
            .vector_storage
            .ok_or_else(|| TianyanError::Internal("向量存储不可用".to_string()))?;

        let mut retriever = DualLayerRetriever::new(vector_storage);

        if let Some(service) = self.embedding_service {
            retriever = retriever.with_embedding_service(service);
        }

        if let Some(loader) = self.content_loader {
            retriever = retriever.with_content_loader(loader);
        }

        if let Some(model) = self.embedding_model {
            retriever = retriever.with_embedding_model(model);
        }

        if let Some(budget) = self.default_token_budget {
            retriever = retriever.with_token_budget(budget);
        }

        Ok(retriever)
    }
}

impl Default for DualLayerRetrieverBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;
    use crate::model::{EmbeddingData, EmbeddingRequest, EmbeddingResponse};
    use crate::storage::{
        ContextEntry, VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType,
    };
    use async_trait::async_trait;

    /// Mock embedding service for testing.
    struct MockEmbeddingService;

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

        fn service_name(&self) -> &str {
            "mock"
        }
    }

    /// Mock content loader for testing.
    struct MockContentLoader;

    #[async_trait]
    impl ContentLoader for MockContentLoader {
        async fn load_content(&self, _uri: &TianyanUri, level: ContentLevel) -> Result<String> {
            Ok(match level {
                ContentLevel::Abstract => "Abstract content".to_string(),
                ContentLevel::Overview => "Overview content".to_string(),
                ContentLevel::Detail => "Detail content".to_string(),
            })
        }

        async fn load_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
            Ok(ContextEntry::new_file(uri.clone()))
        }

        async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> {
            Ok(true)
        }
    }

    /// Mock in-memory vector storage for testing.
    pub struct InMemoryVectorStorage {
        points: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, VectorPoint>>>,
    }

    impl InMemoryVectorStorage {
        pub fn new() -> Self {
            Self {
                points: std::sync::Arc::new(tokio::sync::RwLock::new(
                    std::collections::HashMap::new(),
                )),
            }
        }
    }

    impl Default for InMemoryVectorStorage {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl crate::storage::VectorStorage for InMemoryVectorStorage {
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

                    // Apply category filter
                    if let Some(ref filter) = query.category_filter {
                        if point.payload.category.as_ref() != Some(filter) {
                            return None;
                        }
                    }

                    // Calculate cosine similarity
                    let score = cosine_similarity(&query.vector, vector);

                    // Apply minimum score filter
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

            // Sort by score descending
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // Limit results
            results.truncate(query.limit);

            Ok(results)
        }

        async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>> {
            let points = self.points.read().await;
            Ok(points.get(id).cloned())
        }

        async fn update_vector(
            &self,
            uri: &TianyanUri,
            vector_type: VectorType,
            vector: &[f32],
        ) -> Result<()> {
            let id = uri.to_string().replace("://", "_").replace('/', "_");
            let mut points = self.points.write().await;

            if let Some(point) = points.get_mut(&id) {
                match vector_type {
                    VectorType::Abstract => point.abstract_vector = Some(vector.to_vec()),
                    VectorType::Overview => point.overview_vector = Some(vector.to_vec()),
                    VectorType::Visual => point.visual_vector = Some(vector.to_vec()),
                }
            } else {
                let payload = crate::common::types::EntryMetadata::new(uri.clone(), "unknown");

                let point = VectorPoint {
                    schema_version: crate::storage::CURRENT_SCHEMA_VERSION,
                    id: id.clone(),
                    abstract_vector: if vector_type == VectorType::Abstract {
                        Some(vector.to_vec())
                    } else {
                        None
                    },
                    overview_vector: if vector_type == VectorType::Overview {
                        Some(vector.to_vec())
                    } else {
                        None
                    },
                    visual_vector: if vector_type == VectorType::Visual {
                        Some(vector.to_vec())
                    } else {
                        None
                    },
                    payload,
                };
                points.insert(id, point);
            }

            Ok(())
        }

        async fn count_points(&self) -> Result<usize> {
            let points = self.points.read().await;
            Ok(points.len())
        }

        async fn clear(&self) -> Result<()> {
            let mut points = self.points.write().await;
            points.clear();
            Ok(())
        }
    }

    /// Calculate cosine similarity between two vectors.
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

    async fn create_test_retriever() -> DualLayerRetriever {
        let vector_storage: Arc<dyn crate::storage::VectorStorage> =
            Arc::new(InMemoryVectorStorage::new());
        vector_storage.initialize().await.unwrap();

        DualLayerRetrieverBuilder::new()
            .with_vector_storage(vector_storage)
            .with_embedding_service(Arc::new(MockEmbeddingService))
            .with_content_loader(Arc::new(MockContentLoader))
            .build()
            .unwrap()
    }

    async fn create_test_retriever_with_data() -> (DualLayerRetriever, Arc<InMemoryVectorStorage>) {
        let vector_storage = Arc::new(InMemoryVectorStorage::new());
        vector_storage.initialize().await.unwrap();

        // Add some test data
        for i in 0..5 {
            let uri = TianyanUri::new(
                ContextNamespace::Knowledge,
                vec!["documents".to_string(), format!("doc_{}", i)],
            );
            // Note: category_filter checks payload.category, so we set it to "knowledge"
            // to match the URI's category for the filter to work
            let payload = crate::common::types::EntryMetadata::new(uri.clone(), "unknown")
                .with_category("knowledge")
                .with_importance(0.5 + (i as f32 * 0.1))
                .with_tags(vec![format!("tag_{}", i)]);
            let point = VectorPoint {
                schema_version: crate::storage::CURRENT_SCHEMA_VERSION,
                id: format!("doc_{}", i),
                abstract_vector: Some(vec![0.1 + (i as f32 * 0.01); 768]),
                overview_vector: Some(vec![0.2 + (i as f32 * 0.01); 768]),
                visual_vector: None,
                payload,
            };
            vector_storage.upsert_point(&point).await.unwrap();
        }

        let retriever = DualLayerRetrieverBuilder::new()
            .with_vector_storage(vector_storage.clone())
            .with_embedding_service(Arc::new(MockEmbeddingService))
            .with_content_loader(Arc::new(MockContentLoader))
            .build()
            .unwrap();

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
    fn test_retrieval_result_from_search_result() {
        let uri = TianyanUri::new(ContextNamespace::Memory, vec!["session".to_string()]);
        let payload = crate::common::types::EntryMetadata::new(uri.clone(), "unknown")
            .with_category("memory")
            .with_importance(0.7)
            .with_tags(vec!["test".to_string()]);
        let search_result = crate::storage::VectorSearchResult {
            id: "test_id".to_string(),
            score: 0.85,
            payload,
        };

        let result = RetrievalResult::from_search_result(search_result);
        assert_eq!(result.uri, uri);
        assert!((result.score - 0.85).abs() < 0.001);
        assert_eq!(result.category, "memory");
    }

    #[test]
    fn test_retrieval_result_content_levels() {
        let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);

        // Test with abstract level
        let result_abstract = RetrievalResult::new(uri.clone(), 0.8)
            .with_content("Abstract".to_string(), ContentLevel::Abstract);
        assert_eq!(result_abstract.content_level, ContentLevel::Abstract);

        // Test with overview level
        let result_overview = RetrievalResult::new(uri.clone(), 0.9)
            .with_content("Overview".to_string(), ContentLevel::Overview);
        assert_eq!(result_overview.content_level, ContentLevel::Overview);

        // Test with detail level
        let result_detail = RetrievalResult::new(uri, 0.95)
            .with_content("Detail".to_string(), ContentLevel::Detail);
        assert_eq!(result_detail.content_level, ContentLevel::Detail);
    }

    // ==================== DualLayerRetriever Tests ====================

    #[tokio::test]
    async fn test_dual_layer_retriever_builder() {
        let retriever = create_test_retriever().await;
        assert_eq!(retriever.embedding_model, "text-embedding-3-small");
    }

    #[tokio::test]
    async fn test_dual_layer_retriever_builder_with_options() {
        let vector_storage = Arc::new(InMemoryVectorStorage::new());
        vector_storage.initialize().await.unwrap();

        let retriever = DualLayerRetrieverBuilder::new()
            .with_vector_storage(vector_storage)
            .with_embedding_service(Arc::new(MockEmbeddingService))
            .with_content_loader(Arc::new(MockContentLoader))
            .with_embedding_model("custom-model")
            .with_token_budget(8192)
            .build()
            .unwrap();

        assert_eq!(retriever.embedding_model, "custom-model");
        assert_eq!(retriever.default_token_budget, 8192);
    }

    #[tokio::test]
    async fn test_analyze_intent() {
        let retriever = create_test_retriever().await;
        let intent = retriever.analyze_intent("find my documents").await.unwrap();
        assert_eq!(intent.original_query, "find my documents");
        assert!(intent.query_vector.is_some());
    }

    #[tokio::test]
    async fn test_get_query_vector() {
        let retriever = create_test_retriever().await;
        let intent = retriever.analyze_intent("test query").await.unwrap();
        let vector = retriever.get_query_vector(&intent).await.unwrap();
        assert_eq!(vector.len(), 768);
    }

    #[tokio::test]
    async fn test_fused_search_empty() {
        let retriever = create_test_retriever().await;
        let intent = retriever.analyze_intent("test").await.unwrap();
        let vector = retriever.get_query_vector(&intent).await.unwrap();
        let results = retriever.fused_search(&vector, 10, &intent).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_fused_search_with_data() {
        let (retriever, _) = create_test_retriever_with_data().await;
        let intent = retriever.analyze_intent("documents").await.unwrap();
        let vector = retriever.get_query_vector(&intent).await.unwrap();
        let results = retriever.fused_search(&vector, 10, &intent).await.unwrap();

        // Should return results from the test data
        assert!(!results.is_empty());
        assert!(results.len() <= 10);
    }

    #[tokio::test]
    async fn test_fused_search_with_category_filter() {
        let (retriever, _) = create_test_retriever_with_data().await;

        // Create intent with category filter
        let intent = crate::context::retrieval::intent::Intent::new("documents")
            .with_vector(vec![0.1; 768])
            .with_scope(crate::context::retrieval::intent::TargetScope::new(
                ContextNamespace::Knowledge,
            ));

        let vector = retriever.get_query_vector(&intent).await.unwrap();
        let results = retriever.fused_search(&vector, 10, &intent).await.unwrap();

        // All results should be in the knowledge category
        for result in &results {
            assert_eq!(result.category, "knowledge");
        }
    }

    #[tokio::test]
    async fn test_load_content() {
        let retriever = create_test_retriever().await;
        let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
        let content = retriever.load_content(&uri).await.unwrap();
        assert_eq!(content, "Overview content");
    }

    #[tokio::test]
    async fn test_load_content_at_level() {
        let retriever = create_test_retriever().await;
        let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);

        let abstract_content = retriever
            .load_content_at_level(&uri, ContentLevel::Abstract)
            .await
            .unwrap();
        assert_eq!(abstract_content, "Abstract content");

        let overview_content = retriever
            .load_content_at_level(&uri, ContentLevel::Overview)
            .await
            .unwrap();
        assert_eq!(overview_content, "Overview content");

        let detail_content = retriever
            .load_content_at_level(&uri, ContentLevel::Detail)
            .await
            .unwrap();
        assert_eq!(detail_content, "Detail content");
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

        // Should return results with content loaded
        for result in &results {
            assert!(result.has_content());
        }
    }

    #[tokio::test]
    async fn test_retrieve_with_budget() {
        let (retriever, _) = create_test_retriever_with_data().await;
        let results = retriever
            .retrieve_with_budget("documents", 10, 1000)
            .await
            .unwrap();

        // Results should respect token budget
        let total_tokens: usize = results.iter().map(|r| r.token_count).sum();
        assert!(total_tokens <= 1000 || results.is_empty());
    }

    #[tokio::test]
    async fn test_search_by_visual() {
        let (retriever, _) = create_test_retriever_with_data().await;

        // Add a point with visual vector
        let uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["images".to_string(), "img_1".to_string()],
        );
        let payload = crate::common::types::EntryMetadata::new(uri.clone(), "unknown")
            .with_category("images")
            .with_importance(0.8)
            .with_tags(vec!["image".to_string()]);
        let point = VectorPoint {
            schema_version: crate::storage::CURRENT_SCHEMA_VERSION,
            id: "img_1".to_string(),
            abstract_vector: Some(vec![0.1; 768]),
            overview_vector: Some(vec![0.2; 768]),
            visual_vector: Some(vec![0.5; 512]),
            payload,
        };
        retriever
            .vector_storage()
            .upsert_point(&point)
            .await
            .unwrap();

        // Search by visual vector
        let visual_query = vec![0.5; 512];
        let results = retriever.search_by_visual(&visual_query, 10).await.unwrap();

        assert!(!results.is_empty());
    }

    // ==================== Builder Tests ====================

    #[test]
    fn test_builder_defaults() {
        let builder = DualLayerRetrieverBuilder::new();
        assert!(builder.vector_storage.is_none());
        assert!(builder.embedding_service.is_none());
    }

    #[test]
    fn test_builder_default() {
        let builder = DualLayerRetrieverBuilder::default();
        assert!(builder.vector_storage.is_none());
    }

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
}
