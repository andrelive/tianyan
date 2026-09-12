//! 审批领域数据类型。
//!
//! 风险等级、审批请求/响应/记录、自动审批规则与工作流配置。
//! 工作流逻辑见 [`super::workflow`]。

use serde::{Deserialize, Serialize};

use crate::config::ApprovalMode;
use crate::executor::Action;

/// 判断用户对审批追问的回答是否为"允许执行"语义。
///
/// 启发式匹配：出现肯定词（允许/可以/同意/好/是/确认/继续/yes/ok）且
/// 未出现否定词（不/别/拒绝/否/禁止/取消）时视为批准。
///
/// 审批决策语义归属本模块，Agent 协调层通过此函数解析用户回答，
/// 不自行实现关键词启发式。
pub fn is_user_confirmation(answer: &str) -> bool {
    let lower = answer.trim().to_lowercase();
    let affirmatives = [
        "允许", "可以", "同意", "好", "是", "确认", "继续", "执行", "yes", "ok", "y",
    ];
    let negatives = ["不", "别", "拒绝", "否", "禁止", "取消", "no", "n", "停止"];

    let has_affirmative = affirmatives.iter().any(|w| lower.contains(w));
    let has_negative = negatives.iter().any(|w| lower.contains(w));
    has_affirmative && !has_negative
}

/// 操作风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(i32)]
pub enum RiskLevel {
    /// 安全操作，无需审批。
    Safe = 0,
    /// 低风险，可自动审批。
    Low = 1,
    /// 中风险，建议审批。
    Medium = 2,
    /// 高风险，必须审批。
    High = 3,
    /// 危险操作，默认拒绝。
    Critical = 4,
}

impl RiskLevel {
    /// 是否默认拒绝。
    pub fn default_deny(&self) -> bool {
        matches!(self, Self::Critical)
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Safe => write!(f, "safe"),
            RiskLevel::Low => write!(f, "low"),
            RiskLevel::Medium => write!(f, "medium"),
            RiskLevel::High => write!(f, "high"),
            RiskLevel::Critical => write!(f, "critical"),
        }
    }
}

/// 审批决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// 批准执行。
    Approve,
    /// 拒绝执行。
    Deny,
    /// 要求更多信息。
    RequestMoreInfo,
}

/// 审批请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// 请求 ID。
    pub request_id: String,
    /// 会话 ID。
    pub session_id: String,
    /// 操作描述。
    pub action_description: String,
    /// 具体操作。
    pub action: Action,
    /// 风险等级。
    pub risk_level: RiskLevel,
    /// 请求时间。
    pub requested_at: chrono::DateTime<chrono::Utc>,
    /// 超时时间（秒）。
    pub timeout_secs: u64,
}

/// 审批响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    /// 请求 ID。
    pub request_id: String,
    /// 审批决策。
    pub decision: ApprovalDecision,
    /// 审批理由。
    pub reason: Option<String>,
    /// 审批时间。
    pub responded_at: chrono::DateTime<chrono::Utc>,
    /// 审批者（用户 ID 或 "auto"）。
    pub approved_by: String,
    /// 用户审批时编辑后的命令（纠正/改写场景；仅在 decision=Approve
    /// 且用户在审批面板提供编辑值时存在）。仅用于审计与通知——
    /// 实际执行仍由 agent 按原命令发起。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_command: Option<String>,
}

/// 审批记录（用于审计）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// 请求。
    pub request: ApprovalRequest,
    /// 响应。
    pub response: Option<ApprovalResponse>,
    /// 最终执行结果（成功/失败）。
    pub execution_result: Option<bool>,
    /// 用户审批时编辑后的命令（纠正/改写场景；仅审计展示，
    /// 不影响实际执行——执行链仍按原命令发起）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_command: Option<String>,
}

/// 自动审批规则。
#[derive(Debug, Clone, Serialize)]
pub struct AutoApprovalRule {
    /// 规则名称。
    pub name: String,
    /// 匹配的操作类型。
    pub action_pattern: ActionPattern,
    /// 匹配条件。
    pub condition: ApprovalCondition,
    /// 决策。
    pub decision: ApprovalDecision,
    /// 是否启用。
    pub enabled: bool,
}

/// 操作模式匹配。
#[derive(Debug, Clone, Serialize)]
pub enum ActionPattern {
    /// 匹配任何操作。
    Any,
    /// 匹配命令执行（支持通配符）。
    CommandPattern(String),
    /// 匹配文件写入（支持路径前缀）。
    WriteFilePattern(String),
    /// 匹配特定技能。
    SkillPattern(String),
}

/// 审批条件。
#[derive(Debug, Clone, Serialize)]
pub enum ApprovalCondition {
    /// 无条件匹配。
    Always,
    /// 命令参数匹配正则（pattern 字符串；匹配逻辑当前未启用，保留扩展位）。
    ArgsMatch(String),
    /// 路径匹配。
    PathMatch(String),
    /// 风险等级低于阈值。
    RiskBelow(RiskLevel),
    /// 组合条件（全部满足）。
    All(Vec<ApprovalCondition>),
    /// 组合条件（任一满足）。
    Any(Vec<ApprovalCondition>),
}

/// 审批工作流配置。
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalWorkflowConfig {
    /// 默认超时时间（秒）。
    pub default_timeout_secs: u64,
    /// 是否启用自动审批。
    pub enable_auto_approval: bool,
    /// 自动审批规则。
    pub auto_approval_rules: Vec<AutoApprovalRule>,
    /// 审批行为模式（ADR-033：审批层单一事实源；默认全自主）。
    pub mode: ApprovalMode,
    /// 是否持久化审批记录。
    pub persist_records: bool,
    /// 最大待处理审批数。
    pub max_pending_approvals: usize,
    /// "总是询问"命令列表：命中的命令强制走人工审批/询问，
    /// 不被自动审批规则与任何模式的自动放行覆盖。
    /// 默认空（不强制）。
    pub prompt_commands: Vec<String>,
}

impl Default for ApprovalWorkflowConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 300,
            enable_auto_approval: true,
            auto_approval_rules: vec![
                // 默认规则：安全操作自动通过
                AutoApprovalRule {
                    name: "safe_operations".to_string(),
                    action_pattern: ActionPattern::Any,
                    condition: ApprovalCondition::RiskBelow(RiskLevel::Medium),
                    decision: ApprovalDecision::Approve,
                    enabled: true,
                },
            ],
            // ADR-033：默认全自主（黑名单外全放行）。
            mode: ApprovalMode::default(),
            persist_records: true,
            max_pending_approvals: 100,
            // 默认不强制任何命令走人工审批
            prompt_commands: Vec::new(),
        }
    }
}
/// 审批状态快照（供状态查询接口使用）。
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalStatusSnapshot {
    /// 审批工作流配置。
    pub config: ApprovalWorkflowConfig,
    /// 待人工审批的请求（`wait_for_approval` 通道，当前默认未接入）。
    pub pending_approvals: Vec<ApprovalRequest>,
    /// 待用户确认的操作指纹（"询问用户"降级链路，agent loop 追问中）。
    pub pending_confirmations: Vec<String>,
    /// 最近审批记录（审计，倒序 50 条）。
    pub recent_records: Vec<ApprovalRecord>,
    /// 用户已确认的操作指纹数量。
    pub confirmed_action_count: usize,
}
