//! 工具执行审批。**契约留 core，实现已出 core（ADR-049 期 3-T1）**。
//!
//! 本模块提供工具执行前的审批机制，对标 OpenClaw 的 exec approval 系统：
//! - 危险操作需要用户明确审批
//! - 支持自动审批规则（基于命令模式、文件路径等）
//! - 审批决策持久化，支持审计追踪
//! - 超时机制和默认拒绝策略
//!
//! 按职责拆分：
//! - [`types`]：审批领域数据类型（风险等级 / 请求 / 响应 / 规则 / 配置）
//!   —— **留 core**，它们是 A 级 trait `AgentCoordinator` 签名的一部分
//! - [`gate`]：审批门控契约 [`ApprovalGate`]（内核唯一认识的审批抽象）
//!
//! 工作流实现（状态机 + 自动规则匹配 + 人工审批通道 + 审计）已迁入
//! `plugins/policy-approval`（crate `tianyan-policy-approval`）：改审批模式 / 白名单 /
//! 超时 / 规则不再重编内核（ADR-049 D7：按「变化原因」切）。装配层经
//! `ApprovalGateFactory` 注入实例，未装配时 `ToolContext.approval_gate` 为 `None`
//! （放行分支照旧）+ 启动期告警。

pub mod types;

mod gate;

pub use gate::ApprovalGate;
pub use types::*;

// ── 测试期同源包含：实现的**唯一副本仍在插件里** ───────────────────────────
//
// core 的单测（`cargo test -p tianyan-core --lib`）必须能构造**真实**审批工作流：
// 三个用例锁的是"`ApprovalMode` 三态短路"的判别力——`loop_tests` 的
// `test_run_approval_denied_returns_tool_error`（Confirm 模式拒绝 Medium 风险 →
// 工具失败结果入史）、`agent_ops_tests` 的 `test_subagent_command_denied_without_hang`
// （Interactive 模式下子任务不挂起、立即拒绝）、
// `test_subagent_command_approved_via_shared_fingerprint`（指纹共享 → 子任务放行）。
// 用替身会把这些语义抹平成"永远批准/永远拒绝"，等于放弃断言。
//
// 为什么不能靠 `[dev-dependencies] tianyan-policy-approval`：单测要把 core 的类型
// （`ApprovalGate` trait / `ApprovalWorkflowConfig` / `dyn ApprovalGate`）传进插件
// API，而 `cfg(test)` 的 core 与插件依赖的 core 是**两个编译单元**，trait 与类型随之
// 出现两个互不兼容的实例（ADR-049 期 2 序 4 实测过 22 处 E0308/E0277）。
//
// 所以测试期用 `#[path]` 把插件源码**同源包含**进来：一份源码、零复制、零漂移，
// 正常构建（`build`/`check`/发布）完全不参与——core 的生产依赖图里没有任何插件。
// 由此产生一条硬纪律（与 `plugins/storage-sqlite` 相同）：被包含的 `workflow.rs`
// 及其测试只允许 `tianyan::…` 绝对路径，不得出现 `crate::…`（那时 `crate::` 会解析到
// core 而非插件，编译错误会指向完全不相干的位置）。
//
// 判别力的另一半在插件侧：`plugins/policy-approval/src/approval_tests.rs`（23 个用例）
// 在插件 crate 里照常跑一遍（`cargo test -p tianyan-policy-approval`）。
#[cfg(test)]
#[path = "../../../../plugins/policy-approval/src/workflow.rs"]
mod workflow;

#[cfg(test)]
pub use workflow::{
    build_approval_pending_text, ApprovalPendingNotifier, ApprovalWorkflow, SessionApprovalNotifier,
};

/// 测试模块（拆分至独立文件，保持主模块聚焦生产逻辑）。
#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
