//! 知识类型定义。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::common::types::{ContentSource, TianyanUri};

/// 知识文档。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    /// 文档 ID。
    pub id: String,
    /// 文档 URI。
    pub uri: TianyanUri,
    /// 文档标题。
    pub title: Option<String>,
    /// 文档类型。
    pub doc_type: DocumentType,
    /// 内容来源。
    pub source: ContentSource,
    /// 原始文件路径。
    pub original_path: Option<PathBuf>,
    /// 文件大小（字节）。
    pub file_size: u64,
    /// 创建时间。
    pub created_at: DateTime<Utc>,
    /// 最后修改时间。
    pub modified_at: DateTime<Utc>,
    /// 标签。
    pub tags: Vec<String>,
    /// 语言（用于代码）。
    pub language: Option<String>,
    /// 摘要。
    pub summary: Option<String>,
}

/// 文档类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentType {
    /// 文本文档。
    Text,
    /// Markdown 文档。
    Markdown,
    /// PDF 文档。
    Pdf,
    /// Word 文档。
    Docx,
    /// 代码文件。
    Code,
    /// 图像。
    Image,
    /// 数据文件（CSV、JSON 等）。
    Data,
    /// 电子表格。
    Spreadsheet,
    /// 网页。
    WebPage,
    /// 其他。
    Other,
}

impl DocumentType {
    /// 获取内容类型字符串。
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Code => "code",
            Self::Image => "image",
            Self::Data => "data",
            Self::Spreadsheet => "spreadsheet",
            Self::WebPage => "webpage",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for DocumentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content_type())
    }
}

/// 知识导入结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestionResult {
    /// 文档 ID。
    pub document_id: String,
    /// 文档 URI。
    pub uri: TianyanUri,
    /// 处理的 token 数量。
    pub tokens_processed: usize,
    /// 处理时间（毫秒）。
    pub processing_time_ms: u64,
    /// 导入过程中的警告。
    pub warnings: Vec<String>,
}

/// 知识库中的知识分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum KnowledgeCategory {
    /// 技术文档。
    Technical,
    /// 商业文档。
    Business,
    /// 参考资料。
    References,
    /// 截图。
    Screenshots,
    /// 照片。
    Photos,
    /// 图表。
    Diagrams,
    /// 代码片段。
    CodeSnippets,
    /// 数据集。
    Datasets,
    /// 项目知识。
    Projects,
    /// 外部来源。
    External,
    /// 其他。
    #[default]
    Other,
}

impl KnowledgeCategory {
    /// 获取此分类的目录名称。
    pub fn dir_name(&self) -> &'static str {
        match self {
            Self::Technical => "technical",
            Self::Business => "business",
            Self::References => "references",
            Self::Screenshots => "screenshots",
            Self::Photos => "photos",
            Self::Diagrams => "diagrams",
            Self::CodeSnippets => "snippets",
            Self::Datasets => "datasets",
            Self::Projects => "projects",
            Self::External => "external",
            Self::Other => "other",
        }
    }

    /// 从字符串解析。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "technical" => Some(Self::Technical),
            "business" => Some(Self::Business),
            "references" => Some(Self::References),
            "screenshots" => Some(Self::Screenshots),
            "photos" => Some(Self::Photos),
            "diagrams" => Some(Self::Diagrams),
            "snippets" | "code" => Some(Self::CodeSnippets),
            "datasets" | "data" => Some(Self::Datasets),
            "projects" => Some(Self::Projects),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

impl std::fmt::Display for KnowledgeCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.dir_name())
    }
}

/// 知识导入请求。
#[derive(Debug, Clone)]
pub struct IngestionRequest {
    /// 要导入的内容（文件字节）。
    pub content: Vec<u8>,
    /// 原始文件名。
    pub filename: String,
    /// 内容来源。
    pub source: ContentSource,
    /// 可选的分类覆盖。
    pub category: Option<KnowledgeCategory>,
    /// 可选的标签。
    pub tags: Vec<String>,
    /// 是否生成嵌入向量。
    pub generate_embeddings: bool,
}

impl IngestionRequest {
    /// 创建新的导入请求。
    pub fn new(content: Vec<u8>, filename: impl Into<String>) -> Self {
        Self {
            content,
            filename: filename.into(),
            source: ContentSource::UserUpload,
            category: None,
            tags: Vec::new(),
            generate_embeddings: true,
        }
    }

    /// 设置内容来源。
    pub fn with_source(mut self, source: ContentSource) -> Self {
        self.source = source;
        self
    }

    /// 设置分类。
    pub fn with_category(mut self, category: KnowledgeCategory) -> Self {
        self.category = Some(category);
        self
    }

    /// 设置标签。
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// 与内容一起存储的知识元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeMetadata {
    /// 文档 ID。
    pub document_id: String,
    /// 原始文件名。
    pub original_name: String,
    /// 文档类型。
    pub doc_type: DocumentType,
    /// 知识分类。
    pub category: KnowledgeCategory,
    /// 内容来源。
    pub source: ContentSource,
    /// 文件大小（字节）。
    pub file_size: u64,
    /// 内容哈希（SHA-256）。
    pub content_hash: String,
    /// 标签。
    pub tags: Vec<String>,
    /// 语言（用于代码）。
    pub language: Option<String>,
    /// 总 token 数量。
    pub total_tokens: usize,
    /// 创建时间戳。
    pub created_at: DateTime<Utc>,
    /// 最后更新时间戳。
    pub updated_at: DateTime<Utc>,
    /// 重要性分数。
    pub importance: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_type() {
        let doc_type = DocumentType::Markdown;
        assert_eq!(doc_type, DocumentType::Markdown);
        assert_eq!(doc_type.content_type(), "markdown");
        assert_eq!(doc_type.to_string(), "markdown");
    }

    #[test]
    fn test_knowledge_category() {
        assert_eq!(KnowledgeCategory::Technical.dir_name(), "technical");
        assert_eq!(
            KnowledgeCategory::parse("business"),
            Some(KnowledgeCategory::Business)
        );
        assert_eq!(KnowledgeCategory::parse("unknown"), None);
    }

    #[test]
    fn test_ingestion_request() {
        let request = IngestionRequest::new(b"content".to_vec(), "test.txt")
            .with_source(ContentSource::UserUpload)
            .with_category(KnowledgeCategory::Technical)
            .with_tags(vec!["test".to_string()]);

        assert_eq!(request.filename, "test.txt");
        assert_eq!(request.category, Some(KnowledgeCategory::Technical));
        assert_eq!(request.tags, vec!["test"]);
    }

    #[test]
    fn test_knowledge_metadata() {
        let metadata = KnowledgeMetadata {
            document_id: "doc-1".to_string(),
            original_name: "test.pdf".to_string(),
            doc_type: DocumentType::Pdf,
            category: KnowledgeCategory::Technical,
            source: ContentSource::UserUpload,
            file_size: 1024,
            content_hash: "abc123".to_string(),
            tags: vec!["api".to_string()],
            language: None,
            total_tokens: 500,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            importance: 0.8,
        };

        assert_eq!(metadata.document_id, "doc-1");
        assert_eq!(metadata.doc_type, DocumentType::Pdf);
    }
}
