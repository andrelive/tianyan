//! 审批工作流管理器。
//!
//! 状态机（待审批/已确认）+ 自动规则匹配 + 人工审批通道 + 审计记录。
//! 数据类型定义见 [`super::types`]。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, RwLock};

use super::types::*;
use crate::executor::Action;

/// 审批工作流管理器。
///
/// 管理工具执行的审批流程，支持自动审批规则和人工审批。
pub struct ApprovalWorkflow {
    config: ApprovalWorkflowConfig,
    pending_approvals: Arc<RwLock<HashMap<String, PendingApproval>>>,
    records: Arc<RwLock<Vec<ApprovalRecord>>>,
    /// 用户已确认的操作指纹集合（经"询问用户"降级链路获得批准后记录）。
    confirmed_actions: Arc<RwLock<HashSet<String>>>,
}

/// 待处理审批。
struct PendingApproval {
    request: ApprovalRequest,
    response_tx: oneshot::Sender<ApprovalResponse>,
    _created_at: Instant,
}

impl ApprovalWorkflow {
    /// 创建新的审批工作流。
    pub fn new(config: ApprovalWorkflowConfig) -> Self {
        Self {
            config,
            pending_approvals: Arc::new(RwLock::new(HashMap::new())),
            records: Arc::new(RwLock::new(Vec::new())),
            confirmed_actions: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// 获取工作流配置（快照拷贝）。
    pub fn config(&self) -> ApprovalWorkflowConfig {
        self.config.clone()
    }

    /// 获取用户已确认的操作指纹数量。
    pub async fn confirmed_action_count(&self) -> usize {
        self.confirmed_actions.read().await.len()
    }

    /// 计算操作的稳定指纹（用于"用户已确认"的去重判定）。
    pub fn action_fingerprint(action: &Action) -> String {
        serde_json::to_string(action).unwrap_or_else(|e| format!("{:?}-{e}", action))
    }

    /// 记录用户对该操作的确认（来自"询问用户"降级链路）。
    pub async fn record_user_confirmation(&self, action: &Action) {
        let fp = Self::action_fingerprint(action);
        self.confirmed_actions.write().await.insert(fp);
        tracing::info!(action = ?action, "用户已确认执行该操作");
    }

    /// 记录用户对指定指纹操作的确认（指纹由 [`Self::action_fingerprint`] 生成）。
    pub async fn record_user_confirmation_by_fingerprint(&self, fingerprint: &str) {
        self.confirmed_actions
            .write()
            .await
            .insert(fingerprint.to_string());
    }

    /// 检查操作是否已被用户确认。
    pub async fn is_action_confirmed(&self, action: &Action) -> bool {
        let fp = Self::action_fingerprint(action);
        self.confirmed_actions.read().await.contains(&fp)
    }

    /// 评估操作的风险等级。
    pub fn assess_risk(&self, action: &Action) -> RiskLevel {
        match action {
            Action::ReadFile { .. } => RiskLevel::Safe,
            Action::SearchCode { .. } => RiskLevel::Safe,
            Action::WriteFile { path, .. } => {
                let critical_paths = [
                    "/etc/",
                    "/usr/",
                    "/bin/",
                    "/sbin/",
                    "C:\\Windows",
                    "C:\\Program Files",
                    ".env",
                    "config.yaml",
                    "Cargo.toml",
                ];
                if critical_paths.iter().any(|p| path.contains(p)) {
                    RiskLevel::High
                } else if path.contains("test") || path.contains("temp") || path.contains("tmp") {
                    RiskLevel::Low
                } else {
                    RiskLevel::Medium
                }
            }
            Action::ExecuteCommand { command, .. } => {
                let cmd = command.split_whitespace().next().unwrap_or(command);
                let dangerous_cmds = ["rm", "del", "format", "fdisk", "mkfs", "dd"];
                let moderate_cmds = ["git", "cargo", "npm", "pip", "docker"];

                if dangerous_cmds.iter().any(|c| cmd.contains(c)) {
                    RiskLevel::Critical
                } else if moderate_cmds.iter().any(|c| cmd.contains(c)) {
                    RiskLevel::Medium
                } else {
                    RiskLevel::Low
                }
            }
            Action::CallSkill { skill_id, .. } => {
                let dangerous_skills = ["shell", "exec", "delete", "remove"];
                if dangerous_skills
                    .iter()
                    .any(|s| skill_id.to_lowercase().contains(s))
                {
                    RiskLevel::High
                } else {
                    RiskLevel::Low
                }
            }
            Action::ApplyEdit { .. } => RiskLevel::Medium,
            Action::ApplyPatch { .. } => RiskLevel::Medium,
            // 测试/构建命令经 cmd /C、sh -c 执行任意 shell 命令（与
            // ExecuteCommand 同级），属 Medium 风险，需用户确认。
            Action::RunTests { .. } => RiskLevel::Medium,
            Action::VerifyBuild { .. } => RiskLevel::Medium,
        }
    }

    /// 请求审批。
    ///
    /// - `session_id` - 会话 ID
    /// - `action` - 要执行的操作
    /// - returns: 审批响应（异步等待）
    pub async fn request_approval(
        &self,
        session_id: &str,
        action: &Action,
    ) -> crate::common::error::Result<ApprovalResponse> {
        let risk_level = self.assess_risk(action);

        // 检查自动审批规则
        if self.config.enable_auto_approval {
            if let Some(decision) = self.check_auto_approval(action, risk_level) {
                tracing::info!(
                    action = ?action,
                    decision = ?decision,
                    "自动审批决策"
                );
                return Ok(ApprovalResponse {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    decision,
                    reason: Some("自动审批规则匹配".to_string()),
                    responded_at: chrono::Utc::now(),
                    approved_by: "auto".to_string(),
                });
            }
        }

        // 如果风险等级为 Safe，直接通过
        if risk_level == RiskLevel::Safe {
            return Ok(ApprovalResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                decision: ApprovalDecision::Approve,
                reason: Some("安全操作，无需审批".to_string()),
                responded_at: chrono::Utc::now(),
                approved_by: "system".to_string(),
            });
        }

        // 如果风险等级为 Critical，默认拒绝
        if risk_level.default_deny() {
            return Ok(ApprovalResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                decision: ApprovalDecision::Deny,
                reason: Some(format!("危险操作（{}），默认拒绝", risk_level)),
                responded_at: chrono::Utc::now(),
                approved_by: "system".to_string(),
            });
        }

        // 无人值守模式：Medium/High 风险操作自动批准并记录审计。
        if self.config.unattended_mode {
            let request = ApprovalRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.to_string(),
                action_description: format!("{:?}", action),
                action: action.clone(),
                risk_level,
                requested_at: chrono::Utc::now(),
                timeout_secs: self.config.default_timeout_secs,
            };
            let response = ApprovalResponse {
                request_id: request.request_id.clone(),
                decision: ApprovalDecision::Approve,
                reason: Some(format!("无人值守模式：{} 风险自动批准", risk_level)),
                responded_at: chrono::Utc::now(),
                approved_by: "auto".to_string(),
            };
            tracing::info!(
                session_id = %session_id,
                action = ?action,
                risk_level = %risk_level,
                decision = ?response.decision,
                "无人值守模式自动审批（审计）"
            );
            self.record_approval_audit(request, Some(response.clone()))
                .await;
            return Ok(response);
        }

        // attended 模式（默认）：检查用户是否已通过"询问用户"链路确认过该操作
        if self.is_action_confirmed(action).await {
            return Ok(ApprovalResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                decision: ApprovalDecision::Approve,
                reason: Some("用户已确认执行该操作".to_string()),
                responded_at: chrono::Utc::now(),
                approved_by: "user".to_string(),
            });
        }

        // 审批通道已接入时：等待人工审批响应（带超时）
        if self.config.wait_for_approval {
            return self
                .wait_for_human_approval(session_id, action, risk_level)
                .await;
        }

        // 无审批通道：立即拒绝，由上层（agent loop）降级为"询问用户"追问，
        // 用户确认后重试。降级判断基于 ToolRegistry 的待确认指纹队列，
        // 与 reason 文案无关。
        Ok(ApprovalResponse {
            request_id: uuid::Uuid::new_v4().to_string(),
            decision: ApprovalDecision::Deny,
            reason: Some(format!("需要用户确认：{:?}", action)),
            responded_at: chrono::Utc::now(),
            approved_by: "system".to_string(),
        })
    }

    /// 等待人工审批响应（GUI 审批通道接入后使用），超时默认拒绝。
    async fn wait_for_human_approval(
        &self,
        session_id: &str,
        action: &Action,
        risk_level: RiskLevel,
    ) -> crate::common::error::Result<ApprovalResponse> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        let request = ApprovalRequest {
            request_id: request_id.clone(),
            session_id: session_id.to_string(),
            action_description: format!("{:?}", action),
            action: action.clone(),
            risk_level,
            requested_at: chrono::Utc::now(),
            timeout_secs: self.config.default_timeout_secs,
        };

        {
            let mut pending = self.pending_approvals.write().await;
            if pending.len() >= self.config.max_pending_approvals {
                return Err(crate::common::error::TianyanError::Custom(
                    "内部错误：待处理审批数量超过上限".to_string(),
                ));
            }
            pending.insert(
                request_id.clone(),
                PendingApproval {
                    request: request.clone(),
                    response_tx: tx,
                    _created_at: Instant::now(),
                },
            );
        }

        // 等待审批响应（带超时）
        let timeout = Duration::from_secs(self.config.default_timeout_secs);
        let response = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) | Err(_) => {
                // 超时或通道关闭，默认拒绝
                ApprovalResponse {
                    request_id: request_id.clone(),
                    decision: ApprovalDecision::Deny,
                    reason: Some("审批超时，默认拒绝".to_string()),
                    responded_at: chrono::Utc::now(),
                    approved_by: "system".to_string(),
                }
            }
        };

        // 清理待处理列表
        {
            let mut pending = self.pending_approvals.write().await;
            pending.remove(&request_id);
        }

        // 记录审批历史
        self.record_approval_audit(request, Some(response.clone()))
            .await;

        Ok(response)
    }

    /// 记录审批审计（仅当 `persist_records` 开启时）。
    async fn record_approval_audit(
        &self,
        request: ApprovalRequest,
        response: Option<ApprovalResponse>,
    ) {
        if !self.config.persist_records {
            return;
        }
        let record = ApprovalRecord {
            request,
            response,
            execution_result: None,
        };
        let mut records = self.records.write().await;
        records.push(record);
    }

    /// 响应审批请求。
    ///
    /// - `request_id` - 请求 ID
    /// - `decision` - 审批决策
    /// - `reason` - 审批理由
    /// - `approved_by` - 审批者
    pub async fn respond_to_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
        reason: Option<String>,
        approved_by: &str,
    ) -> crate::common::error::Result<()> {
        let mut pending = self.pending_approvals.write().await;

        if let Some(pending_approval) = pending.remove(request_id) {
            let response = ApprovalResponse {
                request_id: request_id.to_string(),
                decision,
                reason,
                responded_at: chrono::Utc::now(),
                approved_by: approved_by.to_string(),
            };

            if let Err(e) = pending_approval.response_tx.send(response) {
                tracing::warn!(error = ?e, "发送审批响应失败");
            }
            Ok(())
        } else {
            Err(crate::common::error::TianyanError::Custom(
                "内部错误：审批请求不存在或已超时".to_string(),
            ))
        }
    }

    /// 检查自动审批规则。
    ///
    /// `pub(super)`：审批模块内部（含模块测试）可访问，不对外暴露。
    pub(super) fn check_auto_approval(
        &self,
        action: &Action,
        risk_level: RiskLevel,
    ) -> Option<ApprovalDecision> {
        for rule in &self.config.auto_approval_rules {
            if !rule.enabled {
                continue;
            }

            if self.matches_pattern(action, &rule.action_pattern)
                && self.matches_condition(action, risk_level, &rule.condition)
            {
                return Some(rule.decision);
            }
        }
        None
    }

    /// 检查操作是否匹配模式。
    fn matches_pattern(&self, action: &Action, pattern: &ActionPattern) -> bool {
        match (action, pattern) {
            (_, ActionPattern::Any) => true,
            (Action::ExecuteCommand { command, .. }, ActionPattern::CommandPattern(pat)) => command
                .split_whitespace()
                .next()
                .map(|cmd| cmd == pat || cmd.ends_with(&format!(".{}", pat)))
                .unwrap_or(false),
            (Action::WriteFile { path, .. }, ActionPattern::WriteFilePattern(pat)) => {
                path.starts_with(pat)
            }
            (Action::CallSkill { skill_id, .. }, ActionPattern::SkillPattern(pat)) => {
                skill_id == pat
            }
            _ => false,
        }
    }

    /// 检查条件是否满足。
    fn matches_condition(
        &self,
        _action: &Action,
        risk_level: RiskLevel,
        condition: &ApprovalCondition,
    ) -> bool {
        match condition {
            ApprovalCondition::Always => true,
            ApprovalCondition::RiskBelow(threshold) => risk_level < *threshold,
            ApprovalCondition::All(conditions) => conditions
                .iter()
                .all(|c| self.matches_condition(_action, risk_level, c)),
            ApprovalCondition::Any(conditions) => conditions
                .iter()
                .any(|c| self.matches_condition(_action, risk_level, c)),
            _ => false,
        }
    }

    /// 获取待处理审批列表。
    pub async fn get_pending_approvals(&self) -> Vec<ApprovalRequest> {
        let pending = self.pending_approvals.read().await;
        pending.values().map(|p| p.request.clone()).collect()
    }

    /// 获取审批历史记录。
    pub async fn get_approval_records(&self) -> Vec<ApprovalRecord> {
        let records = self.records.read().await;
        records.clone()
    }
}
