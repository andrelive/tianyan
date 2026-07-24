//! 长期记忆提取模块。
//!
//! 本模块提供从会话文本中提取结构化记忆的纯功能，
//! 将提取逻辑与调度、持久化解耦。

mod extractor;

pub use extractor::{
    format_memory_as_markdown, ExtractionConfig, MemoryExtractor, DEFAULT_EXTRACTION_PROMPT,
};
