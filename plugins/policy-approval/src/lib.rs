//! policy-approval 插件：工具执行审批工作流（审批**策略**内核外的实现）。
//!
//! ## 为什么独立成 crate（ADR-049 期 3-T1）
//!
//! `core/src/executor/approval/workflow.rs`（725 生产行）是**审批策略**：风险定级口径、
//! 自动审批规则引擎（Deny 优先）、`ApprovalMode` 三态短路顺序、「总是询问」命令、
//! 人工审批挂起通道、审计记录。改这些东西的原因与改内核机制的原因完全不同
//! （ADR-049 D7：按「变化原因」切）——调一条黑名单、加一条自动放行规则、改超时默认值、
//! 换风险定级口径，都不该重编 `tianyan-core`（42.4K 生产行）与 `tianyan-server`。
//!
//! 搬走之后，这些改动只重编本 crate + 装配包（`app` / `tauri`）；内核与通道层不动。
//!
//! ## 为什么 `types` 留 core（**不是**遗漏）
//!
//! `executor/approval/types.rs`（`RiskLevel` / `ApprovalDecision` / `ApprovalRequest` /
//! `ApprovalResponse` / `ApprovalStatusSnapshot` / `ApprovalWorkflowConfig` …
//! 以及纯函数 `is_user_confirmation`）**必须留在 core**，因为它们是
//! **A 级 trait `AgentCoordinator` 的签名的一部分**：
//!
//! - `coordinator.rs:169` `async fn approval_status(&self) -> Result<ApprovalStatusSnapshot>`
//! - `coordinator.rs:180` `async fn respond_approval(&self, …, decision: ApprovalDecision, …)`
//!
//! 把签名里的类型搬进插件，core 就不得不 `use tianyan_policy_approval::…`
//! ——**内核反向依赖插件**，破 D2-1/D2-3（这是本仓反复撞到的同一条边界：
//! 契约留内核、实现进插件，见 `vfs::StorageBackend` / `vfs::VectorStorage` 先例）。
//! 纯函数 `is_user_confirmation` 同理留 core：`tool_registry/agent_ops.rs:382`
//! （用户回答解析）在用它，且它零 I/O、零状态。
//!
//! ## trait 最小化：内核只认 `ApprovalGate`
//!
//! core 新增**一个** trait `tianyan::executor::approval::ApprovalGate`（4 个方法，
//! 每个都有调用点证据，见该 trait 的文档）。内核侧的审批入口因此从
//! 「持具体类型 `Arc<ApprovalWorkflow>`」变成「持 `Arc<dyn ApprovalGate>`」：
//! `ToolContext.approval_gate` / `Agent.approval_gate` / `AgentBuilder::with_approval_gate`。
//!
//! 本 crate 导出 [`gate`] 作为**装配期唯一入口**（把 `SecurityConfig` 翻成
//! `ApprovalWorkflowConfig` 并接上挂起通知器的逻辑，从 `AgentBuilder::build` 搬到这里），
//! 装配层（`app` bin / `tauri` / 集成测试）只做
//! `Arc::new(|sec, ws, sink| tianyan_policy_approval::gate(sec, ws, sink))` 的类型擦除包装
//! ——「配置 → 规则」的映射保持**单一事实源**在本插件内，三处装配不必各抄一份。
//!
//! ## 降级语义（未装配插槽时）
//!
//! 装配层未注入 `ApprovalGateFactory` 时，`AgentBuilder` 得到 `None`：**不装配审批门控**，
//! `ToolContext::ensure_approved` 走既有的「未装配直接放行」分支（该分支本来就在），
//! 工具照常执行，启动期 `warn!` 一条指路。
//!
//! 为什么允许降级而不是 `Err` / 硬前置：
//!
//! 1. **后果比存储降级轻一个量级**：存储缺失是「内容不落盘、重启即丢数据」（序 4 用
//!    `error!`）；审批缺失只是「危险操作不再被策略层拦一道」。而**自主模式本来就是
//!    默认**（ADR-033：`ApprovalMode::default()` = `Autonomous`，黑名单外全放行），
//!    所以「没有审批门控」与「自主模式下有审批门控」在默认配置下的可观测差异仅为
//!    「黑名单 Deny 少了一层复核 + 审计记录不落」，命令黑名单在
//!    `SecurityPolicy`（工具执行前的守卫层）里**另有**一份拦截。
//! 2. **旧入口不能坏**：`create_app`（构造对象图，测试/嵌入式）不要求装配，
//!    47 处 lib 单测靠它起实例；把它变成 `Err` 等于删入口。
//!
//! 可见性纪律（ADR-038）：`warn!` 必须在装配期留下证据——静默降级的唯一症状会是
//! 「审批面板永远是空的」+「黑名单命令偶尔被放行」，看不出是降级还是没触发。
//!
//! ## 测试
//!
//! `approval_tests.rs`（23 个用例）随实现整体迁入，逐个同名同断言（含「Autonomous
//! 短路」「Interactive 挂起」「Critical 拒绝」「always-ask 强制」「Deny 优先」
//! 「编辑命令审计」等判别力断言）。core 侧只剩 1 个用例（`test_is_user_confirmation`，
//! 它测的是 core 的纯函数）。
//!
//! **源码纪律**：本 crate 的 `workflow.rs` 只允许 `tianyan::…` 绝对路径，
//! 不得出现 `crate::…`——core 的单元测试在 `#[cfg(test)]` 下用 `#[path]` **同源包含**
//! 本文件（`core/src/executor/approval/mod.rs`），那时 `crate::` 会解析到 core 而非
//! 本 crate，编译错误会指向完全不相干的位置。理由与 `plugins/storage-sqlite` 相同：
//! core 的 `[dev-dependencies]` 指向本 crate 行不通（core 被编译两份，`ApprovalGate`
//! 与 `ApprovalWorkflowConfig` 会出现两个互不兼容的实例），而 core 的 3 个用例
//! （`loop_tests::test_run_approval_denied_returns_tool_error`、
//! `agent_ops_tests` 的两条子任务审批用例）必须用**真实**实现（Confirm / Interactive
//! 模式的短路语义正是判别力所在，替身会把它抹掉）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod workflow;

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;

use std::sync::Arc;

pub use workflow::{
    build_approval_pending_text, ApprovalPendingNotifier, ApprovalWorkflow, SessionApprovalNotifier,
};

use tianyan::agent::working_set::WorkingSetRegistry;
use tianyan::config::SecurityConfig;
use tianyan::executor::approval::{
    ActionPattern, ApprovalCondition, ApprovalDecision, ApprovalGate, ApprovalWorkflowConfig,
    AutoApprovalRule,
};
use tianyan::notification::SharedNotificationSink;

/// 构造审批门控实例（**装配期唯一入口**）。
///
/// 把 `SecurityConfig` 的审批面翻成工作流配置，并接上「审批挂起通知器」：
///
/// - `security.approval_mode` → `ApprovalWorkflowConfig.mode`（ADR-033 审批层单一事实源；
///   默认 `Autonomous`）；
/// - `security.prompt_commands` → 「总是询问」列表（命中的命令强制走人工，
///   不被任何自动放行路径覆盖）；
/// - `security.blocked_commands` → **Deny** 自动规则（评估时 Deny 优先，
///   无视 Approve 规则的配置顺序）；
/// - `security.allowed_commands` → **Approve** 自动规则（自动放行）；
/// - 其余字段沿用 `ApprovalWorkflowConfig::default()`（`default_timeout_secs = 300`、
///   `enable_auto_approval = true`、`persist_records = true`、`max_pending_approvals = 100`、
///   默认规则 `safe_operations`，**与迁移前 `AgentBuilder::build` 逐字一致**）。
///
/// 通知器（[`SessionApprovalNotifier`]）把挂起请求经 ADR-039 的唯一写入口
/// （`working_sets`）落成会话 System 消息，并（可选）发桌面通知——因此装配层必须递
/// **与 Agent 同一个** `WorkingSetRegistry` 实例，否则审批提示会写进另一份状态。
///
/// 这三者的来源决定了本函数的签名：`SecurityConfig`（规则）、
/// `WorkingSetRegistry`（落库）、`SharedNotificationSink`（桌面通知）。
pub fn gate(
    security: &SecurityConfig,
    working_sets: Arc<WorkingSetRegistry>,
    notification: SharedNotificationSink,
) -> Arc<dyn ApprovalGate> {
    let mut approval_config = ApprovalWorkflowConfig {
        // ADR-033：审批行为模式（默认全自主；黑名单 Deny 仍最优先）。
        mode: security.approval_mode,
        ..Default::default()
    };
    approval_config.prompt_commands = security.prompt_commands.clone();
    for cmd in &security.blocked_commands {
        approval_config.auto_approval_rules.push(AutoApprovalRule {
            name: format!("security_blocked_{cmd}"),
            action_pattern: ActionPattern::CommandPattern(cmd.clone()),
            condition: ApprovalCondition::Always,
            decision: ApprovalDecision::Deny,
            enabled: true,
        });
    }
    for cmd in &security.allowed_commands {
        approval_config.auto_approval_rules.push(AutoApprovalRule {
            name: format!("security_allowed_{cmd}"),
            action_pattern: ActionPattern::CommandPattern(cmd.clone()),
            condition: ApprovalCondition::Always,
            decision: ApprovalDecision::Approve,
            enabled: true,
        });
    }
    let notifier = SessionApprovalNotifier::new(working_sets).with_notification_sink(notification);
    Arc::new(ApprovalWorkflow::new(approval_config).with_pending_notifier(Arc::new(notifier)))
}
