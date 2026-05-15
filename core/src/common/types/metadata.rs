use std::collections::HashMap;

use chrono::{DateTime, Utc};
use qdrant_client::qdrant::{value::Kind, Value};
use serde::{Deserialize, Serialize};
use serde_json;

use super::content::ContentSource;
use super::namespace::ContextNamespace;
use super::uri::TianyanUri;
use crate::common::error;

/// 上下文条目的元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMetadata {
    /// 条目的 URI
    pub uri: TianyanUri,
    /// 此条目是否为目录
    #[serde(default)]
    pub is_directory: bool,
    /// 内容类型（文档、图片、代码等）
    pub content_type: String,
    /// 内容类型内的分类
    pub category: Option<String>,
    /// 条目标签
    pub tags: Vec<String>,
    /// 内容来源（用户上传、代理生成、外部导入）
    pub source: ContentSource,
    /// 原始文件名（如适用）
    pub original_name: Option<String>,
    /// 文件大小（字节）
    pub file_size: Option<u64>,
    /// 创建时间戳
    pub created_at: DateTime<Utc>,
    /// 最后更新时间戳
    pub updated_at: DateTime<Utc>,
    /// 重要性评分（0.0 - 1.0）
    pub importance: f32,
    /// 自定义元数据字段
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for EntryMetadata {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            uri: TianyanUri::new(ContextNamespace::User, vec![]),
            is_directory: false,
            content_type: "unknown".to_string(),
            category: None,
            tags: Vec::new(),
            source: ContentSource::Unknown,
            original_name: None,
            file_size: None,
            created_at: now,
            updated_at: now,
            importance: 0.5,
            custom: HashMap::new(),
        }
    }
}

impl EntryMetadata {
    /// 使用给定的 URI 和内容类型创建新元数据。
    pub fn new(uri: TianyanUri, content_type: impl Into<String>) -> Self {
        Self {
            uri,
            content_type: content_type.into(),
            ..Default::default()
        }
    }

    /// 设置是否为目录。
    pub fn with_directory(mut self, is_directory: bool) -> Self {
        self.is_directory = is_directory;
        self
    }

    /// 设置分类。
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// 设置标签。
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// 添加标签。
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        let tag = tag.into();
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
    }

    /// 设置来源。
    pub fn with_source(mut self, source: ContentSource) -> Self {
        self.source = source;
        self
    }

    /// 设置重要性评分。
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// 更新时间戳。
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// 转换为 Qdrant Payload 格式。
    pub fn to_qdrant_payload(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "uri".to_string(),
            serde_json::Value::String(self.uri.to_string()),
        );
        payload.insert(
            "namespace".to_string(),
            serde_json::Value::String(self.uri.namespace().to_string()),
        );
        if let Some(ref sub_category) = self.category {
            payload.insert(
                "sub_category".to_string(),
                serde_json::Value::String(sub_category.clone()),
            );
        }
        payload.insert(
            "entry_type".to_string(),
            serde_json::Value::String(if self.is_directory {
                "directory".to_string()
            } else {
                "file".to_string()
            }),
        );
        payload.insert(
            "is_directory".to_string(),
            serde_json::Value::Bool(self.is_directory),
        );
        payload.insert(
            "content_type".to_string(),
            serde_json::Value::String(self.content_type.clone()),
        );
        payload.insert(
            "source".to_string(),
            serde_json::Value::String(format!("{:?}", self.source).to_lowercase()),
        );
        if let Some(ref original_name) = self.original_name {
            payload.insert(
                "original_name".to_string(),
                serde_json::Value::String(original_name.clone()),
            );
        }
        if let Some(file_size) = self.file_size {
            payload.insert(
                "file_size".to_string(),
                serde_json::Value::Number(serde_json::Number::from(file_size)),
            );
        }
        payload.insert(
            "importance".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(self.importance as f64)
                    .unwrap_or_else(|| serde_json::Number::from(0)),
            ),
        );
        payload.insert(
            "tags".to_string(),
            serde_json::Value::Array(
                self.tags
                    .iter()
                    .map(|t| serde_json::Value::String(t.clone()))
                    .collect(),
            ),
        );
        payload.insert(
            "created_at".to_string(),
            serde_json::Value::String(self.created_at.to_rfc3339()),
        );
        payload.insert(
            "updated_at".to_string(),
            serde_json::Value::String(self.updated_at.to_rfc3339()),
        );
        if !self.custom.is_empty() {
            let custom_map: serde_json::Map<String, serde_json::Value> = self
                .custom
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            payload.insert("custom".to_string(), serde_json::Value::Object(custom_map));
        }
        payload
    }

    /// 从 Qdrant Payload 解析。
    pub fn from_qdrant_payload(
        payload: &HashMap<String, qdrant_client::qdrant::Value>,
    ) -> error::Result<Self> {
        fn get_string(value: &Value) -> Option<String> {
            match &value.kind {
                Some(Kind::StringValue(s)) => Some(s.clone()),
                _ => None,
            }
        }

        fn get_bool(value: &Value) -> bool {
            match &value.kind {
                Some(Kind::BoolValue(b)) => *b,
                _ => false,
            }
        }

        fn get_float(value: &Value) -> Option<f32> {
            match &value.kind {
                Some(Kind::DoubleValue(d)) => Some(*d as f32),
                Some(Kind::IntegerValue(i)) => Some(*i as f32),
                _ => None,
            }
        }

        fn get_u64(value: &Value) -> Option<u64> {
            match &value.kind {
                Some(Kind::IntegerValue(i)) => Some(*i as u64),
                _ => None,
            }
        }

        fn get_string_list(value: &Value) -> Vec<String> {
            match &value.kind {
                Some(Kind::ListValue(l)) => l.values.iter().filter_map(get_string).collect(),
                _ => Vec::new(),
            }
        }

        fn get_object(value: &Value) -> Option<&HashMap<String, Value>> {
            match &value.kind {
                Some(Kind::StructValue(obj)) => Some(&obj.fields),
                _ => None,
            }
        }

        fn parse_content_source(value: &Value) -> ContentSource {
            get_string(value)
                .map(|s| match s.as_str() {
                    "user_upload" => ContentSource::UserUpload,
                    "agent_generated" => ContentSource::AgentGenerated,
                    "external_import" => ContentSource::ExternalImport,
                    _ => ContentSource::Unknown,
                })
                .unwrap_or(ContentSource::Unknown)
        }

        let uri_str = payload.get("uri").and_then(get_string).unwrap_or_default();
        let uri = TianyanUri::parse(&uri_str)?;

        let mut custom = HashMap::new();
        if let Some(custom_value) = payload.get("custom") {
            if let Some(obj) = get_object(custom_value) {
                for (k, v) in obj {
                    if let Ok(json_val) = serde_json::to_value(v) {
                        custom.insert(k.clone(), json_val);
                    }
                }
            }
        }

        Ok(Self {
            uri,
            is_directory: payload.get("is_directory").map(get_bool).unwrap_or(false),
            content_type: payload
                .get("content_type")
                .and_then(get_string)
                .unwrap_or_else(|| "unknown".to_string()),
            category: payload.get("sub_category").and_then(get_string),
            source: payload
                .get("source")
                .map(parse_content_source)
                .unwrap_or(ContentSource::Unknown),
            original_name: payload.get("original_name").and_then(get_string),
            file_size: payload.get("file_size").and_then(get_u64),
            importance: payload.get("importance").and_then(get_float).unwrap_or(0.5),
            tags: payload.get("tags").map(get_string_list).unwrap_or_default(),
            created_at: payload
                .get("created_at")
                .and_then(get_string)
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            updated_at: payload
                .get("updated_at")
                .and_then(get_string)
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            custom,
        })
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
