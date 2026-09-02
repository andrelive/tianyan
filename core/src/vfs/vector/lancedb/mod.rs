//! LanceDB 向量存储实现。
//! 嵌入式向量存储，进程内运行，数据持久化在 `{data_dir}/lancedb/` 目录。
//!
//! 按职责拆分：
//! - [`batch`]：RecordBatch ↔ VectorPoint 转换辅助（arrow 细节）
//! - 本文件：存储生命周期（建表、CRUD、检索、RRF 融合）

mod batch;

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
use arrow_array::cast::AsArray;
use arrow_array::{RecordBatch, RecordBatchIterator};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::query::ExecutableQuery;

use crate::common::error::{Result, TianyanError};
use crate::config::StorageConfig;
use crate::vfs::types::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType};
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
                // 维度自愈（ADR-025 后续）：已建表的 FixedSizeList 宽度 ≠ 当前配置时，
                // 嵌入/写入/检索将全部失败（历史教训：配置改过但表维度不迁移，
                // 三层维度错位导致检索全空 + 摘要任务全失败）。向量是可从 VFS
                // 摘要文本再生的派生数据，自动重建安全；空表由启动期回填补齐。
                let stale = match t.schema().await {
                    Ok(actual) => fixed_size_dims_differ(&actual, emb_dim, vis_dim),
                    Err(e) => {
                        tracing::warn!(error = %e, "LanceDB 表 schema 读取失败，跳过维度自愈检测");
                        false
                    }
                };
                if stale {
                    tracing::warn!(
                        configured_dim = emb_dim,
                        "LanceDB 表维度与配置不一致，自动重建（存量向量将由启动期回填重嵌入）"
                    );
                    db.drop_table(table_name, &[])
                        .await
                        .map_err(|e| {
                            TianyanError::Custom(format!("向量数据库错误：重建表删除失败: {e}"))
                        })?;
                    let empty = RecordBatch::new_empty(schema.clone());
                    db.create_table(table_name, empty)
                        .execute()
                        .await
                        .map_err(|e| {
                            TianyanError::Custom(format!("向量数据库错误：重建表失败: {e}"))
                        })?
                } else {
                    tracing::info!("LanceDB 表已打开: {}", table_name);
                    t
                }
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

    /// 将上下文条目转换为 RecordBatch（单行）。
    fn point_to_batch(&self, point: &VectorPoint) -> Result<RecordBatch> {
        // 维度前置校验（防御性）：向量长度 ≠ 建表维度时，arrow FixedSizeListBuilder
        // 会在 finish/访问时 panic——release panic=abort 下整进程闪退且日志无输出。
        // 此处提前转为带行动指引的干净错误。
        let check = |name: &str, v: &Option<Vec<f32>>, dim: usize| -> Result<()> {
            if let Some(v) = v {
                if v.len() != dim {
                    return Err(TianyanError::Custom(format!(
                        "向量数据库错误：{} 维度不匹配：实际 {}，向量库 {}——请使 storage.vector.vector_dimension 与嵌入模型输出维度一致（嵌入请求已按向量库维度发起；检查嵌入模型配置）",
                        name, v.len(), dim
                    )));
                }
            }
            Ok(())
        };
        check(
            "abstract_vector",
            &point.abstract_vector,
            self.embedding_dim,
        )?;
        check(
            "overview_vector",
            &point.overview_vector,
            self.embedding_dim,
        )?;
        check("visual_vector", &point.visual_vector, self.visual_dim)?;

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
        batch::append_vector(&mut abs_b, &point.abstract_vector);
        batch::append_vector(&mut ov_b, &point.overview_vector);
        batch::append_vector(&mut vis_b, &point.visual_vector);
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
}

// ─── VectorStorage trait impl ───

#[async_trait]
impl VectorStorage for LanceDbVectorStore {
    fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn count_rows(&self) -> Result<u64> {
        self.table
            .count_rows(None)
            .await
            .map(|n| n as u64)
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：行数读取失败: {e}")))
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
        // 查询向量维度校验（同 upsert：不一致时 arrow/lance 内部 panic 不可控）
        if query.vector.len() != self.embedding_dim {
            return Err(TianyanError::Custom(format!(
                "向量数据库错误：查询向量维度不匹配：实际 {}，向量库 {}——请使 storage.vector.vector_dimension 与嵌入模型输出维度一致",
                query.vector.len(),
                self.embedding_dim
            )));
        }
        let stream = self
            .table
            .query()
            .nearest_to(query.vector)
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：搜索失败: {e}")))?
            .column(batch::vector_column_name(query.vector_type))
            .execute()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：执行失败: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| TianyanError::Custom(format!("向量数据库错误：收集失败: {e}")))?;

        let mut all = Vec::new();
        for batch in &batches {
            let mut r = batch::batch_to_results(batch)?;
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
                        return Ok(batch::batch_to_point(batch, i));
                    }
                }
            }
        }
        Ok(None)
    }

    async fn search_fused(
        &self,
        query_vector: Vec<f32>,
        vector_types: &[VectorType],
        top_k: usize,
        category_filter: Option<&str>,
        min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>> {
        /// RRF 常数 k，用于平滑排序差异（标准值为 60）。
        const RRF_K: f32 = 60.0;

        if vector_types.is_empty() {
            return Ok(vec![]);
        }

        // 对每个向量列独立搜索，收集带排名的结果。
        // point_id → (fused_rrf_score, best_similarity, best_result)
        // RRF 分数（1/(rank+k)）仅用于跨列排序；暴露给调用方的 score 取各列中
        // 最高的相似度（1.0 - distance，与单列 search() 同尺度），
        // 供 ContentLoadStrategy 的绝对阈值（0.6/0.85）使用。
        let mut merged: HashMap<String, (f32, f32, VectorSearchResult)> = HashMap::new();

        for &vec_type in vector_types {
            let column = batch::vector_column_name(vec_type);

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
                let results = batch::batch_to_results(batch)?;
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

/// 表内 FixedSizeList 列宽与配置维度是否不一致（小写辅助：任一缺失或不等 = 重建）。
fn fixed_size_dims_differ(schema: &Schema, expected_emb: usize, expected_vis: usize) -> bool {
    let dim_of = |field: &str| -> Option<usize> {
        schema
            .field_with_name(field)
            .ok()
            .and_then(|f| match f.data_type() {
                DataType::FixedSizeList(_, w) => Some(*w as usize),
                _ => None,
            })
    };
    let (abs, ov, vis) = (
        dim_of("abstract_vec"),
        dim_of("overview_vec"),
        dim_of("visual_vec"),
    );
    !matches!(
        (abs, ov, vis),
        (Some(a), Some(o), Some(v))
            if a == expected_emb && o == expected_emb && v == expected_vis
    )
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
