//! 存储后端和虚拟文件系统 trait。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, SearchResult, TianyanUri};

use super::types::{
    ContextEntry, DirectoryIndex, StorageStats, VectorPoint, VectorSearchQuery, VectorSearchResult,
    VectorType,
};

/// 内容元数据。
#[derive(Debug, Clone)]
pub struct ContentMetadata {
    /// 内容大小（字节）
    pub size: u64,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 更新时间
    pub updated_at: DateTime<Utc>,
}

/// 存储后端 trait。
///
/// 此 trait 定义了持久化上下文条目及其元数据的存储后端接口。
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// 初始化存储后端。
    async fn initialize(&self) -> Result<()>;

    /// 检查给定 URI 处是否存在条目。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;

    /// 从存储读取条目。
    async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 将条目写入存储。
    async fn write_entry(&self, entry: &ContextEntry) -> Result<()>;

    /// 从存储删除条目。
    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出目录中的条目。
    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;

    /// 读取特定层级的内容。
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;

    /// 写入特定层级的内容。
    async fn write_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()>;

    /// 追加内容到特定层级的文件末尾。
    ///
    /// 此方法用于需要持续追加的场景（如会话消息记录）。
    /// 如果底层存储支持真正的追加写入，实现应该使用它；
    /// 否则使用默认实现（读取 + 合并 + 写入）。
    ///
    /// # 参数
    /// - `uri`: 条目 URI
    /// - `level`: 内容层级
    /// - `content`: 要追加的内容
    ///
    /// # 返回
    /// - `Ok(())`: 追加成功
    /// - `Err`: 追加失败
    async fn append_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()> {
        // 默认实现：读取 + 合并 + 写入
        let existing = self.read_content(uri, level).await.unwrap_or_default();
        let combined = format!("{}{}", existing, content);
        self.write_content(uri, level, &combined).await
    }

    /// 获取目录索引。
    async fn get_directory_index(&self, uri: &TianyanUri) -> Result<DirectoryIndex>;

    /// 更新目录索引。
    async fn update_directory_index(&self, uri: &TianyanUri, index: &DirectoryIndex) -> Result<()>;

    /// 获取存储统计信息。
    async fn get_stats(&self) -> Result<StorageStats>;

    /// 获取存储根路径。
    fn root_path(&self) -> &Path;

    /// 将 URI 转换为文件系统路径。
    fn uri_to_path(&self, uri: &TianyanUri) -> std::path::PathBuf;
}

/// 向量存储后端 trait。
///
/// 此 trait 定义了存储和搜索向量嵌入的向量存储后端接口。
#[async_trait]
pub trait VectorStorage: Send + Sync {
    /// 初始化向量存储。
    async fn initialize(&self) -> Result<()>;

    /// 插入或更新向量点。
    async fn upsert_point(&self, point: &VectorPoint) -> Result<()>;

    /// 批量插入或更新向量点。
    async fn upsert_points(&self, points: &[VectorPoint]) -> Result<()> {
        for point in points {
            self.upsert_point(point).await?;
        }
        Ok(())
    }

    /// 按 ID 删除向量点。
    async fn delete_point(&self, id: &str) -> Result<()>;

    /// 批量删除向量点。
    async fn delete_points(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.delete_point(id).await?;
        }
        Ok(())
    }

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
    ///
    /// # 参数
    /// - `query_vector`: 查询向量
    /// - `vector_names`: 要搜索的命名向量列表
    /// - `top_k`: 返回结果数量
    /// - `category_filter`: 可选的类别过滤器
    /// - `min_score`: 可选的最低分数阈值
    async fn search_fused(
        &self,
        query_vector: Vec<f32>,
        vector_names: &[&str],
        top_k: usize,
        category_filter: Option<&str>,
        min_score: Option<f32>,
    ) -> Result<Vec<VectorSearchResult>> {
        // 默认实现：对每个向量类型分别搜索，然后手动融合
        let mut all_results: Vec<(String, VectorSearchResult)> = Vec::new();

        for vector_name in vector_names {
            let vector_type = match *vector_name {
                "abstract" => VectorType::Abstract,
                "overview" => VectorType::Overview,
                "visual" => VectorType::Visual,
                _ => continue,
            };

            let query = VectorSearchQuery {
                vector: query_vector.clone(),
                vector_type,
                limit: top_k * 2,
                category_filter: category_filter.map(|s| s.to_string()),
                min_score,
            };

            let results = self.search(query).await?;
            for result in results {
                all_results.push((vector_name.to_string(), result));
            }
        }

        // 使用 RRF 算法融合结果
        Ok(fuse_results_rrf(all_results, top_k))
    }

    /// 使用摘要和概览向量进行融合搜索。
    ///
    /// 这是一个便捷方法，使用相同的查询向量同时搜索
    /// abstract 和 overview 两个命名向量，并通过 RRF 算法融合结果。
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
            Some(0.5),
        )
        .await
    }
}

/// 使用 RRF (Reciprocal Rank Fusion) 算法融合搜索结果。
///
/// RRF 公式: score(d) = sum(1 / (k + rank(d))) for each query
/// 其中 k 是一个常数（通常为 60）。
fn fuse_results_rrf(
    results: Vec<(String, VectorSearchResult)>,
    top_k: usize,
) -> Vec<VectorSearchResult> {
    use std::collections::HashMap;

    /// RRF 常数。
    const RRF_K: f32 = 60.0;

    // 按向量名称分组并排序
    let mut by_vector: HashMap<String, Vec<VectorSearchResult>> = HashMap::new();
    for (vector_name, result) in results {
        by_vector.entry(vector_name).or_default().push(result);
    }

    // 对每个向量类型的结果按分数排序
    for results in by_vector.values_mut() {
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // 计算每个 URI 的 RRF 分数
    let mut uri_scores: HashMap<String, (f32, VectorSearchResult)> = HashMap::new();

    for results in by_vector.values() {
        for (rank, result) in results.iter().enumerate() {
            let rrf_score = 1.0 / (RRF_K + (rank + 1) as f32);
            let uri = result.uri().to_string();

            uri_scores
                .entry(uri.clone())
                .and_modify(|(score, existing)| {
                    *score += rrf_score;
                    // 保留分数更高的原始结果
                    if result.score > existing.score {
                        *existing = result.clone();
                    }
                })
                .or_insert_with(|| (rrf_score, result.clone()));
        }
    }

    // 按 RRF 分数排序并返回 top_k
    let mut fused: Vec<VectorSearchResult> = uri_scores
        .into_values()
        .map(|(rrf_score, mut result)| {
            // 用 RRF 分数替换原始分数
            result.score = rrf_score;
            result
        })
        .collect();

    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    fused.truncate(top_k);

    fused
}

/// 虚拟文件系统 trait。
///
/// 此 trait 定义了提供所有上下文条目统一视图的虚拟文件系统接口。
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    /// 初始化文件系统。
    async fn initialize(&self) -> Result<()>;

    /// 检查条目是否存在。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;

    /// 获取条目。
    async fn get_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 创建目录。
    async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 创建文件。
    async fn create_file(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 写入条目内容。
    async fn write_content(&self, uri: &TianyanUri, content: &str) -> Result<()>;

    /// 读取条目内容。
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;

    /// 追加内容到文件末尾。
    ///
    /// 此方法用于需要持续追加的场景（如会话消息记录）。
    /// 实现应该调用底层存储的真正追加方法。
    ///
    /// # 参数
    /// - `uri`: 条目 URI
    /// - `content`: 要追加的内容
    ///
    /// # 返回
    /// - `Ok(())`: 追加成功
    /// - `Err`: 追加失败（如文件不存在、权限问题等）。
    async fn append_content(&self, uri: &TianyanUri, content: &str) -> Result<()>;

    /// 更新条目元数据。
    async fn update_metadata(
        &self,
        uri: &TianyanUri,
        importance: f32,
        custom: HashMap<String, serde_json::Value>,
    ) -> Result<()>;

    /// 删除条目。
    async fn delete(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出目录中的条目。
    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;

    /// 移动条目。
    async fn move_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()>;

    /// 复制条目。
    async fn copy_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()>;

    /// 搜索条目。
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;

    /// 获取存储后端。
    fn storage_backend(&self) -> &dyn StorageBackend;

    /// 获取向量存储。
    fn vector_storage(&self) -> &dyn VectorStorage;

    /// 获取向量存储（Arc 版本，供 Agent 使用）。
    fn get_vector_storage(&self) -> Arc<dyn VectorStorage>;

    /// 写入摘要内容（Abstract 层级）。
    async fn write_abstract(&self, uri: &TianyanUri, content: &str) -> Result<()>;

    /// 写入概览内容（Overview 层级）。
    async fn write_overview(&self, uri: &TianyanUri, content: &str) -> Result<()>;

    /// 更新条目的摘要和概览向量
    async fn update_summary_vectors(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<()>;

    /// 检查特定层级是否存在内容。
    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool>;

    /// 获取特定层级内容的元数据
    async fn get_content_metadata(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
    ) -> Result<Option<ContentMetadata>>;

    /// 批量获取所有层级的内容元数据（优化扫描）。
    async fn get_all_content_metadata(
        &self,
        uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, Option<ContentMetadata>>>;

    /// 按命名空间搜索。
    async fn search_by_namespace(
        &self,
        namespace: ContextNamespace,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>>;

    /// 搜索会话
    async fn search_session(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;

    /// 搜索记忆
    async fn search_memory(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;

    /// 搜索知识
    async fn search_knowledge(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;

    /// 搜索技能
    async fn search_skill(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;

    /// 读取文件。
    async fn read_file(&self, uri: &TianyanUri, filename: &str) -> Result<String>;

    /// 写入文件。
    async fn write_file(&self, uri: &TianyanUri, filename: &str, content: &str) -> Result<()>;

    /// 检查文件是否存在。
    async fn file_exists(&self, uri: &TianyanUri, filename: &str) -> Result<bool>;

    /// 列出文件。
    async fn list_files(&self, uri: &TianyanUri) -> Result<Vec<String>>;

    /// 读取摘要内容。
    async fn read_abstract(&self, uri: &TianyanUri) -> Result<String>;

    /// 读取概览内容。
    async fn read_overview(&self, uri: &TianyanUri) -> Result<String>;

    /// 重新生成元数据。
    async fn regenerate_metadata(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出所有 URI。
    async fn list_all_uris(&self) -> Result<Vec<TianyanUri>>;
}

/// 内容加载 trait。
///
/// 此 trait 定义了从存储加载内容的接口，供检索层使用。
#[async_trait]
pub trait ContentLoader: Send + Sync {
    /// 加载特定层级的内容。
    async fn load_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;

    /// 加载条目。
    async fn load_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 检查特定层级是否存在内容。
    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    // trait 测试占位符
    #[test]
    fn test_traits_defined() {
        // 此测试仅验证 trait 能正确编译
        assert!(true);
    }
}
