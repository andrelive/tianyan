use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::content::ContentSource;
use super::namespace::ContextNamespace;
use super::uri::TianyanUri;

/// 上下文条目的元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMetadata {
    /// 资源的 URI。
    pub uri: TianyanUri,
    /// 是否为目录。
    pub is_directory: bool,
    /// 内容类型（如 "document", "image", "code" 等）。
    pub content_type: String,
    /// 子分类（如 "rust", "python" 等）。
    pub category: Option<String>,
    /// 内容来源。
    pub source: ContentSource,
    /// 原始文件名。
    pub original_name: Option<String>,
    /// 文件大小（字节）。
    pub file_size: Option<u64>,
    /// 重要性评分（0.0-1.0）。
    pub importance: f32,
    /// 标签列表。
    pub tags: Vec<String>,
    /// 创建时间。
    pub created_at: DateTime<Utc>,
    /// 更新时间。
    pub updated_at: DateTime<Utc>,
    /// 自定义键值对。
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for EntryMetadata {
    fn default() -> Self {
        Self {
            uri: TianyanUri::new(ContextNamespace::User, vec!["default".to_string()]),
            is_directory: false,
            content_type: "unknown".to_string(),
            category: None,
            source: ContentSource::Unknown,
            original_name: None,
            file_size: None,
            importance: 0.5,
            tags: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            custom: HashMap::new(),
        }
    }
}

impl EntryMetadata {
    /// 创建新的元数据。
    pub fn new(uri: TianyanUri, content_type: impl Into<String>) -> Self {
        Self {
            uri,
            content_type: content_type.into(),
            ..Default::default()
        }
    }

    /// 设置是否为目录。
    pub fn with_directory(mut self, is_dir: bool) -> Self {
        self.is_directory = is_dir;
        self
    }

    /// 设置子分类。
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// 设置标签。
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// 设置内容来源。
    pub fn with_source(mut self, source: ContentSource) -> Self {
        self.source = source;
        self
    }

    /// 设置重要性（自动 clamp 到 0.0-1.0）。
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// 添加标签（去重）。
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        let tag = tag.into();
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
    }

    /// 更新时间戳。
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_metadata_default() {
        let metadata = EntryMetadata::default();
        assert!(!metadata.is_directory);
        assert_eq!(metadata.content_type, "unknown");
        assert!(metadata.tags.is_empty());
        assert_eq!(metadata.source, ContentSource::Unknown);
        assert_eq!(metadata.importance, 0.5);
    }

    #[test]
    fn test_entry_metadata_new() {
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["test".to_string()]);
        let metadata = EntryMetadata::new(uri, "document");
        assert_eq!(metadata.content_type, "document");
        assert_eq!(metadata.uri.namespace(), ContextNamespace::Knowledge);
    }

    #[test]
    fn test_entry_metadata_builders() {
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["test".to_string()]);
        let metadata = EntryMetadata::new(uri, "code")
            .with_directory(false)
            .with_category("rust")
            .with_tags(vec!["async".to_string(), "tokio".to_string()])
            .with_source(ContentSource::UserUpload)
            .with_importance(0.9);

        assert_eq!(metadata.content_type, "code");
        assert!(!metadata.is_directory);
        assert_eq!(metadata.category, Some("rust".to_string()));
        assert_eq!(metadata.tags, vec!["async", "tokio"]);
        assert_eq!(metadata.source, ContentSource::UserUpload);
        assert_eq!(metadata.importance, 0.9);
    }

    #[test]
    fn test_entry_metadata_add_tag() {
        let mut metadata = EntryMetadata::default();
        metadata.add_tag("tag1");
        metadata.add_tag("tag2");
        metadata.add_tag("tag1");
        assert_eq!(metadata.tags.len(), 2);
    }

    #[test]
    fn test_entry_metadata_touch() {
        let mut metadata = EntryMetadata::default();
        let original_updated = metadata.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(1));
        metadata.touch();
        assert!(metadata.updated_at > original_updated);
    }

    #[test]
    fn test_entry_metadata_importance_clamp() {
        let metadata = EntryMetadata::default().with_importance(1.5);
        assert_eq!(metadata.importance, 1.0);

        let metadata = EntryMetadata::default().with_importance(-0.5);
        assert_eq!(metadata.importance, 0.0);
    }
}
