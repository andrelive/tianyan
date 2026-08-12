//! 存储后端和虚拟文件系统 trait。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, SearchResult, TianyanUri};

use super::types::ContextEntry;
use crate::common::types::EntryMetadata;

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

    /// 批量获取所有层级的内容元数据。
    async fn get_all_content_metadata(
        &self,
        uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>>;
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

    /// 为条目生成并存储索引向量。
    ///
    /// 将 abstract/overview 文本 embed 为向量，与可选的 visual_vector
    /// 和自定义 payload 元数据一起写入向量库。一次调用完成完整索引。
    ///
    /// 默认实现委托给 `update_summary_vectors()`（忽略 visual_vector 和 payload），
    /// 以保持与未实现本方法的 mock 后向兼容。
    async fn index_entry(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
        visual_vector: Option<Vec<f32>>,
        payload: EntryMetadata,
        embedding_model: &str,
    ) -> Result<()> {
        let _ = (visual_vector, payload, embedding_model);
        self.update_summary_vectors(uri, abstract_content, overview_content)
            .await
    }
}

/// 组合超 trait —— 提供统一的 VirtualFileSystem 接口。
///
/// 任何同时实现了上述三个子 trait 的类型自动实现本 trait。
#[async_trait]
pub trait VirtualFileSystem: VfsCore + ContentStore + VfsSearch {
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_traits_defined() {}
}
