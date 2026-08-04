//! LanceDB 向量存储实现。
//! 嵌入式向量存储，进程内运行，数据持久化在 `{data_dir}/lancedb/` 目录。

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
use arrow_array::cast::AsArray;
use arrow_array::types::Float32Type;
use arrow_array::{Array, RecordBatch, RecordBatchIterator};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::query::ExecutableQuery;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{EntryMetadata, TianyanUri};
use crate::config::StorageConfig;
use crate::vfs::types::{
    VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType, CURRENT_SCHEMA_VERSION,
};
use crate::vfs::vector::VectorStorage;

/// LanceDB 向量存储实现。
///
/// 使用嵌入式 LanceDB 作为向量数据库后端，支持多向量列（Abstract/Overview/Visual）
/// 的高效近似最近邻搜索与 RRF 融合检索。数据持久化在 `{data_dir}/lancedb/` 目录。
pub struct LanceDbVectorStore {
    table: lancedb::Table,
    schema: Arc<Schema>,
    embedding_dim: usize,
    visual_dim: usize,
}

impl LanceDbVectorStore {
    /// 创建并初始化 LanceDB 向量存储实例。
    ///
    /// 连接或创建指定路径下的 LanceDB 数据库，打开或创建配置中指定的集合表。
    /// 表结构包含 id、三层向量列（abstract_vec/overview_vec/visual_vec）
    /// 以及 namespace、uri、custom_json、importance 等元数据列。
    ///
    /// # 参数
    /// * `config` - 存储配置，从中读取数据目录、向量维度、集合名称等参数。
    pub async fn new(config: &StorageConfig) -> Result<Self> {
        let db_path = config.data_dir.join("lancedb");
        std::fs::create_dir_all(&db_path)
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：创建目录失败: {e}")))?;

        let db_path_str = db_path
            .to_str()
            .ok_or_else(|| TianyanError::Custom("向量数据库错误：路径非 UTF-8".to_string()))?;

        let db = lancedb::connect(db_path_str)
            .execute()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：连接 LanceDB 失败: {e}")))?;

        let emb_dim = config.vector.vector_dimension;
        let vis_dim = config.vector.vector_dimension / 3;
        let table_name = &config.vector.collection_name;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "abstract_vec",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    emb_dim as i32,
                ),
                true,
            ),
            Field::new(
                "overview_vec",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    emb_dim as i32,
                ),
                true,
            ),
            Field::new(
                "visual_vec",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    vis_dim as i32,
                ),
                true,
            ),
            Field::new("namespace", DataType::Utf8, true),
            Field::new("uri", DataType::Utf8, true),
            Field::new("custom_json", DataType::Utf8, true),
            Field::new("importance", DataType::Float32, true),
        ]));

        let table = match db.open_table(table_name).execute().await {
            Ok(t) => {
                tracing::info!("LanceDB 表已打开: {}", table_name);
                t
            }
            Err(_) => {
                let empty = RecordBatch::new_empty(schema.clone());
                let tbl = db
                    .create_table(table_name, empty)
                    .execute()
                    .await
                    .map_err(|e| {
                        TianyanError::Custom(format!("向量数据库错误：创建表失败: {e}"))
                    })?;
                tracing::info!("LanceDB 表已创建: {}", table_name);
                tbl
            }
        };

        Ok(Self {
            table,
            schema,
            embedding_dim: emb_dim,
            visual_dim: vis_dim,
        })
    }

    // ─── RecordBatch ↔ VectorPoint ───

    fn point_to_batch(&self, point: &VectorPoint) -> Result<RecordBatch> {
        let id = point.uri().to_point_id();
        let mut id_b = StringBuilder::new();
        let mut abs_b = FixedSizeListBuilder::new(Float32Builder::new(), self.embedding_dim as i32);
        let mut ov_b = FixedSizeListBuilder::new(Float32Builder::new(), self.embedding_dim as i32);
        let mut vis_b = FixedSizeListBuilder::new(Float32Builder::new(), self.visual_dim as i32);
        let mut ns_b = StringBuilder::new();
        let mut uri_b = StringBuilder::new();
        let mut cust_b = StringBuilder::new();
        let mut imp_b = Float32Builder::new();

        id_b.append_value(&id);
        Self::append_vector(&mut abs_b, &point.abstract_vector);
        Self::append_vector(&mut ov_b, &point.overview_vector);
        Self::append_vector(&mut vis_b, &point.visual_vector);
        ns_b.append_value(point.uri().namespace().to_string());
        uri_b.append_value(point.uri().as_str());
        cust_b.append_value(
            serde_json::to_string(&point.payload.custom).unwrap_or_else(|_| "{}".to_string()),
        );
        imp_b.append_value(point.payload.importance);

        RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(id_b.finish()),
                Arc::new(abs_b.finish()),
                Arc::new(ov_b.finish()),
                Arc::new(vis_b.finish()),
                Arc::new(ns_b.finish()),
                Arc::new(uri_b.finish()),
                Arc::new(cust_b.finish()),
                Arc::new(imp_b.finish()),
            ],
        )
        .map_err(|e| TianyanError::Custom(format!("向量数据库错误：构建 RecordBatch 失败: {e}")))
    }

    fn append_vector(builder: &mut FixedSizeListBuilder<Float32Builder>, vec: &Option<Vec<f32>>) {
        match vec {
            Some(v) => {
                for &x in v {
                    builder.values().append_value(x);
                }
                builder.append(true);
            }
            None => {
                // Arrow FixedSizeListBuilder::finish() 要求 values.len() == value_length * len()。
                // 即使 null 条目也需要填充 value_length 个占位 null 值。
                let dim = builder.value_length() as usize;
                for _ in 0..dim {
                    builder.values().append_null();
                }
                builder.append(false);
            }
        }
    }

    fn extract_vector(col: &dyn Array, row: usize) -> Option<Vec<f32>> {
        if col.is_null(row) {
            return None;
        }
        Some(
            col.as_fixed_size_list()
                .value(row)
                .as_primitive::<Float32Type>()
                .values()
                .to_vec(),
        )
    }

    /// 从 RecordBatch 的行中提取基础字段并构造 EntryMetadata。
    /// 返回 `(id, payload)`，当 id 或 uri 列为空时返回 `None`。
    fn extract_base_fields(batch: &RecordBatch, row: usize) -> Option<(String, EntryMetadata)> {
        let id = batch
            .column_by_name("id")?
            .as_string::<i32>()
            .value(row)
            .to_string();
        let uri_s = batch
            .column_by_name("uri")?
            .as_string::<i32>()
            .value(row)
            .to_string();
        let ns = batch
            .column_by_name("namespace")
            .map(|c| c.as_string::<i32>().value(row).to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let importance = batch
            .column_by_name("importance")
            .map(|c| c.as_primitive::<Float32Type>().value(row))
            .unwrap_or(0.5);
        let custom: HashMap<String, serde_json::Value> = batch
            .column_by_name("custom_json")
            .and_then(|c| serde_json::from_str(c.as_string::<i32>().value(row)).ok())
            .unwrap_or_default();

        let parsed_uri = TianyanUri::parse(&uri_s).unwrap_or_else(|_| {
            TianyanUri::new(
                crate::common::types::ContextNamespace::User,
                vec![id.clone()],
            )
        });
        let mut payload = EntryMetadata::new(parsed_uri, &ns);
        payload.importance = importance;
        payload.custom = custom;

        Some((id, payload))
    }

    fn batch_to_point(batch: &RecordBatch, row: usize) -> Option<VectorPoint> {
        let (id, payload) = Self::extract_base_fields(batch, row)?;
        Some(VectorPoint {
            schema_version: CURRENT_SCHEMA_VERSION,
            id,
            abstract_vector: Self::extract_vector(batch.column_by_name("abstract_vec")?, row),
            overview_vector: Self::extract_vector(batch.column_by_name("overview_vec")?, row),
            visual_vector: Self::extract_vector(batch.column_by_name("visual_vec")?, row),
            payload,
        })
    }

    /// 将 VectorType 映射到 LanceDB 中的列名。
    fn vector_column_name(vec_type: VectorType) -> &'static str {
        match vec_type {
            VectorType::Abstract => "abstract_vec",
            VectorType::Overview => "overview_vec",
            VectorType::Visual => "visual_vec",
        }
    }

    fn batch_to_results(batch: &RecordBatch) -> Result<Vec<VectorSearchResult>> {
        let score_data = batch
            .column_by_name("_distance")
            .map(|c| c.as_primitive::<Float32Type>());

        let mut results = Vec::with_capacity(batch.num_rows());
        for i in 0..batch.num_rows() {
            let (id, payload) = Self::extract_base_fields(batch, i).ok_or_else(|| {
                TianyanError::Custom("向量数据库错误：缺少 id 或 uri 列".to_string())
            })?;
            let score = score_data.as_ref().map(|c| 1.0 - c.value(i)).unwrap_or(0.0);
            results.push(VectorSearchResult { id, score, payload });
        }
        Ok(results)
    }
}

// ─── VectorStorage trait impl ───

#[async_trait]
impl VectorStorage for LanceDbVectorStore {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn upsert_point(&self, point: &VectorPoint) -> Result<()> {
        let batch = self.point_to_batch(point)?;
        let schema = batch.schema();
        let reader = RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema);
        let mut merge = self.table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(reader))
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：upsert 失败: {e}")))?;
        Ok(())
    }

    async fn delete_point(&self, id: &str) -> Result<()> {
        self.table
            .delete(&format!("id = '{}'", id))
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：删除失败: {e}")))?;
        Ok(())
    }

    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>> {
        let stream = self
            .table
            .query()
            .nearest_to(query.vector)
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：搜索失败: {e}")))?
            .column(Self::vector_column_name(query.vector_type))
            .execute()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：执行失败: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：收集失败: {e}")))?;

        let mut all = Vec::new();
        for batch in &batches {
            let mut r = Self::batch_to_results(batch)?;
            // 应用层过滤
            if let Some(ref cat) = query.category_filter {
                r.retain(|res| res.payload.uri.namespace().to_string() == *cat);
            }
            if let Some(ms) = query.min_score {
                r.retain(|res| res.score >= ms);
            }
            r.truncate(query.limit);
            all.append(&mut r);
        }
        Ok(all)
    }

    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>> {
        // 扫描全表找匹配 ID（适合桌面量级 <10K 条目）。
        // 注：LanceDB 0.31 的 Query builder 不支持 .filter()，升级到 0.40+ 后可迁移。
        let dummy = vec![0.0f32; self.embedding_dim];
        let stream = self
            .table
            .query()
            .nearest_to(dummy)
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：查询失败: {e}")))?
            .column("abstract_vec")
            .execute()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：执行失败: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：收集失败: {e}")))?;

        for batch in &batches {
            if let Some(col) = batch.column_by_name("id") {
                for i in 0..batch.num_rows() {
                    if col.as_string::<i32>().value(i) == id {
                        return Ok(Self::batch_to_point(batch, i));
                    }
                }
            }
        }
        Ok(None)
    }

    async fn update_vector(
        &self,
        uri: &TianyanUri,
        vector_type: VectorType,
        vector: &[f32],
    ) -> Result<()> {
        let id = uri.to_point_id();
        let existing = self.get_point(&id).await?;
        let mut point = existing.unwrap_or_else(|| VectorPoint {
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
        self.upsert_point(&point).await
    }

    async fn count_points(&self) -> Result<usize> {
        self.table
            .count_rows(None)
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：计数失败: {e}")))
    }

    async fn clear(&self) -> Result<()> {
        self.table
            .delete("true")
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：清空失败: {e}")))?;
        Ok(())
    }

    async fn search_fused(
        &self,
        query_vector: Vec<f32>,
        vector_names: &[&str],
        top_k: usize,
        category_filter: Option<&str>,
        min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>> {
        /// RRF 常数 k，用于平滑排序差异（标准值为 60）。
        const RRF_K: f32 = 60.0;

        if vector_names.is_empty() {
            return Ok(vec![]);
        }

        // 对每个向量列独立搜索，收集带排名的结果。
        // point_id → (fused_rrf_score, best_similarity, best_result)
        // RRF 分数（1/(rank+k)）仅用于跨列排序；暴露给调用方的 score 取各列中
        // 最高的相似度（1.0 - distance，与单列 search() 同尺度），
        // 供 ContentLoadStrategy 的绝对阈值（0.6/0.85）使用。
        let mut merged: HashMap<String, (f32, f32, VectorSearchResult)> = HashMap::new();

        for &vec_name in vector_names {
            let column = Self::vector_column_name(match vec_name {
                "abstract" => VectorType::Abstract,
                "overview" => VectorType::Overview,
                "visual" => VectorType::Visual,
                _ => continue,
            });

            let stream = match self
                .table
                .query()
                .nearest_to(query_vector.clone())
                .map_err(|e| TianyanError::Custom(format!("向量数据库错误：RRF 搜索失败: {e}")))?
                .column(column)
                .execute()
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, column, "RRF 单列搜索失败，跳过");
                    continue;
                }
            };

            let batches: Vec<RecordBatch> = match stream.try_collect().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, column, "RRF 收集结果失败，跳过");
                    continue;
                }
            };

            let mut rank: usize = 0;
            for batch in &batches {
                let results = Self::batch_to_results(batch)?;
                for res in results {
                    rank += 1;
                    let rrf = 1.0 / (rank as f32 + RRF_K);
                    let point_id = res.id.clone();

                    merged
                        .entry(point_id)
                        .and_modify(|(rrf_acc, best_sim, _)| {
                            *rrf_acc += rrf;
                            *best_sim = best_sim.max(res.score);
                        })
                        .or_insert_with(|| (rrf, res.score, res));
                }
            }
        }

        // 按 RRF 融合分数排序（排序依据），暴露的 score 为最高相似度（加载深度依据）。
        let mut fused: Vec<(f32, VectorSearchResult)> = merged
            .into_values()
            .map(|(rrf_score, best_sim, mut res)| {
                res.score = best_sim;
                (rrf_score, res)
            })
            .collect();

        fused.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut fused: Vec<VectorSearchResult> = fused.into_iter().map(|(_, res)| res).collect();

        if let Some(cat) = category_filter {
            fused.retain(|res| res.payload.uri.namespace().to_string() == cat);
        }
        if let Some(ms) = min_score {
            fused.retain(|res| res.score >= ms);
        }

        fused.truncate(top_k);
        Ok(fused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;
    use crate::config::StorageConfig;
    use tempfile::tempdir;

    /// 创建一个测试用的向量（维度 8）。
    fn test_vec() -> Vec<f32> {
        vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    }

    /// 创建一个不同的测试向量。
    fn test_vec2() -> Vec<f32> {
        vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    }

    /// 辅助函数：创建测试 VectorPoint。
    fn make_point(uri: &str, abs_vec: Option<Vec<f32>>, ov_vec: Option<Vec<f32>>) -> VectorPoint {
        let parsed_uri = TianyanUri::parse(uri)
            .unwrap_or_else(|_| TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]));
        VectorPoint {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: uri.replace("tianyan://", "").replace('/', "_"),
            abstract_vector: abs_vec,
            overview_vector: ov_vec,
            visual_vector: None,
            payload: EntryMetadata::new(parsed_uri, "test"),
        }
    }

    /// 辅助函数：创建隔离的 LanceDbVectorStore + TempDir。
    async fn create_store() -> (LanceDbVectorStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let mut config = StorageConfig {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        config.vector.vector_dimension = 8;
        config.vector.collection_name = "test_lancedb".to_string();
        let store = LanceDbVectorStore::new(&config).await.unwrap();
        (store, dir)
    }

    // ─── 基础创建与初始化 ───

    #[tokio::test]
    async fn test_new_store() {
        let (store, _dir) = create_store().await;
        assert_eq!(store.count_points().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_initialize() {
        let (store, _dir) = create_store().await;
        // initialize() is a no-op for LanceDB but should succeed
        store.initialize().await.unwrap();
    }

    // ─── 插入与获取 ───

    #[tokio::test]
    async fn test_upsert_and_get_point() {
        let (store, _dir) = create_store().await;
        let point = make_point("tianyan://knowledge/test_doc", Some(test_vec()), None);
        store.upsert_point(&point).await.unwrap();

        let id = point.uri().to_point_id();
        let retrieved = store.get_point(&id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.abstract_vector, point.abstract_vector);
        assert_eq!(retrieved.overview_vector, point.overview_vector);
        assert_eq!(retrieved.visual_vector, point.visual_vector);
        // id 字段在存储时由 to_point_id() 重新生成
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_upsert_updates_existing() {
        let (store, _dir) = create_store().await;
        let uri = "tianyan://memory/event_1";
        let p1 = make_point(uri, Some(test_vec()), None);
        store.upsert_point(&p1).await.unwrap();

        // 用不同的向量 upsert 同一个点
        let p2 = make_point(uri, Some(test_vec2()), None);
        store.upsert_point(&p2).await.unwrap();

        let id = p2.uri().to_point_id();
        let retrieved = store.get_point(&id).await.unwrap().unwrap();
        // 应该拿到更新的向量
        assert_eq!(retrieved.abstract_vector, Some(test_vec2()));
        assert_eq!(retrieved.overview_vector, None);
    }

    // ─── 获取不存在 ───

    #[tokio::test]
    async fn test_get_point_nonexistent() {
        let (store, _dir) = create_store().await;
        let result = store.get_point("nonexistent-id-12345").await.unwrap();
        assert!(result.is_none());
    }

    // ─── 搜索 ───

    #[tokio::test]
    async fn test_search_empty() {
        let (store, _dir) = create_store().await;
        // 空表搜索：nearest_to 可能返回错误或空结果
        let query = VectorSearchQuery {
            vector: test_vec(),
            vector_type: VectorType::Abstract,
            limit: 10,
            category_filter: None,
            min_score: None,
        };
        match store.search(query).await {
            Ok(results) => assert!(results.is_empty()),
            Err(_) => {
                // LanceDB empty-table search 可能失败，接受两种行为
            }
        }
    }

    #[tokio::test]
    async fn test_search_with_data() {
        let (store, _dir) = create_store().await;
        let p1 = make_point("tianyan://knowledge/doc_a", Some(test_vec()), None);
        let p2 = make_point("tianyan://knowledge/doc_b", Some(test_vec2()), None);
        store.upsert_point(&p1).await.unwrap();
        store.upsert_point(&p2).await.unwrap();

        let query = VectorSearchQuery {
            vector: test_vec(),
            vector_type: VectorType::Abstract,
            limit: 10,
            category_filter: None,
            min_score: None,
        };
        let results = store.search(query).await.unwrap();
        assert!(!results.is_empty());
        // 第一个结果应该与 test_vec 相似（doc_a）
        assert_eq!(results[0].id, p1.uri().to_point_id());
        // 分数应接近 1.0（完全相同）
        assert!((results[0].score - 1.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_search_with_category_filter() {
        let (store, _dir) = create_store().await;
        // 插入 knowledge 和 memory 两个命名空间
        let p_know = make_point("tianyan://knowledge/ref1", Some(test_vec()), None);
        let p_mem = make_point("tianyan://memory/ref1", Some(test_vec()), None);
        store.upsert_point(&p_know).await.unwrap();
        store.upsert_point(&p_mem).await.unwrap();

        let query = VectorSearchQuery {
            vector: test_vec(),
            vector_type: VectorType::Abstract,
            limit: 10,
            category_filter: Some("knowledge".to_string()),
            min_score: None,
        };
        let results = store.search(query).await.unwrap();
        assert!(!results.is_empty());
        for r in &results {
            assert_eq!(r.payload.uri.namespace().to_string(), "knowledge");
        }
    }

    #[tokio::test]
    async fn test_search_with_min_score() {
        let (store, _dir) = create_store().await;
        let point = make_point("tianyan://knowledge/only_one", Some(test_vec()), None);
        store.upsert_point(&point).await.unwrap();

        // 使用远超可能范围的 min_score
        let query = VectorSearchQuery {
            vector: test_vec(),
            vector_type: VectorType::Abstract,
            limit: 10,
            category_filter: None,
            min_score: Some(10.0),
        };
        let results = store.search(query).await.unwrap();
        assert!(results.is_empty());
    }

    // ─── 删除 ───

    #[tokio::test]
    async fn test_delete_point() {
        let (store, _dir) = create_store().await;
        let point = make_point("tianyan://knowledge/to_delete", Some(test_vec()), None);
        store.upsert_point(&point).await.unwrap();
        let id = point.uri().to_point_id();

        // 确认存在
        assert!(store.get_point(&id).await.unwrap().is_some());

        // 删除
        store.delete_point(&id).await.unwrap();
        assert!(store.get_point(&id).await.unwrap().is_none());
        assert_eq!(store.count_points().await.unwrap(), 0);
    }

    // ─── 计数 ───

    #[tokio::test]
    async fn test_count_points() {
        let (store, _dir) = create_store().await;
        assert_eq!(store.count_points().await.unwrap(), 0);

        let p1 = make_point("tianyan://knowledge/a", Some(test_vec()), None);
        let p2 = make_point("tianyan://memory/b", Some(test_vec2()), None);
        store.upsert_point(&p1).await.unwrap();
        store.upsert_point(&p2).await.unwrap();
        assert_eq!(store.count_points().await.unwrap(), 2);

        store.delete_point(&p1.uri().to_point_id()).await.unwrap();
        assert_eq!(store.count_points().await.unwrap(), 1);
    }

    // ─── 清空 ───

    #[tokio::test]
    async fn test_clear() {
        let (store, _dir) = create_store().await;
        let p1 = make_point("tianyan://knowledge/x", Some(test_vec()), None);
        let p2 = make_point("tianyan://memory/y", Some(test_vec2()), None);
        store.upsert_point(&p1).await.unwrap();
        store.upsert_point(&p2).await.unwrap();
        assert_eq!(store.count_points().await.unwrap(), 2);

        store.clear().await.unwrap();
        assert_eq!(store.count_points().await.unwrap(), 0);
    }

    // ─── 更新向量 ───

    #[tokio::test]
    async fn test_update_vector() {
        let (store, _dir) = create_store().await;
        let uri = TianyanUri::parse("tianyan://knowledge/updatable").unwrap();
        let point = make_point("tianyan://knowledge/updatable", None, None);
        store.upsert_point(&point).await.unwrap();

        // 更新 abstract vector
        store
            .update_vector(&uri, VectorType::Abstract, &test_vec())
            .await
            .unwrap();

        let id = uri.to_point_id();
        let retrieved = store.get_point(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.abstract_vector, Some(test_vec()));
        assert!(retrieved.overview_vector.is_none());
        assert!(retrieved.visual_vector.is_none());

        // 更新 overview vector
        store
            .update_vector(&uri, VectorType::Overview, &test_vec2())
            .await
            .unwrap();
        let retrieved = store.get_point(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.abstract_vector, Some(test_vec()));
        assert_eq!(retrieved.overview_vector, Some(test_vec2()));
    }

    // ─── search_abstract_and_overview（默认 trait 方法） ───

    #[tokio::test]
    async fn test_search_abstract_and_overview() {
        let (store, _dir) = create_store().await;
        let p1 = make_point("tianyan://knowledge/a", Some(test_vec()), None);
        store.upsert_point(&p1).await.unwrap();

        // 通过 trait 调用默认方法
        use crate::vfs::vector::VectorStorage;
        let results = store
            .search_abstract_and_overview(test_vec(), 10, None)
            .await
            .unwrap();

        // RRF 融合仅决定排序；score 保持相似度尺度（1.0 - distance），
        // 与单列 search() 一致，供 ContentLoadStrategy 绝对阈值使用。
        assert!(!results.is_empty());
        assert_eq!(results[0].id, p1.uri().to_point_id());
        assert!(
            results[0].score > 0.5,
            "融合搜索分数应为相似度尺度（≈1.0），实际: {}",
            results[0].score
        );
    }
}
