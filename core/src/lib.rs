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
//! 系统由以下几个关键模块组成（完整索引见 `docs/architecture/module-map.md`）：
//!
//! 基础层：
//! - [`common`][]: 通用类型、错误（4 变体 + 语义谓词）、日志、token 估算
//! - [`config`][]: 配置管理（含安全策略 `approval_mode`/`safety_mode`，ADR-033）
//! - [`roles`][]: 角色基础类型；[`db`][]: 统一写入门面（ADR-020）
//!
//! 存储层：
//! - [`vfs`][]: 统一存储与检索（L0/L1/L2 + RRF 融合）
//! - [`role_store`][]: 角色 VFS 存储
//!
//! 领域层：
//! - [`agent`][]: Agent 协调器 + AgentLoop + ToolRegistry + 后台任务
//! - [`context`][]: 上下文工程（检索 + 压缩 + 组装）
//! - [`session`][]: 会话权威存储（ADR-018）+ 回忆检索
//! - [`model`][]: 模型服务容器；[`scheduler`][]: 定时任务
//! - [`knowledge`][]: 知识库导入；[`skills`][]: 技能（VFS 方法论文档）
//! - [`executor`][]: 工具执行支撑 + 语义化编辑/浏览/代码智能
//! - [`observability`][]: 指标/Trace/统计；[`snapshot`][]: 工作区快照（ADR-006 例外）
//! - [`events`][]: 事件驱动触发；[`goals`][]/[`todos`][]: 目标与待办；[`notification`][]: 通知通道
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
/// 统一写入门面（Database Repository 层；单连接 + schema 集中）。
pub mod db;
/// 事件驱动触发（文件监听 + webhook → 事件总线 → 规则动作，T1 路线）。
pub mod events;
/// 执行器（独立执行函数 + Action）。
pub mod executor;
/// 目标管理（长期目标 + 进度跟踪；与待办联动）。
pub mod goals;
pub mod knowledge;
pub mod model;
pub mod notification;
pub mod observability;
/// 角色 VFS 存储（独立存储层，依赖 vfs + roles）。
pub mod role_store;
/// 角色基础类型（纯类型层：config/agent/scheduler 共用）。
pub mod roles;
pub mod scheduler;
pub mod session;
pub mod skills;
/// 工作区文件快照（会话回退时恢复文件修改）。
pub mod snapshot;
/// 待办清单（todolist；用户跟踪多步任务）。
pub mod todos;
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
