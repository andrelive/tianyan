//! 对话领域模块
//!
//! 本模块提供对话完成功能，包括：
//! - 非流式对话完成
//! - 流式对话完成（SSE）
//! - 会话管理集成
//! - 记忆持久化

/// 聊天请求处理函数
pub mod handlers;
/// 聊天路由定义
pub mod routes;
/// 聊天业务逻辑
pub mod services;
/// 聊天类型定义
pub mod types;

pub use routes::routes;
pub use types::{ChatRequest, ChatResponse, ChatStreamEvent};
