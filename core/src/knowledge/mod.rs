//! 知识管理模块。
//!
//! 本模块提供知识管理能力，包括文档解析、图像理解和知识导入。
//!
//! # 架构
//!
//! 知识系统由以下组件组成：
//!
//! - **解析器**：支持多种格式的文档解析器（PDF、DOCX、Markdown 等），将异构格式转为纯文本
//! - **图像**：图像处理和基于 VLM 的理解（VLM 分析 → 统一文本表示）
//! - **导入器**：知识导入协调器，解析文档后直接写入 VFS，由 SummaryEngine 生成分层摘要
//!
//! 导入器不会对文档做切片——VFS 的分层摘要 + 双层检索替代了传统 RAG 的切片逻辑。
//!
//! # 示例
//!
//! ```no_run
//! use tianyan::knowledge::CompositeParser;
//!
//! fn demo() {
//!     let parser = CompositeParser::new();
//!     // 解析后直接写入 VFS，由 SummaryEngine 自动生成 overview/abstract
//! }
//! ```

mod image;
mod ingestor;
mod parser;
mod types;

pub use image::{
    ExifMetadata, ImageAnalysis, ImageAnalyzer, ImageFormatType, ImageProcessor,
    ImageProcessorConfig, ImageType, ProcessedImage, UnifiedTextRepresentation,
};
pub use ingestor::{IngestorConfig, KnowledgeIngestor};
pub use parser::{
    CodeParser, CompositeParser, DocumentMetadata, DocumentParser, DocxParser, MarkdownParser,
    ParsedDocument, PdfParser, TextParser, TocEntry,
};
pub use types::{
    DocumentType, IngestionRequest, IngestionResult, KnowledgeCategory, KnowledgeDocument,
    KnowledgeMetadata, KnowledgeSearchResult,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _parser = CompositeParser::new();
        let _image_config = ImageProcessorConfig::new();
        let _ingestor_config = IngestorConfig::new();
    }
}
