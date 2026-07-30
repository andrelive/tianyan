//! 通用模块。
//!
//! 包含整个系统共享的通用组件和类型。

pub mod error;
pub mod logging;
pub mod types;

// 重新导出常用类型
pub use error::{Result, TianyanError};
pub use types::{
    ContentLevel, ContentSource, ContextNamespace, Embedding, EntryMetadata, EntryType,
    MemoryCategory, MemoryEntry, Message, MessageRole, SearchResult, TianyanUri, TokenUsage,
};
