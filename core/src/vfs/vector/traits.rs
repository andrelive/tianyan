//! 向量存储后端 trait。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::vfs::types::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType};

/// 向量存储后端 trait。
///
/// 此 trait 定义了存储和搜索向量嵌入的向量存储后端接口。
#[async_trait]
pub trait VectorStorage: Send + Sync {
    /// 向量存储的嵌入维度（建表 schema 的 FixedSizeList 宽度）。
    ///
    /// 嵌入路径据此向 API 请求同维度向量，保证写入/查询与表 schema 一致。
    /// 维度不一致曾触发 arrow 内部 panic + release `panic = "abort"` 整进程闪退
    /// （0xC0000409 fail-fast，日志无任何输出）。
    fn embedding_dim(&self) -> usize;

    /// 初始化向量存储。
    async fn initialize(&self) -> Result<()>;

    /// 插入或更新向量点。
    async fn upsert_point(&self, point: &VectorPoint) -> Result<()>;

    /// 按 ID 删除向量点。
    async fn delete_point(&self, id: &str) -> Result<()>;

    /// 搜索相似向量。
    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>>;

    /// 按 ID 获取向量点。
    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>>;

    /// 使用多向量融合搜索。
    ///
    /// 使用相同的查询向量搜索多个命名向量，并通过 RRF 算法融合结果。
    async fn search_fused(
        &self,
        query_vector: Vec<f32>,
        vector_types: &[VectorType],
        top_k: usize,
        category_filter: Option<&str>,
        min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>>;

    /// 使用摘要和概览向量进行融合搜索。
    ///
    /// RRF 融合决定跨列排序；返回结果的 `score` 为各列中最高的相似度
    /// （1.0 - distance，与单列 `search()` 同尺度），供上层绝对阈值
    /// （如 `ContentLoadStrategy` 的 0.6/0.85）判断加载深度。
    async fn search_abstract_and_overview(
        &self,
        query_vector: Vec<f32>,
        top_k: usize,
        category_filter: Option<&str>,
    ) -> Result<Vec<VectorSearchResult>> {
        self.search_fused(
            query_vector,
            &[VectorType::Abstract, VectorType::Overview],
            top_k,
            category_filter,
            None,
        )
        .await
    }
}
