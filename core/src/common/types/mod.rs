//! Tianyan 代理系统的核心类型定义。
//!
//! 本模块定义了整个系统使用的基础类型，
//! 包括 URI 处理、内容层级、元数据和其他常用类型。
//! 按领域拆分为独立子模块，通过本文件统一重新导出。

mod content;
mod embedding;
pub(crate) mod injectable;
mod memory;
mod message;
mod metadata;
mod namespace;
mod search;
mod structured_message;
mod token;
pub(crate) mod tool;
mod uri;

pub use injectable::InjectableContext;

pub use content::{ContentLevel, ContentSource, EntryType};
pub use embedding::Embedding;
pub use memory::{MemoryCategory, MemoryEntry};
pub use message::{Message, MessageRole};
pub use metadata::EntryMetadata;
pub use namespace::ContextNamespace;
pub use search::SearchResult;
pub use structured_message::{
    CacheUsage, DetailedTokenUsage, MessageTime, Part, PartTime, StructuredMessage,
};
pub use token::TokenUsage;
pub use tool::{FunctionCall, ToolCall, ToolCallType};
pub use uri::{AgentPath, TianyanUri, TIANYAN_URI_SCHEME};
