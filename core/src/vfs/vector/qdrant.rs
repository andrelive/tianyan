//! Qdrant 向量存储实现。

use async_trait::async_trait;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{EntryMetadata, TianyanUri};
use crate::vfs::vector::VectorStorage;
use crate::vfs::types::{
    VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType, CURRENT_SCHEMA_VERSION,
};

use crate::config::StorageConfig;

/// 融合搜索的默认最低分数阈值。
const L1_MIN_SCORE: f32 = 0.5;

/// Qdrant 向量存储实现。
///
/// 此实现使用 qdrant-client 库进行向量存储。
/// 支持每个点的多向量存储（摘要、概览、视觉）。
pub struct QdrantVectorStore {
    client: qdrant_client::Qdrant,
    collection_name: String,
    embedding_dimension: usize,
    visual_embedding_dimension: usize,
}

impl QdrantVectorStore {
    /// 使用配置创建新的 Qdrant 向量存储。
    pub fn new(config: &StorageConfig) -> Result<Self> {
        let url = &config.vector.url;
        let client = qdrant_client::Qdrant::from_url(url)
            .build()
            .map_err(|e| TianyanError::VectorDatabase(format!("创建 Qdrant 客户端失败：{}", e)))?;

        Ok(Self {
            client,
            collection_name: config.vector.collection_name.clone(),
            embedding_dimension: config.vector.vector_dimension,
            visual_embedding_dimension: config.vector.vector_dimension / 3,
        })
    }

    /// 使用自定义 URL 创建新的 Qdrant 向量存储。
    pub fn with_url(config: &StorageConfig, url: &str) -> Result<Self> {
        let client = qdrant_client::Qdrant::from_url(url).build().map_err(|e| {
            TianyanError::VectorDatabase(format!(
                "创建 Qdrant 客户端失败：{}",
                e
            ))
        })?;

        Ok(Self {
            client,
            collection_name: config.vector.collection_name.clone(),
            embedding_dimension: config.vector.vector_dimension,
            visual_embedding_dimension: config.vector.vector_dimension / 3,
        })
    }

    /// 从现有客户端创建 Qdrant 向量存储。
    pub fn with_client(
        client: qdrant_client::Qdrant,
        collection_name: String,
        embedding_dimension: usize,
        visual_embedding_dimension: usize,
    ) -> Self {
        Self {
            client,
            collection_name,
            embedding_dimension,
            visual_embedding_dimension,
        }
    }

    /// 获取集合名称。
    pub fn collection_name(&self) -> &str {
        &self.collection_name
    }

    /// 如果集合不存在则创建。
    pub async fn ensure_collection(&self) -> Result<()> {
        use qdrant_client::qdrant::{
            CreateCollectionBuilder, Distance, VectorParamsBuilder, VectorsConfigBuilder,
        };

        // 检查集合是否存在
        let collections = self
            .client
            .list_collections()
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("列出集合失败: {}", e)))?;

        let exists = collections
            .collections
            .iter()
            .any(|c| c.name == self.collection_name);

        if !exists {
            // 创建具有多个命名向量的集合
            let mut vector_config = VectorsConfigBuilder::default();
            vector_config.add_named_vector_params(
                "abstract",
                VectorParamsBuilder::new(self.embedding_dimension as u64, Distance::Cosine),
            );
            vector_config.add_named_vector_params(
                "overview",
                VectorParamsBuilder::new(self.embedding_dimension as u64, Distance::Cosine),
            );
            vector_config.add_named_vector_params(
                "visual",
                VectorParamsBuilder::new(self.visual_embedding_dimension as u64, Distance::Cosine),
            );

            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.collection_name)
                        .vectors_config(vector_config),
                )
                .await
                .map_err(|e| TianyanError::VectorDatabase(format!("创建集合失败：{}", e)))?;

            tracing::info!("已创建 Qdrant 集合：{}", self.collection_name);
        }

        Ok(())
    }

    /// 插入或更新带有向量的点。
    pub async fn upsert_vectors(
        &self,
        id: &str,
        abstract_vector: Option<Vec<f32>>,
        overview_vector: Option<Vec<f32>>,
        visual_vector: Option<Vec<f32>>,
        payload: qdrant_client::Payload,
    ) -> Result<()> {
        use qdrant_client::qdrant::{NamedVectors, PointStruct, UpsertPointsBuilder};

        let mut named_vectors = NamedVectors::default();

        if let Some(v) = abstract_vector {
            named_vectors = named_vectors.add_vector("abstract", v);
        }
        if let Some(v) = overview_vector {
            named_vectors = named_vectors.add_vector("overview", v);
        }
        if let Some(v) = visual_vector {
            named_vectors = named_vectors.add_vector("visual", v);
        }

        let point = PointStruct::new(id, named_vectors, payload);

        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.collection_name, vec![point]).wait(true))
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("插入或更新点失败: {}", e)))?;

        Ok(())
    }

    /// 更新现有点的视觉嵌入。
    pub async fn update_visual_embedding(
        &self,
        uri: &TianyanUri,
        visual_embedding: &[f32],
    ) -> Result<()> {
        use qdrant_client::qdrant::{NamedVectors, PointVectors, UpdatePointVectorsBuilder};

        let id = uri.to_point_id();

        let named_vectors = NamedVectors::default().add_vector("visual", visual_embedding.to_vec());

        self.client
            .update_vectors(UpdatePointVectorsBuilder::new(
                &self.collection_name,
                vec![PointVectors {
                    id: Some(id.into()),
                    vectors: Some(named_vectors.into()),
                }],
            ))
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("更新视觉嵌入失败: {}", e)))?;

        Ok(())
    }

    /// 更新或创建指定 URI 的单个命名向量。
    ///
    /// 如果点不存在，会创建一个新点（仅包含该向量和基本负载）。
    pub async fn update_named_vector(
        &self,
        uri: &TianyanUri,
        vector_name: &str,
        vector: &[f32],
    ) -> Result<()> {
        let id = uri.to_point_id();

        let existing_point = self.get_point(&id).await?;

        if let Some(_point) = existing_point {
            use qdrant_client::qdrant::{NamedVectors, PointVectors, UpdatePointVectorsBuilder};

            let named_vectors = NamedVectors::default().add_vector(vector_name, vector.to_vec());

            self.client
                .update_vectors(UpdatePointVectorsBuilder::new(
                    &self.collection_name,
                    vec![PointVectors {
                        id: Some(id.into()),
                        vectors: Some(named_vectors.into()),
                    }],
                ))
                .await
                .map_err(|e| TianyanError::VectorDatabase(format!("更新向量失败：{}", e)))?;

            tracing::debug!("已更新向量 {} [{}]", uri, vector_name);
        } else {
            let payload = EntryMetadata::new(uri.clone(), "unknown");

            let abstract_vector = if vector_name == "abstract" {
                Some(vector.to_vec())
            } else {
                None
            };
            let overview_vector = if vector_name == "overview" {
                Some(vector.to_vec())
            } else {
                None
            };
            let visual_vector = if vector_name == "visual" {
                Some(vector.to_vec())
            } else {
                None
            };

            let point = VectorPoint {
                schema_version: CURRENT_SCHEMA_VERSION,
                id: id.clone(),
                abstract_vector,
                overview_vector,
                visual_vector,
                payload,
            };

            self.upsert_point(&point).await?;

            tracing::info!("已创建新向量 {} [{}]", uri, vector_name);
        }

        Ok(())
    }

    /// 将 VectorPoint 转换为 Qdrant 负载
    fn point_to_payload(&self, point: &VectorPoint) -> qdrant_client::Payload {
        let mut payload = point.payload.to_qdrant_payload();
        if !point.payload.custom.is_empty() {
            let custom_map: serde_json::Map<String, serde_json::Value> = point
                .payload
                .custom
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            payload.insert("custom".to_string(), serde_json::Value::Object(custom_map));
        }

        qdrant_client::Payload::try_from(serde_json::Value::Object(payload)).unwrap_or_default()
    }
}

#[async_trait]
impl VectorStorage for QdrantVectorStore {
    async fn initialize(&self) -> Result<()> {
        self.ensure_collection().await
    }

    async fn upsert_point(&self, point: &VectorPoint) -> Result<()> {
        let id = point.uri().to_point_id();
        let payload = self.point_to_payload(point);

        self.upsert_vectors(
            &id,
            point.abstract_vector.clone(),
            point.overview_vector.clone(),
            point.visual_vector.clone(),
            payload,
        )
        .await
    }

    async fn delete_point(&self, id: &str) -> Result<()> {
        use qdrant_client::qdrant::{DeletePointsBuilder, PointId};

        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection_name).points(vec![PointId::from(id)]),
            )
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("删除点失败：{}", e)))?;

        Ok(())
    }

    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>> {
        use qdrant_client::qdrant::{Condition, Filter, SearchPointsBuilder};

        let vector_name = match query.vector_type {
            VectorType::Abstract => "abstract",
            VectorType::Overview => "overview",
            VectorType::Visual => "visual",
        };

        let mut search_builder =
            SearchPointsBuilder::new(&self.collection_name, query.vector, query.limit as u64)
                .vector_name(vector_name)
                .with_payload(true);

        if let Some(ref cat) = query.category_filter {
            search_builder = search_builder.filter(Filter::must([Condition::matches(
                "namespace",
                cat.to_string(),
            )]));
        }

        if let Some(score) = query.min_score {
            search_builder = search_builder.score_threshold(score);
        }

        let results = self
            .client
            .search_points(search_builder)
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("搜索失败: {}", e)))?;

        Ok(results
            .result
            .into_iter()
            .map(|r| {
                let payload = EntryMetadata::from_qdrant_payload(&r.payload)
                    .unwrap_or_else(|_| EntryMetadata::default());
                VectorSearchResult {
                    id: format!("{:?}", r.id),
                    score: r.score,
                    payload,
                }
            })
            .collect())
    }

    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>> {
        use qdrant_client::qdrant::{GetPointsBuilder, PointId};

        let result = self
            .client
            .get_points(
                GetPointsBuilder::new(&self.collection_name, vec![PointId::from(id)])
                    .with_vectors(true)
                    .with_payload(true),
            )
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("获取点失败：{}", e)))?;

        let point = match result.result.into_iter().next() {
            Some(p) => p,
            None => return Ok(None),
        };
        let payload = EntryMetadata::from_qdrant_payload(&point.payload)
            .unwrap_or_else(|_| EntryMetadata::default());

        // 提取向量
        let (abstract_vector, overview_vector, visual_vector) = if let Some(vectors) = point.vectors
        {
            match vectors.vectors_options {
                Some(qdrant_client::qdrant::vectors_output::VectorsOptions::Vectors(
                    named_vectors,
                )) => {
                    let abstract_vector = named_vectors.vectors.get("abstract").and_then(|v| {
                        v.vector.as_ref().and_then(|vec| match vec {
                            qdrant_client::qdrant::vector_output::Vector::Dense(d) => {
                                Some(d.data.clone())
                            }
                            _ => None,
                        })
                    });
                    let overview_vector = named_vectors.vectors.get("overview").and_then(|v| {
                        v.vector.as_ref().and_then(|vec| match vec {
                            qdrant_client::qdrant::vector_output::Vector::Dense(d) => {
                                Some(d.data.clone())
                            }
                            _ => None,
                        })
                    });
                    let visual_vector = named_vectors.vectors.get("visual").and_then(|v| {
                        v.vector.as_ref().and_then(|vec| match vec {
                            qdrant_client::qdrant::vector_output::Vector::Dense(d) => {
                                Some(d.data.clone())
                            }
                            _ => None,
                        })
                    });
                    (abstract_vector, overview_vector, visual_vector)
                }
                _ => (None, None, None),
            }
        } else {
            (None, None, None)
        };

        Ok(Some(VectorPoint {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: id.to_string(),
            abstract_vector,
            overview_vector,
            visual_vector,
            payload,
        }))
    }

    async fn update_vector(
        &self,
        uri: &TianyanUri,
        vector_type: VectorType,
        vector: &[f32],
    ) -> Result<()> {
        let vector_name = match vector_type {
            VectorType::Abstract => "abstract",
            VectorType::Overview => "overview",
            VectorType::Visual => "visual",
        };

        self.update_named_vector(uri, vector_name, vector).await
    }

    async fn count_points(&self) -> Result<usize> {
        use qdrant_client::qdrant::CountPointsBuilder;

        let result = self
            .client
            .count(CountPointsBuilder::new(&self.collection_name).exact(true))
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("计数点失败：{}", e)))?;

        Ok(result.result.map(|r| r.count as usize).unwrap_or(0))
    }

    async fn clear(&self) -> Result<()> {
        self.client
            .delete_collection(&self.collection_name)
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("删除集合失败: {}", e)))?;

        self.ensure_collection().await
    }

    /// 使用 Qdrant 原生的融合搜索能力。
    async fn search_fused(
        &self,
        query_vector: Vec<f32>,
        vector_names: &[&str],
        top_k: usize,
        category_filter: Option<&str>,
        min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>> {
        use qdrant_client::qdrant::{
            Condition, Filter, Fusion, PrefetchQueryBuilder, Query, QueryPointsBuilder,
        };

        let mut prefetch_queries: Vec<qdrant_client::qdrant::PrefetchQuery> = Vec::new();

        for vector_name in vector_names {
            let prefetch = PrefetchQueryBuilder::default()
                .query(query_vector.clone())
                .using(vector_name.to_string())
                .limit((top_k * 2) as u64)
                .build();
            prefetch_queries.push(prefetch);
        }

        let mut query_builder = QueryPointsBuilder::new(&self.collection_name)
            .query(Query::new_fusion(Fusion::Rrf))
            .prefetch(prefetch_queries)
            .limit(top_k as u64)
            .with_payload(true);

        if let Some(cat) = category_filter {
            query_builder = query_builder.filter(Filter::must([Condition::matches(
                "namespace",
                cat.to_string(),
            )]));
        }

        if let Some(score) = min_score {
            query_builder = query_builder.score_threshold(score);
        }

        let results = self
            .client
            .query(query_builder)
            .await
            .map_err(|e| TianyanError::VectorDatabase(format!("融合搜索失败: {}", e)))?;

        Ok(results
            .result
            .into_iter()
            .map(|r| {
                let payload = EntryMetadata::from_qdrant_payload(&r.payload)
                    .unwrap_or_else(|_| EntryMetadata::default());
                VectorSearchResult {
                    id: format!("{:?}", r.id),
                    score: r.score,
                    payload,
                }
            })
            .collect())
    }

    /// 使用 Qdrant 原生的融合搜索能力。
    async fn search_abstract_and_overview(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        category_filter: Option<&str>,
    ) -> Result<Vec<VectorSearchResult>> {
        self.search_fused(
            query_vector,
            &["abstract", "overview"],
            top_k,
            category_filter,
            Some(L1_MIN_SCORE),
        )
        .await
    }
}

/// 用于创建 Qdrant 向量存储实例的构建器。
pub struct QdrantVectorStoreBuilder {
    config: Option<StorageConfig>,
    url: Option<String>,
    collection_name: Option<String>,
    embedding_dimension: Option<usize>,
    visual_embedding_dimension: Option<usize>,
}

impl QdrantVectorStoreBuilder {
    /// 创建新的构建器。
    pub fn new() -> Self {
        Self {
            config: None,
            url: None,
            collection_name: None,
            embedding_dimension: None,
            visual_embedding_dimension: None,
        }
    }

    /// 设置存储配置。
    pub fn with_config(mut self, config: StorageConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// 设置 Qdrant URL。
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// 设置集合名称。
    pub fn with_collection_name(mut self, name: impl Into<String>) -> Self {
        self.collection_name = Some(name.into());
        self
    }

    /// 设置嵌入维度。
    pub fn with_embedding_dimension(mut self, dim: usize) -> Self {
        self.embedding_dimension = Some(dim);
        self
    }

    /// 设置视觉嵌入维度。
    pub fn with_visual_embedding_dimension(mut self, dim: usize) -> Self {
        self.visual_embedding_dimension = Some(dim);
        self
    }

    /// 构建 Qdrant 向量存储。
    pub fn build(self) -> Result<QdrantVectorStore> {
        let mut config = self.config.unwrap_or_default();

        if let Some(collection_name) = self.collection_name {
            config.vector.collection_name = collection_name;
        }
        if let Some(embedding_dimension) = self.embedding_dimension {
            config.vector.vector_dimension = embedding_dimension;
        }

        if let Some(url) = self.url {
            QdrantVectorStore::with_url(&config, &url)
        } else {
            QdrantVectorStore::new(&config)
        }
    }
}

impl Default for QdrantVectorStoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_to_point_id() {
        let uri = TianyanUri::new(
            crate::common::types::ContextNamespace::User,
            vec!["profile".to_string(), "basic_info".to_string()],
        );
        let id = uri.to_point_id();

        // 验证 ID 是有效的标准 UUID 格式：36 字符，8-4-4-4-12
        assert_eq!(id.len(), 36);
        assert_eq!(
            id.chars().filter(|&c| c == '-').count(),
            4,
            "UUID 应该包含 4 个连字符"
        );

        // 验证格式：xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        assert!(parts
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_hexdigit())));

        // 验证相同 URI 产生相同 ID（确定性）
        let id2 = uri.to_point_id();
        assert_eq!(id, id2);

        // 验证不同 URI 产生不同 ID
        let uri2 = TianyanUri::new(
            crate::common::types::ContextNamespace::Memory,
            vec!["session1".to_string()],
        );
        let id3 = uri2.to_point_id();
        assert_ne!(id, id3);
    }

    #[test]
    fn test_builder() {
        let builder = QdrantVectorStoreBuilder::new()
            .with_url("http://localhost:6334")
            .with_collection_name("test_collection")
            .with_embedding_dimension(768)
            .with_visual_embedding_dimension(512);

        // 注意：这需要在运行 Qdrant 实例的情况下才能成功
        // 此测试用于验证构建器模式是否正常工作
        assert!(builder.url.is_some());
        assert!(builder.collection_name.is_some());
        assert!(builder.embedding_dimension.is_some());
        assert!(builder.visual_embedding_dimension.is_some());
    }
}
