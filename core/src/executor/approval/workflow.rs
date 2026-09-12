//! 审批工作流管理器。
//!
//! 状态机（待审批/已确认）+ 自动规则匹配 + 人工审批通道 + 审计记录。
//! 数据类型定义见 [`super::types`]。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, RwLock};

use super::types::*;
use crate::common::types::StructuredMessage;
use crate::config::ApprovalMode;
use crate::executor::Action;
use crate::notification::SharedNotificationSink;
use crate::session::SessionManager;

/// 审批挂起通知器。
///
/// 人工审批请求挂起等待时调用（GUI 审批面板通道），把请求告知用户——
/// 否则后台任务等无人值守场景的审批请求会静默挂起（直到超时拒绝）。
#[async_trait::async_trait]
pub trait ApprovalPendingNotifier: Send + Sync {
    /// 审批请求挂起等待人工响应时调用。
    async fn on_approval_pending(&self, request: &ApprovalRequest);
}

/// 默认通知器：把审批请求作为 System 消息持久化到所属会话。
///
/// 前台任务：用户正在对话，流式停止即感知，System 消息给出审批面板指引；
/// 后台任务：审批请求不再静默——用户回到会话即看到待处理请求，
/// 到审批面板批准/拒绝后任务经 oneshot 通道恢复继续。
/// 可选注入系统通知通道：挂起时同时发出桌面通知（无人值守场景即时提醒）。
pub struct SessionApprovalNotifier {
    session_manager: Arc<dyn SessionManager>,
    notification: Option<SharedNotificationSink>,
}

impl SessionApprovalNotifier {
    /// 创建通知器。
    pub fn new(session_manager: Arc<dyn SessionManager>) -> Self {
        Self {
            session_manager,
            notification: None,
        }
    }

    /// 设置系统通知通道（挂起时发出桌面通知；未注入时仅持久化会话消息）。
    pub fn with_notification_sink(mut self, sink: SharedNotificationSink) -> Self {
        self.notification = Some(sink);
        self
    }
}

/// 构建审批挂起通知文本。
pub fn build_approval_pending_text(request: &ApprovalRequest) -> String {
    format!(
        "[审批请求] 操作需要人工确认：{}（风险等级：{}，请求 ID：{}）。\n请到「审批」面板批准或拒绝；超时（{} 秒）将默认拒绝。",
        request.action_description, request.risk_level, request.request_id, request.timeout_secs
    )
}

#[async_trait::async_trait]
impl ApprovalPendingNotifier for SessionApprovalNotifier {
    async fn on_approval_pending(&self, request: &ApprovalRequest) {
        let sm = StructuredMessage::system(
            request.session_id.clone(),
            build_approval_pending_text(request),
        );
        if let Err(e) = self
            .session_manager
            .add_structured_message(&request.session_id, sm)
            .await
        {
            tracing::warn!(
                request_id = %request.request_id,
                error = %e,
                "审批挂起通知持久化失败"
            );
        }

        // 桌面系统通知：审批挂起时即时提醒用户（非阻塞；未注入时静默）
        if let Some(sink) = &self.notification {
            sink.notify("需要审批", &build_approval_pending_text(request));
        }
    }
}

/// 审批工作流管理器。
///
/// 管理工具执行的审批流程，支持自动审批规则和人工审批。
pub struct ApprovalWorkflow {
    config: ApprovalWorkflowConfig,
    pending_approvals: Arc<RwLock<HashMap<String, PendingApproval>>>,
    records: Arc<RwLock<Vec<ApprovalRecord>>>,
    /// 用户已确认的操作指纹集合（经"询问用户"降级链路获得批准后记录）。
    confirmed_actions: Arc<RwLock<HashSet<String>>>,
    /// 审批挂起通知器（GUI 审批通道挂起时告知用户）。
    pending_notifier: Option<Arc<dyn ApprovalPendingNotifier>>,
}

/// 待处理审批。
struct PendingApproval {
    request: ApprovalRequest,
    response_tx: oneshot::Sender<ApprovalResponse>,
}

/// 路径段集合（按 `/` 与 `\\` 切分；小写归一——Windows 文件系统大小写不敏感）。
fn path_segments(path: &str) -> Vec<String> {
    path.to_lowercase()
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 写文件是否命中关键路径（High 风险）：目录段精确匹配系统目录；
/// 文件名段命中环境/配置文件形态（`.env*`、`config.y*ml*`）。
///
/// 与此前的 `contains` 子串匹配的区别：按路径段匹配避免误判——
/// `myconfig.yaml.bak` 这类目录名/文件名前缀仍命中（保守方向），
/// 但 `/etc2/`、`C:\\WindowsBackup` 等子串巧合不再命中；
/// 大小写统一归一（`c:\\windows\\...` 同样命中）。
fn is_critical_write_path(path: &str) -> bool {
    let segments = path_segments(path);
    // 系统目录段：/etc/、/usr/、/bin/、/sbin/、C:\\Windows\\、C:\\Program Files\\
    if segments.iter().any(|s| {
        matches!(
            s.as_str(),
            "etc" | "usr" | "bin" | "sbin" | "windows" | "program files"
        )
    }) {
        return true;
    }
    // 文件名段：.env（含 .env.local 等变体）、config.y*ml*（yaml/yml 及 example 变体）
    if let Some(file) = segments.last() {
        if file.starts_with(".env") || file.starts_with("config.y") {
            return true;
        }
    }
    false
}

/// 写文件是否命中测试/临时目录语义（Low Risk）：路径段按非字母数字
/// 分词后含 test/tests/temp/tmp 词（如 `test.txt`、`tests/foo.rs`、
/// `tmp/x`、`unit_test.rs`）。
///
/// 与此前的 `contains("test")` 子串匹配的区别：`attestation.txt`、
/// `latest_report.md`（"latest" 含 "test"）、`attempt.log`（含 "tmp"）等
/// 非测试文件不再被误降级为 Low（绕过审批门控的安全漏洞方向）。
fn is_test_or_temp_path(path: &str) -> bool {
    path_segments(path).iter().any(|seg| {
        seg.split(|c: char| !c.is_alphanumeric())
            .any(|w| matches!(w, "test" | "tests" | "temp" | "tmp"))
    })
}

/// 技能名是否命中危险关键词（shell/exec/delete/remove）：按非字母数字
/// 分词后精确匹配——`executive_summary` 不再误命中 "exec"，
/// `delete_all_files` 仍命中 "delete"。
fn is_dangerous_skill(skill_id: &str) -> bool {
    let lower = skill_id.to_lowercase();
    ["shell", "exec", "delete", "remove"]
        .iter()
        .any(|k| lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == *k))
}

impl ApprovalWorkflow {
    /// 创建新的审批工作流。
    pub fn new(config: ApprovalWorkflowConfig) -> Self {
        Self {
            config,
            pending_approvals: Arc::new(RwLock::new(HashMap::new())),
            records: Arc::new(RwLock::new(Vec::new())),
            confirmed_actions: Arc::new(RwLock::new(HashSet::new())),
            pending_notifier: None,
        }
    }

    /// 设置审批挂起通知器（Agent 装配层注入）。
    pub fn with_pending_notifier(mut self, notifier: Arc<dyn ApprovalPendingNotifier>) -> Self {
        self.pending_notifier = Some(notifier);
        self
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
                if is_critical_write_path(path) {
                    RiskLevel::High
                } else if is_test_or_temp_path(path) {
                    RiskLevel::Low
                } else {
                    RiskLevel::Medium
                }
            }
            Action::ExecuteCommand { command, .. } => {
                // 命令风险定级单一事实源（security.rs classify_command_risk）：
                // Blocked/Dangerous → Critical（需用户确认），Moderate → Medium，Low → Low。
                // 命令名经 extract_command_base 归一（小写、无路径/后缀），
                // 与 check_command 同一匹配语义（此前为 contains 子串匹配，
                // 会误伤 rmdir→rm 等边缘命令名）。
                let cmd_name = crate::executor::command::extract_command_base(command);
                match crate::executor::security::classify_command_risk(&cmd_name) {
                    crate::executor::security::CommandRisk::Blocked
                    | crate::executor::security::CommandRisk::Dangerous => RiskLevel::Critical,
                    crate::executor::security::CommandRisk::Moderate => RiskLevel::Medium,
                    crate::executor::security::CommandRisk::Low => RiskLevel::Low,
                }
            }
            Action::CallSkill { skill_id, .. } => {
                if is_dangerous_skill(skill_id) {
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

    /// 请求审批（主循环路径：可按配置等待人工审批）。
    ///
    /// - `session_id` - 会话 ID
    /// - `action` - 要执行的操作
    /// - returns: 审批响应（异步等待）
    pub async fn request_approval(
        &self,
        session_id: &str,
        action: &Action,
    ) -> crate::common::error::Result<ApprovalResponse> {
        self.request_approval_internal(session_id, action, true)
            .await
    }

    /// 请求审批（子任务路径：**永不等待人工响应**）。
    ///
    /// 子 agent 是主 agent 意图的执行器，任务下发即授权边界——遇到未授权
    /// 的新危险操作时不交互、不挂起，立即拒绝；拒绝原因携带"需要主任务
    /// 授权"标记，由子 agent 上报主 agent，在主对话中向用户确认。
    pub async fn request_approval_no_wait(
        &self,
        session_id: &str,
        action: &Action,
    ) -> crate::common::error::Result<ApprovalResponse> {
        self.request_approval_internal(session_id, action, false)
            .await
    }

    async fn request_approval_internal(
        &self,
        session_id: &str,
        action: &Action,
        allow_human_wait: bool,
    ) -> crate::common::error::Result<ApprovalResponse> {
        let risk_level = self.assess_risk(action);

        // "总是询问"命令：强制走人工审批/询问，不被任何自动放行路径放行
        let forced_prompt = self.matches_prompt_command(action);

        // 检查自动审批规则（Deny 规则优先；总是询问命令跳过）
        if !forced_prompt && self.config.enable_auto_approval {
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
                    edited_command: None,
                });
            }
        }

        // 模式短路（ADR-033）：全自主——黑名单外全放行并记录审计
        // （总是询问命令除外；黑名单 Deny 已在上方规则评估中优先处理）。
        if !forced_prompt && self.config.mode == ApprovalMode::Autonomous {
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
                reason: Some("全自主模式：自动批准".to_string()),
                responded_at: chrono::Utc::now(),
                approved_by: "auto".to_string(),
                edited_command: None,
            };
            tracing::info!(
                session_id = %session_id,
                action = ?action,
                risk_level = %risk_level,
                decision = ?response.decision,
                "全自主模式自动审批（审计）"
            );
            self.record_approval_audit(request, Some(response.clone()))
                .await;
            return Ok(response);
        }

        // 如果风险等级为 Safe，直接通过（总是询问命令除外）
        if !forced_prompt && risk_level == RiskLevel::Safe {
            return Ok(ApprovalResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                decision: ApprovalDecision::Approve,
                reason: Some("安全操作，无需审批".to_string()),
                responded_at: chrono::Utc::now(),
                approved_by: "system".to_string(),
                edited_command: None,
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
                edited_command: None,
            });
        }

        // 确认/交互模式（ADR-033）：检查用户是否已通过"询问用户"链路确认过该操作
        if self.is_action_confirmed(action).await {
            return Ok(ApprovalResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                decision: ApprovalDecision::Approve,
                reason: Some("用户已确认执行该操作".to_string()),
                responded_at: chrono::Utc::now(),
                approved_by: "user".to_string(),
                edited_command: None,
            });
        }

        // 交互模式（ADR-033）且允许等待时：挂起等待人工审批响应（带超时）。
        // 总是询问命令在这里强制挂起等待人工响应。
        if self.config.mode == ApprovalMode::Interactive && allow_human_wait {
            return self
                .wait_for_human_approval(session_id, action, risk_level)
                .await;
        }

        // 子任务路径（不允许等待）：立即拒绝，由子 agent 上报主 agent，
        // 在主对话中向用户确认后重新委托。
        let reason = if !allow_human_wait {
            format!("子任务操作需要主任务授权确认：{:?}", action)
        } else {
            format!("需要用户确认：{:?}", action)
        };
        Ok(ApprovalResponse {
            request_id: uuid::Uuid::new_v4().to_string(),
            decision: ApprovalDecision::Deny,
            reason: Some(reason),
            responded_at: chrono::Utc::now(),
            approved_by: "system".to_string(),
            edited_command: None,
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
                },
            );
        }

        // 挂起通知：告知用户有审批请求待处理（后台任务不再静默等待）
        if let Some(notifier) = &self.pending_notifier {
            notifier.on_approval_pending(&request).await;
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
                    edited_command: None,
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
            response: response.clone(),
            execution_result: None,
            edited_command: response.as_ref().and_then(|r| r.edited_command.clone()),
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
    /// - `edited_command` - 用户编辑后的命令（纠正/改写场景；仅
    ///   decision=Approve 且提供时生效，记入审计，不影响实际执行）
    pub async fn respond_to_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
        reason: Option<String>,
        approved_by: &str,
        edited_command: Option<String>,
    ) -> crate::common::error::Result<()> {
        let mut pending = self.pending_approvals.write().await;

        if let Some(pending_approval) = pending.remove(request_id) {
            // 编辑命令仅对"批准"语义生效；空字符串视为未提供
            let edited = if decision == ApprovalDecision::Approve {
                edited_command.filter(|c| !c.trim().is_empty())
            } else {
                None
            };
            if let Some(cmd) = &edited {
                tracing::info!(
                    request_id = %request_id,
                    edited_command = %cmd,
                    "审批批准时收到用户编辑后的命令（仅审计/通知，实际执行仍按原命令）"
                );
            }
            let response = ApprovalResponse {
                request_id: request_id.to_string(),
                decision,
                reason,
                responded_at: chrono::Utc::now(),
                approved_by: approved_by.to_string(),
                edited_command: edited,
            };

            if let Err(e) = pending_approval.response_tx.send(response) {
                tracing::warn!(error = ?e, "发送审批响应失败");
            }
            Ok(())
        } else {
            Err(crate::common::error::TianyanError::not_found(
                "审批请求不存在或已超时",
            ))
        }
    }

    /// 检查自动审批规则。
    ///
    /// 评估顺序：**Deny 规则优先**——先匹配所有拒绝规则（强制拒绝，
    /// 无视 Approve 规则的配置顺序），再匹配放行规则（Approve /
    /// RequestMoreInfo），保证黑名单命令不被白名单规则覆盖。
    ///
    /// `pub(super)`：审批模块内部（含模块测试）可访问，不对外暴露。
    pub(super) fn check_auto_approval(
        &self,
        action: &Action,
        risk_level: RiskLevel,
    ) -> Option<ApprovalDecision> {
        // 第一遍：Deny 规则（拒绝优先）
        for rule in &self.config.auto_approval_rules {
            if !rule.enabled || rule.decision != ApprovalDecision::Deny {
                continue;
            }
            if self.matches_pattern(action, &rule.action_pattern)
                && self.matches_condition(action, risk_level, &rule.condition)
            {
                return Some(ApprovalDecision::Deny);
            }
        }
        // 第二遍：放行规则（Approve / RequestMoreInfo）
        for rule in &self.config.auto_approval_rules {
            if !rule.enabled || rule.decision == ApprovalDecision::Deny {
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

    /// 检查操作是否命中"总是询问"命令列表。
    ///
    /// 匹配语义与 [`Self::matches_pattern`] 的 `CommandPattern` 一致：
    /// 命令首词精确匹配或扩展名后缀匹配（如 `git.exe` 命中 `git`）。
    /// 命中的命令强制走人工审批/询问，不被自动放行。
    fn matches_prompt_command(&self, action: &Action) -> bool {
        if self.config.prompt_commands.is_empty() {
            return false;
        }
        let Action::ExecuteCommand { command, .. } = action else {
            return false;
        };
        let cmd = command.split_whitespace().next().unwrap_or(command);
        self.config
            .prompt_commands
            .iter()
            .any(|p| cmd == p || cmd.ends_with(&format!(".{}", p)))
    }

    /// 检查操作是否匹配模式。
    fn matches_pattern(&self, action: &Action, pattern: &ActionPattern) -> bool {
        match (action, pattern) {
            (_, ActionPattern::Any) => true,
            (Action::ExecuteCommand { command, .. }, ActionPattern::CommandPattern(pat)) => {
                // 多词模式（如 `rm -rf /`）按整条命令前缀匹配（大小写不敏感，
                // 兼容 Windows 命令），使全目录删除类黑名单生效；
                // 单词模式保持首词精确匹配（兼容既有行为）。
                if pat.contains(' ') {
                    command
                        .to_lowercase()
                        .trim_start()
                        .starts_with(&pat.to_lowercase())
                } else {
                    command
                        .split_whitespace()
                        .next()
                        .map(|cmd| cmd == pat || cmd.ends_with(&format!(".{}", pat)))
                        .unwrap_or(false)
                }
            }
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
