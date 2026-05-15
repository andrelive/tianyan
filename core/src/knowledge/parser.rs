//! 知识导入的文档解析器。
//!
//! 本模块提供多种文档格式的解析器，包括 PDF、DOCX、Markdown 和纯文本文件。

use std::path::Path;

use crate::common::error::{Result, TianyanError};

use super::types::DocumentType;

/// 解析后的文档内容。
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    /// 提取的文本内容。
    pub text: String,
    /// 文档类型。
    pub doc_type: DocumentType,
    /// 页数（用于 PDF）。
    pub page_count: Option<usize>,
    /// 检测到的语言（如果可用）。
    pub language: Option<String>,
    /// 目录（用于结构化文档）。
    pub toc: Vec<TocEntry>,
    /// 从文档中提取的元数据。
    pub metadata: DocumentMetadata,
}

/// 目录条目。
#[derive(Debug, Clone)]
pub struct TocEntry {
    /// 条目标题。
    pub title: String,
    /// 条目级别（1 = 顶级）。
    pub level: usize,
    /// 页码（如果可用）。
    pub page: Option<usize>,
    /// 锚点/ID（如果可用）。
    pub anchor: Option<String>,
}

/// 文档元数据。
#[derive(Debug, Clone, Default)]
pub struct DocumentMetadata {
    /// 文档标题。
    pub title: Option<String>,
    /// 文档作者。
    pub author: Option<String>,
    /// 创建日期。
    pub created_at: Option<String>,
    /// 修改日期。
    pub modified_at: Option<String>,
    /// 主题/描述。
    pub subject: Option<String>,
    /// 关键词。
    pub keywords: Vec<String>,
    /// 自定义元数据字段。
    pub custom: std::collections::HashMap<String, String>,
}

/// 文档解析器 trait。
pub trait DocumentParser: Send + Sync {
    /// 从给定内容解析文档。
    fn parse(&self, content: &[u8], filename: &str) -> Result<ParsedDocument>;

    /// 检查此解析器是否支持给定的文件扩展名。
    fn supports_extension(&self, ext: &str) -> bool;

    /// 获取此解析器处理的文档类型。
    fn document_type(&self) -> DocumentType;
}

/// PDF 文档解析器。
pub struct PdfParser;

impl PdfParser {
    /// 创建新的 PDF 解析器。
    pub fn new() -> Self {
        Self
    }
}

impl Default for PdfParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentParser for PdfParser {
    fn parse(&self, content: &[u8], _filename: &str) -> Result<ParsedDocument> {
        // 使用 pdf-extract 提取文本
        let text = pdf_extract::extract_text_from_mem(content)
            .map_err(|e| TianyanError::DocumentProcessing(format!("PDF 解析失败: {}", e)))?;

        // 通过查找分页符来计算近似页数
        let page_count = text.matches('\x0c').count().max(1);

        Ok(ParsedDocument {
            text,
            doc_type: DocumentType::Pdf,
            page_count: Some(page_count),
            language: None,
            toc: Vec::new(),
            metadata: DocumentMetadata::default(),
        })
    }

    fn supports_extension(&self, ext: &str) -> bool {
        matches!(ext.to_lowercase().as_str(), "pdf")
    }

    fn document_type(&self) -> DocumentType {
        DocumentType::Pdf
    }
}

/// DOCX 文档解析器。
pub struct DocxParser;

impl DocxParser {
    /// 创建新的 DOCX 解析器。
    pub fn new() -> Self {
        Self
    }
}

impl Default for DocxParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentParser for DocxParser {
    fn parse(&self, content: &[u8], _filename: &str) -> Result<ParsedDocument> {
        // 使用 docx-rs 提取文本
        let doc = docx_rs::read_docx(content)
            .map_err(|e| TianyanError::DocumentProcessing(format!("DOCX 解析失败: {}", e)))?;

        let mut text = String::new();

        // 从文档中提取文本 - 简化方。
        for child in doc.document.children {
            match child {
                docx_rs::DocumentChild::Paragraph(para) => {
                    for content in para.children {
                        if let docx_rs::ParagraphChild::Run(run) = content {
                            for run_child in run.children {
                                if let docx_rs::RunChild::Text(text_elem) = run_child {
                                    text.push_str(&text_elem.text);
                                }
                            }
                        }
                    }
                    text.push('\n');
                }
                docx_rs::DocumentChild::Table(table) => {
                    for row in table.rows {
                        let docx_rs::TableChild::TableRow(table_row) = row;
                        for cell in table_row.cells {
                            let docx_rs::TableRowChild::TableCell(table_cell) = cell;
                            for content in table_cell.children {
                                if let docx_rs::TableCellContent::Paragraph(para) = content {
                                    for para_content in para.children {
                                        if let docx_rs::ParagraphChild::Run(run) = para_content {
                                            for run_child in run.children {
                                                if let docx_rs::RunChild::Text(text_elem) =
                                                    run_child
                                                {
                                                    text.push_str(&text_elem.text);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            text.push('\t');
                        }
                        text.push('\n');
                    }
                }
                _ => {}
            }
        }

        // 提取元数据（简化版 - docx-rs 不直接暴露 core_properties）
        let metadata = DocumentMetadata::default();

        Ok(ParsedDocument {
            text,
            doc_type: DocumentType::Docx,
            page_count: None,
            language: None,
            toc: Vec::new(),
            metadata,
        })
    }

    fn supports_extension(&self, ext: &str) -> bool {
        matches!(ext.to_lowercase().as_str(), "docx")
    }

    fn document_type(&self) -> DocumentType {
        DocumentType::Docx
    }
}

/// Markdown 文档解析器。
pub struct MarkdownParser;

impl MarkdownParser {
    /// 创建新的 Markdown 解析器。
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarkdownParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentParser for MarkdownParser {
    fn parse(&self, content: &[u8], _filename: &str) -> Result<ParsedDocument> {
        let text = String::from_utf8_lossy(content).to_string();

        // 解析 markdown 并提取目。
        let mut toc = Vec::new();

        use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};

        let parser = Parser::new(&text);
        let mut in_heading = false;
        let mut current_heading_text = String::new();
        let mut current_heading_level = 1;

        for event in parser {
            match event {
                Event::Start(Tag::Heading(level, _, _)) => {
                    in_heading = true;
                    current_heading_text.clear();
                    current_heading_level = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                }
                Event::Text(text) if in_heading => {
                    current_heading_text.push_str(&text);
                }
                Event::End(Tag::Heading(_, _, _)) => {
                    if in_heading && !current_heading_text.is_empty() {
                        toc.push(TocEntry {
                            title: current_heading_text.trim().to_string(),
                            level: current_heading_level,
                            page: None,
                            anchor: None,
                        });
                    }
                    in_heading = false;
                }
                _ => {}
            }
        }

        Ok(ParsedDocument {
            text,
            doc_type: DocumentType::Markdown,
            page_count: None,
            language: None,
            toc,
            metadata: DocumentMetadata::default(),
        })
    }

    fn supports_extension(&self, ext: &str) -> bool {
        matches!(ext.to_lowercase().as_str(), "md" | "markdown")
    }

    fn document_type(&self) -> DocumentType {
        DocumentType::Markdown
    }
}

/// 纯文本文档解析器。
pub struct TextParser;

impl TextParser {
    /// 创建新的文本解析器。
    pub fn new() -> Self {
        Self
    }
}

impl Default for TextParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentParser for TextParser {
    fn parse(&self, content: &[u8], _filename: &str) -> Result<ParsedDocument> {
        let text = String::from_utf8_lossy(content).to_string();

        Ok(ParsedDocument {
            text,
            doc_type: DocumentType::Text,
            page_count: None,
            language: None,
            toc: Vec::new(),
            metadata: DocumentMetadata::default(),
        })
    }

    fn supports_extension(&self, ext: &str) -> bool {
        matches!(ext.to_lowercase().as_str(), "txt" | "text")
    }

    fn document_type(&self) -> DocumentType {
        DocumentType::Text
    }
}

/// 代码文件解析器。
pub struct CodeParser {
    /// 编程语言。
    language: String,
}

impl CodeParser {
    /// 为给定语言创建新的代码解析器。
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
        }
    }
}

impl DocumentParser for CodeParser {
    fn parse(&self, content: &[u8], _filename: &str) -> Result<ParsedDocument> {
        let text = String::from_utf8_lossy(content).to_string();

        Ok(ParsedDocument {
            text,
            doc_type: DocumentType::Code,
            page_count: None,
            language: Some(self.language.clone()),
            toc: Vec::new(),
            metadata: DocumentMetadata::default(),
        })
    }

    fn supports_extension(&self, ext: &str) -> bool {
        // 将扩展名映射到语言
        matches!(
            ext.to_lowercase().as_str(),
            "rs" | "py" | "js" | "ts" | "java" | "c" | "cpp" | "go" | "rb" | "php" | "swift" | "kt"
        )
    }

    fn document_type(&self) -> DocumentType {
        DocumentType::Code
    }
}

/// 组合解析器，根据文件扩展名委托给适当的解析器。
pub struct CompositeParser {
    parsers: Vec<Box<dyn DocumentParser>>,
}

impl CompositeParser {
    /// 创建包含所有内置解析器的新组合解析器。
    pub fn new() -> Self {
        let parsers: Vec<Box<dyn DocumentParser>> = vec![
            Box::new(PdfParser::new()),
            Box::new(DocxParser::new()),
            Box::new(MarkdownParser::new()),
            Box::new(TextParser::new()),
            Box::new(CodeParser::new("auto")),
        ];
        Self { parsers }
    }

    /// 添加自定义解析器。
    pub fn with_parser(mut self, parser: Box<dyn DocumentParser>) -> Self {
        self.parsers.push(parser);
        self
    }

    /// 获取文件对应的解析器。
    pub fn get_parser(&self, path: &Path) -> Option<&dyn DocumentParser> {
        let ext = path.extension()?.to_str()?;
        self.parsers
            .iter()
            .find(|p| p.supports_extension(ext))
            .map(|p| p.as_ref())
    }

    /// 从字节解析文件。
    pub fn parse_file(&self, content: &[u8], path: &Path) -> Result<ParsedDocument> {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        if let Some(parser) = self.get_parser(path) {
            parser.parse(content, filename)
        } else {
            // 默认使用文本解析。
            TextParser::new().parse(content, filename)
        }
    }

    /// 从文件扩展名检测文档类型。
    pub fn detect_type(&self, path: &Path) -> DocumentType {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        for parser in &self.parsers {
            if parser.supports_extension(ext) {
                return parser.document_type();
            }
        }

        DocumentType::Other
    }
}

impl Default for CompositeParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_parser() {
        let parser = TextParser::new();
        let content = b"Hello, world!\nThis is a test.";
        let result = parser.parse(content, "test.txt").unwrap();

        assert_eq!(result.doc_type, DocumentType::Text);
        assert!(result.text.contains("Hello, world!"));
    }

    #[test]
    fn test_markdown_parser() {
        let parser = MarkdownParser::new();
        let content = b"# Title\n\n## Section 1\n\nContent here.\n\n## Section 2\n\nMore content.";
        let result = parser.parse(content, "test.md").unwrap();

        assert_eq!(result.doc_type, DocumentType::Markdown);
        assert!(result.toc.len() >= 2);
        assert!(result.toc.iter().any(|t| t.title == "Title"));
    }

    #[test]
    fn test_composite_parser() {
        let parser = CompositeParser::new();

        // 测试文本文件
        let text_content = b"Plain text content";
        let result = parser
            .parse_file(text_content, Path::new("test.txt"))
            .unwrap();
        assert_eq!(result.doc_type, DocumentType::Text);

        // 测试 markdown 文件
        let md_content = b"# Heading\n\nContent";
        let result = parser.parse_file(md_content, Path::new("test.md")).unwrap();
        assert_eq!(result.doc_type, DocumentType::Markdown);
    }

    #[test]
    fn test_detect_type() {
        let parser = CompositeParser::new();

        assert_eq!(parser.detect_type(Path::new("test.pdf")), DocumentType::Pdf);
        assert_eq!(
            parser.detect_type(Path::new("test.docx")),
            DocumentType::Docx
        );
        assert_eq!(
            parser.detect_type(Path::new("test.md")),
            DocumentType::Markdown
        );
        assert_eq!(
            parser.detect_type(Path::new("test.txt")),
            DocumentType::Text
        );
        assert_eq!(parser.detect_type(Path::new("test.rs")), DocumentType::Code);
    }

    #[test]
    fn test_supports_extension() {
        let pdf_parser = PdfParser::new();
        assert!(pdf_parser.supports_extension("pdf"));
        assert!(!pdf_parser.supports_extension("txt"));

        let md_parser = MarkdownParser::new();
        assert!(md_parser.supports_extension("md"));
        assert!(md_parser.supports_extension("markdown"));
    }
}
