//! 知识管理模块。
//!
//! 本模块提供知识管理能力，包括文档处理、代码索引和知识检索。
//!
//! # 架构
//!
//! 知识系统由以下组件组成：
//!
//! - **解析器**：支持多种格式的文档解析器（PDF、DOCX、Markdown 等）
//! - **分块器**：具有语义感知能力的智能文档分块
//! - **图像**：图像处理和基于 VLM 的理解
//! - **导入器**：知识导入协调器
//!
//! # 示例
//!
//! ```no_run
//! use std::path::Path;
//! use tianyan::knowledge::{CompositeParser, DocumentChunker, ChunkingConfig};
//!
//! async fn process_document() {
//!     let parser = CompositeParser::new();
//!     let chunker = DocumentChunker::with_defaults().unwrap();
//!     
//!     // 解析并分块文档
//!     let content = b"# Hello\n\nWorld";
//!     let parsed = parser.parse_file(content, Path::new("test.md")).unwrap();
//!     let chunks = chunker.chunk_document(&parsed, "doc-1").unwrap();
//! }
//! ```

mod chunker;
mod image;
mod ingestor;
mod parser;
mod types;

pub use chunker::{
    ChunkMetadata, ChunkingConfig, DocumentChunk, DocumentChunker, DEFAULT_CHUNK_OVERLAP,
    DEFAULT_CHUNK_SIZE,
};
pub use image::{
    ExifMetadata, ImageAnalysis, ImageAnalyzer, ImageFormatType, ImageProcessor,
    ImageProcessorConfig, ImageType, ProcessedImage, UnifiedTextRepresentation,
};
pub use ingestor::{IngestorConfig, KnowledgeIngestor, KnowledgeIngestorBuilder};
pub use parser::{
    CodeParser, CompositeParser, DocumentMetadata, DocumentParser, DocxParser, MarkdownParser,
    ParsedDocument, PdfParser, TextParser, TocEntry,
};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // 测试所有导出是否可访问
        let _parser = CompositeParser::new();
        let _config = ChunkingConfig::new();
        let _image_config = ImageProcessorConfig::new();
        let _ingestor_config = IngestorConfig::new();
    }
}
