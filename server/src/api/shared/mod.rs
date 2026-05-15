//! 共享模块
//!
//! 本模块包含 API 层通用的类型、错误处理和响应格式，
//! 可在各个 API 子模块间复用，确保一致性和代码复用。
//!
//! # 模块结构
//!
//! - [`types`] - 通用类型定义（消息、Token 使用等）
//! - [`error`] - 错误处理（ApiError 枚举、错误响应）
//! - [`response`] - 响应格式（ApiResponse 包装器、分页响应）
//!
//! # 使用示例
//!
//! ```
//! use tianyan_server::api::shared::types::{ChatMessage, MessageRole, TokenUsage};
//! use tianyan_server::api::shared::error::ApiError;
//! use tianyan_server::api::shared::response::ApiResponse;
//!
//! // 创建聊天消息
//! let message = ChatMessage::user("Hello");
//!
//! // 创建成功响应
//! let response = ApiResponse::success(message);
//!
//! // 创建错误
//! let error = ApiError::NotFound("资源不存在".to_string());
//! ```

pub mod error;
pub mod response;
pub mod types;

// 重新导出常用类型，方便使用
pub use error::{ApiError, ErrorResponse};
pub use types::{short_uuid, ChatMessage, MessageRole, TokenUsage};
