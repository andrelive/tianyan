//! 天演智能体系统的会话管理模块。
//!
//! 本模块提供会话管理能力，包括：
//!
//! - **会话类型**：会话、消息记录等核心数据结构
//! - **会话管理**：会话创建、维护、关键信息提取和摘要生成
//! - **持久化**：支持内存和虚拟文件系统两种存储方式
//!
//! # 架构
//!
//! 会话模块分为两个层次：
//!
//! 1. **类型层** (`types.rs`)：会话、消息记录和提取信息的核心数据结构
//! 2. **管理层** (`manager.rs`)：会话管理器和持久化实现
//!
//! # 示例
//!
//! ```no_run
//! use std::sync::Arc;
//! use tianyan::session::{PersistentSessionManager, SessionManager};
//! use tianyan::common::types::Message;
//!
//! async fn example() {

//! }
//! ```

mod manager;
mod types;

/// 会话消息数量上限（持久态安全上限）。
///
/// 从 VFS 加载会话时最多保留的消息条数（`manager.rs`），同时作为内存态
/// 会话裁剪的触发阈值（`agent/session_state.rs`）。
pub const MAX_SESSION_MESSAGES: usize = 100;

/// 内存态裁剪后保留的最近消息条数。
///
/// 会话历史超过上限后，`agent/session_state.rs` 仅保留最近
/// `KEEP_RECENT_MESSAGES` 条消息（含 compression_marker 处理）。
pub const KEEP_RECENT_MESSAGES: usize = 50;

// 重新导出类型
pub use types::{Session, SessionMetadata};

// 重新导出管理层
pub use manager::{PersistentSessionManager, SessionManager};
