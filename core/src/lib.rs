//! Tianyan - 本地智能代理系统。
//!
//! 本 crate 提供了一个由大语言模型驱动的本地智能代理系统。
//! 它采用统一上下文存储架构，具有分层摘要功能，
//! 能够高效检索和管理所有上下文信息。
//!
//! **注意**：这是一个纯库 crate，不包含 CLI 功能。
//!
//! # 架构
//!
//! 系统由以下几个关键模块组成：
//!
//! - [`config`][]: 配置管理
//! - [`common`][]: 通用类型和错误处理
//! - [`model`][]: 模型服务接口
//! - [`vfs`][]: 存储后端和虚拟文件系统
//! - [`context`][]: 上下文工程和检索
//! - [`session`][]: 会话管理
//! - [`knowledge`][]: 知识库管理
//! - [`skills`][]: 技能管理和执行
//! - [`agent`][]: 代理协调器
//!
//! # 示例
//!
//! ```rust,ignore
//! use tianyan::config::get_config;
//!
//! fn example() {
//!     let config = get_config();
//!     println!("数据目录：{:?}", config.storage.data_dir);
//! }
//! ```

// 测试代码中 unwrap/expect 是有意的（失败即 panic 即测试失败），
// 豁免这些 lint 以保持测试可读性。生产代码不受影响。
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::unwrap_in_result)
)]

pub mod agent;
pub mod common;
pub mod config;
pub mod context;
/// 执行器（独立执行函数 + Action）。
pub mod executor;
pub mod knowledge;
pub mod memory;
pub mod model;
pub mod observability;
pub mod scheduler;
pub mod session;
pub mod skills;
/// 工作区文件快照（会话回退时恢复文件修改）。
pub mod snapshot;
pub mod vfs;

/// 测试工具（仅在 cfg(test) 时编译）。
#[cfg(test)]
pub mod test_utils;

// 重新导出常用类型
pub use common::error::{Result, TianyanError};
pub use common::types::{
    ContentLevel, ContextNamespace, Embedding, Message, MessageRole, TianyanUri, TokenUsage,
};

/// Crate 版本号。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Crate 名称。
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// 初始化库。
///
/// # Panics
/// - 配置加载失败时 panic
///
/// # Errors
/// - `TianyanError::Io` - 日志初始化失败
pub fn init() -> Result<()> {
    let config = config::get_config();

    common::logging::init_logging(&config.logging)?;

    tracing::info!("正在初始化 {} v{}", NAME, VERSION);

    Ok(())
}

/// 优雅关闭库。
pub fn shutdown() {
    tracing::info!("正在关闭 {}", NAME);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_name() {
        assert_eq!(NAME, "tianyan-core");
    }
}
