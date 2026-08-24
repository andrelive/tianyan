//! 对话领域模块
//!
//! 本模块提供对话完成功能：流式对话完成（SSE，含追问流式）
//! 与会话管理集成。非流式通道（/chat、/chat/clarify）已移除——
//! GUI 只用 SSE，避免维护两份解析逻辑。

/// 聊天请求处理函数
pub mod handlers;
/// 聊天路由定义
pub mod routes;
/// 聊天业务逻辑
pub mod services;
/// 聊天类型定义
pub mod types;

pub use routes::routes;
pub use types::{ChatRequest, ChatStreamEvent};
