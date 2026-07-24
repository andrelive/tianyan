//! 会话领域模块
//!
//! 本模块提供会话管理功能，包括：
//! - 列出所有会话
//! - 创建新会话
//! - 获取会话消息
//! - 删除会话
//! - 会话持久化

/// 会话请求处理函数
pub mod handlers;
/// 会话路由定义
pub mod routes;
/// 会话业务逻辑
pub mod services;
/// 会话类型定义
pub mod types;

pub use routes::routes;
pub use types::{
    CreateSessionRequest, CreateSessionResponse, DeleteSessionResponse, ListSessionsResponse,
    Session, SessionMessagesResponse, SessionMetadata,
};
