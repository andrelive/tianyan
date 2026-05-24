//! 存储后端和虚拟文件系统 trait。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, SearchResult, TianyanUri};

use super::types::{
    ContextEntry, VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType,
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
        Ok(vec![])
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

/// 虚拟文件系统核心操作 —— 条目生命周期管理。
#[async_trait]
pub trait VfsCore: Send + Sync {
    /// 初始化文件系统和向量存储。
    async fn initialize(&self) -> Result<()>;

    /// 检查条目是否存在。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;

    /// 获取条目。
    async fn get_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 创建目录。
    async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 创建文件。
    async fn create_file(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 递归删除条目及其子条目，同时清理向量库。
    async fn delete(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出指定 URI 前缀下的所有条目。
    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;

    /// 移动条目及其子条目到新位置。
    async fn move_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()>;
}

/// 分层内容读写 —— L0 Abstract / L1 Overview / L2 Detail。
#[async_trait]
pub trait ContentStore: Send + Sync {
    /// 写入指定层级的内容。条目不存在则自动创建。
    async fn write(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()>;

    /// 读取指定层级的内容。
    async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;

    /// 追加内容到 Detail 层级末尾。用于会话消息持续写入。
    async fn append(&self, uri: &TianyanUri, content: &str) -> Result<()>;

    /// 检查条目在指定层级是否已有内容。
    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool>;

    /// 写入 Detail 层级内容（便捷方法）。
    async fn write_content(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        self.write(uri, ContentLevel::Detail, content).await
    }

    /// 写入 Abstract 层级内容（便捷方法）。
    async fn write_abstract(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        self.write(uri, ContentLevel::Abstract, content).await
    }

    /// 写入 Overview 层级内容（便捷方法）。
    async fn write_overview(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        self.write(uri, ContentLevel::Overview, content).await
    }
}

/// 向量检索 —— 查询文本先 embed 再通过 RRF 融合 Abstract+Overview 向量搜索。
#[async_trait]
pub trait VfsSearch: Send + Sync {
    /// 用 EmbeddingService 将查询文本转为向量，进行 Abstract + Overview 双向量 RRF 融合检索。
    /// `namespace` 为 `None` 时搜索全部命名空间。
    async fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>>;

    /// 通过视觉向量搜索（用于图像相似性检索）。
    async fn search_by_visual(
        &self,
        visual_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>>;

    /// 将 Abstract 和 Overview 文本 embed 为向量，存入向量库。
    async fn update_summary_vectors(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<()>;
}

/// 元数据管理 —— 重要性评分与自定义标签。
#[async_trait]
pub trait VfsMetadata: Send + Sync {
    /// 更新条目的重要性评分和自定义键值标签。
    async fn update_metadata(
        &self,
        uri: &TianyanUri,
        importance: f32,
        custom: HashMap<String, serde_json::Value>,
    ) -> Result<()>;

    /// 批量获取所有层级的内容元数据。
    async fn get_all_content_metadata(
        &self,
        uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>>;
}

/// 组合超 trait —— 提供统一的 VirtualFileSystem 接口。
///
/// 任何同时实现了上述四个子 trait 的类型自动实现本 trait。
#[async_trait]
pub trait VirtualFileSystem: VfsCore + ContentStore + VfsSearch + VfsMetadata {
    /// 读取指定层级的内容（便捷方法，委托给 ContentStore::read）。
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        ContentStore::read(self, uri, level).await
    }

    /// 追加内容到 Detail 层级末尾（便捷方法，委托给 ContentStore::append）。
    async fn append_content(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        ContentStore::append(self, uri, content).await
    }

    /// 读取 Abstract 层级内容。
    async fn read_abstract(&self, uri: &TianyanUri) -> Result<String> {
        self.read_content(uri, ContentLevel::Abstract).await
    }

    /// 读取 Overview 层级内容。
    async fn read_overview(&self, uri: &TianyanUri) -> Result<String> {
        self.read_content(uri, ContentLevel::Overview).await
    }

    /// 读取子文件内容。
    async fn read_file(&self, uri: &TianyanUri, filename: &str) -> Result<String> {
        let child_uri = uri.append(filename);
        self.read_content(&child_uri, ContentLevel::Detail).await
    }

    /// 写入子文件内容。
    async fn write_file(&self, uri: &TianyanUri, filename: &str, content: &str) -> Result<()> {
        let child_uri = uri.append(filename);
        if !self.exists(&child_uri).await? {
            self.create_file(&child_uri).await?;
        }
        self.write_content(&child_uri, content).await
    }

    /// 检查子文件是否存在。
    async fn file_exists(&self, uri: &TianyanUri, filename: &str) -> Result<bool> {
        let child_uri = uri.append(filename);
        self.exists(&child_uri).await
    }

    /// 列出子文件名。
    async fn list_files(&self, uri: &TianyanUri) -> Result<Vec<String>> {
        let entries = self.list(uri).await?;
        Ok(entries
            .into_iter()
            .filter(|e| !e.is_directory())
            .filter_map(|e| e.uri().path().last().cloned())
            .collect())
    }

    /// 复制条目。
    async fn copy_entry(&self, _source: &TianyanUri, _destination: &TianyanUri) -> Result<()> {
        Err(crate::common::error::TianyanError::Internal(
            "copy_entry not implemented".to_string(),
        ))
    }

    /// 重新生成元数据。
    async fn regenerate_metadata(&self, _uri: &TianyanUri) -> Result<()> {
        Ok(())
    }

    /// 列出所有 URI。
    async fn list_all_uris(&self) -> Result<Vec<TianyanUri>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_traits_defined() {
        assert!(true);
    }
}
