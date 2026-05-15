//! 对话领域模块
//!
//! 本模块提供对话完成功能，包括：
//! - 非流式对话完成
//! - 流式对话完成（SSE）
//! - 会话管理集成
//! - 记忆持久化

pub mod handlers;
pub mod routes;
pub mod services;
pub mod types;

pub use routes::routes;
pub use types::{ChatRequest, ChatResponse, ChatStreamEvent};
