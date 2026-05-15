//! 会话领域模块
//!
//! 本模块提供会话管理功能，包括：
//! - 列出所有会话
//! - 创建新会话
//! - 获取会话消息
//! - 删除会话
//! - 会话持久化

pub mod handlers;
pub mod routes;
pub mod services;
pub mod types;

pub use routes::routes;
pub use types::{
    CreateSessionRequest, CreateSessionResponse, DeleteSessionResponse, ListSessionsResponse,
    Session, SessionMessagesResponse, SessionMetadata,
};
