//! 审批门控契约（ADR-049 期 3-T1）。
//!
//! 内核只认本 trait，不认识任何具体审批实现（实现已出 core，见
//! `plugins/policy-approval`）。四个方法**逐个都有调用点证据**，没有"以后可能要用"
//! 的方法——每期净增 pub trait ≤ 3 的反指标下，本批净增 **1 个** trait。
//!
//! ## 方法 → 调用点（证据）
//!
//! | 方法 | 内核侧调用点 | 语义 |
//! |---|---|---|
//! | [`ApprovalGate::authorize`] | `agent/tool_context.rs:150` `ensure_approved`（其内部 `:162`/`:164` 的两个入口合一）——`write_file` / `apply_edit` / `apply_patch` / `execute_command` 四个危险工具域都经此门控 | 给出审批决策（同一实现同时服务主循环「可等待人工」与子任务「永不等待」两条路径） |
//! | [`ApprovalGate::status`] | `agent/coordinator.rs:474` `AgentCoordinator::approval_status` | 状态快照（配置 + 待处理 + 最近审计 + 已确认指纹数）；`pending_confirmations` 是核心侧状态，由协调器覆写 |
//! | [`ApprovalGate::respond_to_approval`] | `agent/coordinator.rs:502` `AgentCoordinator::respond_approval`（GUI 审批面板） | 把人工决策送进挂起通道 |
//! | [`ApprovalGate::record_user_confirmation_by_fingerprint`] | `agent/tool_registry/mod.rs:552` `confirm_pending_approval`（"询问用户"降级链路，用户答"允许"后记指纹） | 记一个**已确认**的操作指纹 |
//!
//! ## 为什么方法不叫 `ensure_approved`
//!
//! `ToolContext::ensure_approved` 除了"问审批"，还做两件**核心侧**的事：拒绝时把操作
//! 指纹写进 `ToolContext::pending_approval_fingerprints`（`Agent` 轮末读它生成追问）、
//! 并把拒绝翻成统一错误文案。那份状态与轮末追问都在 core，搬进插件要么把 core 状态
//! 传进插件、要么在插件里复制一份"待确认队列"（两个写者）。所以契约切在**真正跨
//! crate 的那条线**上：`authorize` 只回答"准不准"，拒绝后的降级动作留在 core。
//!
//! ## 与 `ApprovalWorkflow`（插件实现）的关系
//!
//! 实现是 `plugins/policy-approval` 的 `ApprovalWorkflow`；装配层经
//! `ApprovalGateFactory` 注入（`server/src/assembly.rs` 第五插槽）。未装配时
//! `ToolContext.approval_gate` 为 `None`，`ensure_approved` 走既有的"未装配直接放行"
//! 分支——降级语义见 `plugins/policy-approval/src/lib.rs` 的 crate 文档。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::executor::approval::{ApprovalDecision, ApprovalResponse, ApprovalStatusSnapshot};
use crate::executor::Action;

/// 审批门控契约：危险操作执行前的审批判定 + 状态查询 + 人工响应 + 指纹确认。
///
/// 实现者必须保持审批决策的**短路顺序**（这是 `ApprovalMode` 三态语义的载体，
/// 与 ADR-033 绑定）：always-ask 强制 → 自动规则（Deny 优先）→
/// `Autonomous` 全放行 → `Safe` 放行 → `Critical` 默认拒绝 → 已确认指纹放行 →
/// `Interactive` 挂起等人工 → 兜底拒绝。
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// 请求审批（唯一的判定入口）。
    ///
    /// - `session_id` — 会话 ID（审计与挂起通知携带）
    /// - `action` — 待执行操作（风险定级与规则匹配的输入）
    /// - `allow_human_wait` — 是否允许挂起等待人工响应。`false` 是**子任务路径**
    ///   （ADR-011：子 agent 是主 agent 意图的执行器，任务下发即授权边界）——
    ///   未授权的新危险操作立即拒绝、不挂起、不入待处理队列，拒绝原因携带
    ///   "需要主任务授权"标记，由子 agent 上报主 agent 在主对话中确认。
    async fn authorize(
        &self,
        session_id: &str,
        action: &Action,
        allow_human_wait: bool,
    ) -> Result<ApprovalResponse>;

    /// 审批状态快照（配置 / 待处理请求 / 最近审计记录 / 已确认指纹数）。
    ///
    /// `pending_confirmations`（待用户确认的指纹，属核心侧 `Agent` 状态）
    /// 由调用方覆写——实现不拥有它。
    async fn status(&self) -> ApprovalStatusSnapshot;

    /// 响应一个待处理审批请求（GUI 审批面板）。
    ///
    /// 请求不存在或已超时返回 `not_found` 语义错误（调用方以 `is_not_found` 映射 404）。
    async fn respond_to_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
        reason: Option<String>,
        approved_by: &str,
        edited_command: Option<String>,
    ) -> Result<()>;

    /// 记录用户对某个操作指纹的确认（"询问用户"降级链路批准后调用）。
    ///
    /// 指纹由 [`crate::executor::approval::action_fingerprint`] 生成——生成规则
    /// 留 core，避免"待确认队列"（core）与"已确认集合"（实现）两侧算法漂移。
    async fn record_user_confirmation_by_fingerprint(&self, fingerprint: &str);
}
