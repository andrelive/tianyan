//! 存储类型定义。

use serde::{Deserialize, Serialize};

use crate::common::types::{ContentLevel, EntryMetadata, TianyanUri};

/// 当前持久化数据的 Schema 版本号。
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// 用于 serde 反序列化时的默认 schema_version。
fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

/// 虚拟文件系统中的上下文条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// Schema 版本号，用于兼容性检查。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// L0 抽象内容（约 100 个 token）
    pub abstract_content: Option<String>,
    /// L1 概览内容（约 2K 个 token）
    pub overview_content: Option<String>,
    /// L2 详情内容（完整内容）
    pub detail_content: Option<String>,
    /// 此条目的元数据（包含 URI 和 is_directory）
    pub metadata: EntryMetadata,
}

impl ContextEntry {
    /// 创建新的文件条目。
    pub fn new_file(uri: TianyanUri) -> Self {
        let metadata = EntryMetadata::new(uri, "unknown").with_directory(false);
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            abstract_content: None,
            overview_content: None,
            detail_content: None,
            metadata,
        }
    }

    /// 创建新的目录条目。
    pub fn new_directory(uri: TianyanUri) -> Self {
        let metadata = EntryMetadata::new(uri, "directory").with_directory(true);
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            abstract_content: None,
            overview_content: None,
            detail_content: None,
            metadata,
        }
    }

    /// 获取条目的 URI。
    pub fn uri(&self) -> &TianyanUri {
        &self.metadata.uri
    }

    /// 检查是否为目录。
    pub fn is_directory(&self) -> bool {
        self.metadata.is_directory
    }

    /// 获取特定层级的内容。
    pub fn get_content(&self, level: ContentLevel) -> Option<&str> {
        match level {
            ContentLevel::Abstract => self.abstract_content.as_deref(),
            ContentLevel::Overview => self.overview_content.as_deref(),
            ContentLevel::Detail => self.detail_content.as_deref(),
        }
    }

    /// 设置特定层级的内容。
    pub fn set_content(&mut self, level: ContentLevel, content: String) {
        match level {
            ContentLevel::Abstract => self.abstract_content = Some(content),
            ContentLevel::Overview => self.overview_content = Some(content),
            ContentLevel::Detail => self.detail_content = Some(content),
        }
    }

    /// 检查特定层级是否存在内容。
    pub fn has_content(&self, level: ContentLevel) -> bool {
        match level {
            ContentLevel::Abstract => self.abstract_content.is_some(),
            ContentLevel::Overview => self.overview_content.is_some(),
            ContentLevel::Detail => self.detail_content.is_some(),
        }
    }
}

/// 向量存储的向量点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorPoint {
    /// Schema 版本号，用于兼容性检查。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// 唯一 ID（从 URI 派生）。
    pub id: String,
    /// L0 抽象向量。
    pub abstract_vector: Option<Vec<f32>>,
    /// L1 概览向量。
    pub overview_vector: Option<Vec<f32>>,
    /// 视觉向量（用于图片）。
    pub visual_vector: Option<Vec<f32>>,
    /// 负载元数据（EntryMetadata 包含 URI 和所有元数据）。
    pub payload: EntryMetadata,
}

impl VectorPoint {
    /// 从 ContextEntry 创建 VectorPoint
    pub fn from_entry(entry: &ContextEntry) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: entry.metadata.uri.to_string(),
            abstract_vector: None,
            overview_vector: None,
            visual_vector: None,
            payload: entry.metadata.clone(),
        }
    }

    /// 获取条目的 URI
    pub fn uri(&self) -> &TianyanUri {
        &self.payload.uri
    }

    /// 检查是否为目录。
    pub fn is_directory(&self) -> bool {
        self.payload.is_directory
    }
}

/// 向量搜索的查询
#[derive(Debug, Clone)]
pub struct VectorSearchQuery {
    /// 查询向量
    pub vector: Vec<f32>,
    /// 要搜索的向量类型（抽象、概览、视觉）
    pub vector_type: VectorType,
    /// 返回结果数量
    pub limit: usize,
    /// 按分类过滤
    pub category_filter: Option<String>,
    /// 最小评分阈值
    pub min_score: Option<f32>,
}

/// 搜索的向量类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorType {
    /// 抽象向量（L0）。
    Abstract,
    /// 概览向量（L1）。
    Overview,
    /// 视觉向量（用于图片）。
    Visual,
}

/// 向量搜索结果
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    /// 点 ID
    pub id: String,
    /// 相似度评分
    pub score: f32,
    /// 负载元数据
    pub payload: EntryMetadata,
}

impl VectorSearchResult {
    /// 获取条目的 URI
    pub fn uri(&self) -> &TianyanUri {
        &self.payload.uri
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_entry() {
        let uri = TianyanUri::parse("tianyan://user/profile/basic").unwrap();
        let mut entry = ContextEntry::new_file(uri.clone());
        entry.set_content(ContentLevel::Abstract, "测试摘要".to_string());
        assert!(entry.has_content(ContentLevel::Abstract));
        assert_eq!(entry.get_content(ContentLevel::Abstract), Some("测试摘要"));
        assert_eq!(entry.uri().as_str(), uri.as_str());
        assert!(!entry.is_directory());
    }

    #[test]
    fn test_context_entry_directory() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let entry = ContextEntry::new_directory(uri.clone());
        assert_eq!(entry.uri().as_str(), uri.as_str());
        assert!(entry.is_directory());
        assert_eq!(entry.metadata.content_type, "directory");
    }
}
