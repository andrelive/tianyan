//! 向量存储后端 trait。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::TianyanUri;
use crate::vfs::types::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType};

/// 向量存储后端 trait。
///
/// 此 trait 定义了存储和搜索向量嵌入的向量存储后端接口。
#[async_trait]
pub trait VectorStorage: Send + Sync {
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

    /// 更新指定 URI 的单个命名向量。
    ///
    /// 如果点不存在，会创建一个新点。
    async fn update_vector(
        &self,
        uri: &TianyanUri,
        vector_type: VectorType,
        vector: &[f32],
    ) -> Result<()>;

    /// 获取向量点数量。
    async fn count_points(&self) -> Result<usize>;

    /// 清空所有向量点。
    async fn clear(&self) -> Result<()>;

    /// 使用多向量融合搜索。
    ///
    /// 使用相同的查询向量搜索多个命名向量，并通过 RRF 算法融合结果。
    /// 默认实现使用两次单独搜索然后手动融合。
    async fn search_fused(
        &self,
        _query_vector: Vec<f32>,
        _vector_types: &[VectorType],
        _top_k: usize,
        _category_filter: Option<&str>,
        _min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>> {
        Ok(vec![])
    }

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
