//! 工具执行审批工作流。
//!
//! 本模块提供工具执行前的审批机制，对标 OpenClaw 的 exec approval 系统：
//! - 危险操作需要用户明确审批
//! - 支持自动审批规则（基于命令模式、文件路径等）
//! - 审批决策持久化，支持审计追踪
//! - 超时机制和默认拒绝策略
//!
//! 按职责拆分：
//! - [`types`]：审批领域数据类型（风险等级 / 请求 / 响应 / 规则 / 配置）
//! - [`workflow`]：审批工作流管理器（状态机 + 自动规则匹配 + 人工审批通道）

pub mod types;

mod workflow;

pub use types::*;
pub use workflow::{
    build_approval_pending_text, ApprovalPendingNotifier, ApprovalWorkflow, SessionApprovalNotifier,
};

/// 测试模块（拆分至独立文件，保持主模块聚焦生产逻辑）。
#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
