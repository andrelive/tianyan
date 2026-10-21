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

// ADR-049 期 2 序 4：VFS 存储实现（`LocalFileBackend` / `SqliteBackend`）已迁至
// `plugins/storage-sqlite`。core 的**单元测试**（`--lib`）却必须能构造它们：单测与
// `dev-dependencies` 里的插件会各自链一份 `tianyan`（一个带 `cfg(test)`、一个不带），
// 于是同一个 `StorageBackend` trait 出现两个互不兼容的实例，跨 crate 传类型直接编译失败。
// 因此测试期以 `#[path]` **同源包含**插件实现（见 `vfs/backend/mod.rs`），本别名让那份
// 源码里的 `tianyan::…` 在同一 crate 内指向自身。仅 `cfg(test)` 生效，正常构建无此行。
#[cfg(test)]
extern crate self as tianyan;

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

/// 测试工具（`cfg(test)` 或 `test-utils` 特性下编译）。
///
/// 除了 core 自己的单测，**插件 crate 的单元测试**也要用它（`MockVfs` /
/// `MockChatService`）：ADR-049 期 2 序 5a 把 5 个内置任务搬进
/// `plugins/task-builtin` 时，那些测试的写法与判别力必须原样保留——若替身不可达，
/// 只能复制一份进插件（两份实现同时漂移，测试也就失去意义）。
///
/// `test-utils` 特性由插件的 `[dev-dependencies]` 打开，**默认构建不编译**
/// （`cargo tree -p tianyan-core` 无 mockall）。测试替身里大量 `.unwrap()`
/// 是刻意的（失败即 panic 即测试失败），故模块级豁免 clippy 的 unwrap 组——
/// 特性构建不是 `cfg(test)`，拿不到 crate 级的 `cfg_attr(test, allow(..))`。
#[cfg(any(test, feature = "test-utils"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unwrap_in_result)]
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
