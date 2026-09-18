use std::sync::Arc;
use std::time::Duration;

use crate::config::ApprovalMode;
use crate::executor::Action;

use super::*;

#[test]
fn test_is_user_confirmation() {
    assert!(is_user_confirmation("允许"));
    assert!(is_user_confirmation("可以，继续吧"));
    assert!(is_user_confirmation("好的，同意执行"));
    assert!(is_user_confirmation("yes, go ahead"));
    assert!(!is_user_confirmation("不允许"));
    assert!(!is_user_confirmation("拒绝执行"));
    assert!(!is_user_confirmation("不可以"));
    assert!(!is_user_confirmation("no"));
    assert!(!is_user_confirmation(""));
    assert!(!is_user_confirmation("请不要执行"));
}

#[test]
fn test_risk_level_assessment() {
    let workflow = ApprovalWorkflow::new(ApprovalWorkflowConfig::default());

    let read_action = Action::ReadFile {
        path: "test.txt".to_string(),
    };
    assert_eq!(workflow.assess_risk(&read_action), RiskLevel::Safe);

    let write_action = Action::WriteFile {
        path: "test.txt".to_string(),
        content: "hello".to_string(),
    };
    assert_eq!(workflow.assess_risk(&write_action), RiskLevel::Low);

    let dangerous_write = Action::WriteFile {
        path: "/etc/passwd".to_string(),
        content: "hack".to_string(),
    };
    assert_eq!(workflow.assess_risk(&dangerous_write), RiskLevel::High);

    let dangerous_cmd = Action::ExecuteCommand {
        command: "rm -rf /".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    assert_eq!(workflow.assess_risk(&dangerous_cmd), RiskLevel::Critical);
}

#[test]
fn test_write_tool_risk_uses_same_caliber_as_write_file() {
    // T1-16：写类工具风险定级统一口径——此前 ApplyEdit/ApplyPatch 恒 Medium，
    // 同一次“改 `.env`”用 write_file 判 High、用 apply_edit/apply_patch 只判
    // Medium，风险定级可被工具选择绕过。
    let workflow = ApprovalWorkflow::new(ApprovalWorkflowConfig::default());

    let edit_env = Action::ApplyEdit {
        path: "/etc/passwd".to_string(),
        edits: vec![],
    };
    assert_eq!(
        workflow.assess_risk(&edit_env),
        RiskLevel::High,
        "apply_edit 写关键路径应与 write_file 同为 High"
    );

    let patch_env = Action::ApplyPatch {
        path: "/etc/passwd".to_string(),
        patch: "*** Update File: /etc/passwd\n@@\n-root\n+backdoor\n".to_string(),
    };
    assert_eq!(
        workflow.assess_risk(&patch_env),
        RiskLevel::High,
        "apply_patch 写关键路径应与 write_file 同为 High"
    );

    // 测试路径**刻意不降级**（与 write_file 的 Low 差异是有意的）：apply_edit
    // 改的是已存在文件（可能含生产代码），降级会让「改 tests/ 下文件」绕过确认门
    let edit_test = Action::ApplyEdit {
        path: "tests/foo_test.rs".to_string(),
        edits: vec![],
    };
    assert_eq!(
        workflow.assess_risk(&edit_test),
        RiskLevel::Medium,
        "编辑已存在文件不因 test 路径降级（保守）"
    );

    // 普通路径保持 Medium（原行为不变）
    let edit_src = Action::ApplyEdit {
        path: "src/lib.rs".to_string(),
        edits: vec![],
    };
    assert_eq!(workflow.assess_risk(&edit_src), RiskLevel::Medium);
}

#[test]
fn test_multi_file_patch_risk_takes_strictest_path() {
    // T1-16：多文件补丁——仅**后续**文件命中关键路径时也必须提升定级
    // （首路径普通，此前恒 Medium）。
    let workflow = ApprovalWorkflow::new(ApprovalWorkflowConfig::default());
    let patch =
        "*** Update File: src/main.rs\n@@\n-a\n+b\n*** Update File: /etc/hosts\n@@\n-x\n+y\n";
    let action = Action::ApplyPatch {
        path: "src/main.rs".to_string(),
        patch: patch.to_string(),
    };
    assert_eq!(
        workflow.assess_risk(&action),
        RiskLevel::High,
        "补丁中任一关键路径应提升整次定级"
    );
}

#[test]
fn test_risk_assessment_takes_max_across_command_segments() {
    // T0-2：多段命令取最严段——修复前只按整条首词定级，
    // `git status && rm -rf /` 被低估为 Medium（危险命令确认门被绕过）。
    let workflow = ApprovalWorkflow::new(ApprovalWorkflowConfig::default());
    let chained = Action::ExecuteCommand {
        command: "git status && rm -rf /".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    assert_eq!(
        workflow.assess_risk(&chained),
        RiskLevel::Critical,
        "第二段危险命令必须提升整条定级"
    );
    // 单段行为不变（首词即全命令语义）
    let single = Action::ExecuteCommand {
        command: "git status".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    assert_eq!(workflow.assess_risk(&single), RiskLevel::Medium);
}

#[test]
fn test_risk_assessment_segment_matching() {
    // 回归保护：风险定级按路径段/词边界匹配——
    // 1) 子串巧合不得误降级（attestation.txt 含 test、latest 含 test、
    //    attempt.log 含 tmp——都是非测试文件，必须保持 Medium）；
    // 2) 关键路径大小写归一（Windows 路径 c:\\windows 同样 High）；
    // 3) 技能名词边界（executive_summary 不含危险词）。
    let workflow = ApprovalWorkflow::new(ApprovalWorkflowConfig::default());

    // 误降级方向（安全）：子串巧合不得把普通文件当测试文件
    for p in [
        "attestation.txt",
        "latest_report.md",
        "attempt.log",
        "contemplate.md",
        "src/latest.rs",
    ] {
        assert_eq!(
            workflow.assess_risk(&Action::WriteFile {
                path: p.to_string(),
                content: String::new(),
            }),
            RiskLevel::Medium,
            "非测试文件不得因子串巧合被降级: {p}",
        );
    }

    // 真正的测试/临时文件仍为 Low
    for p in [
        "test.txt",
        "tests/foo.rs",
        "tmp/x.log",
        "src/temp.rs",
        "unit_test.rs",
    ] {
        assert_eq!(
            workflow.assess_risk(&Action::WriteFile {
                path: p.to_string(),
                content: String::new(),
            }),
            RiskLevel::Low,
            "测试/临时文件应为 Low: {p}",
        );
    }

    // 关键路径：大小写归一 + 变体命中
    for p in [
        "c:\\windows\\system32\\x.dll",
        "C:\\PROGRAM FILES\\app\\x.dll",
        "project/.env.local",
        "project/config.yaml.example",
    ] {
        assert_eq!(
            workflow.assess_risk(&Action::WriteFile {
                path: p.to_string(),
                content: String::new(),
            }),
            RiskLevel::High,
            "关键路径应为 High: {p}",
        );
    }

    // etc2 不是关键目录段（子串巧合不再命中）——中等风险
    assert_eq!(
        workflow.assess_risk(&Action::WriteFile {
            path: "/etc2/not-system".to_string(),
            content: String::new(),
        }),
        RiskLevel::Medium,
    );

    // 技能名词边界
    let exec_summary = Action::CallSkill {
        skill_id: "executive_summary".to_string(),
        parameters: serde_json::Map::new(),
    };
    assert_eq!(
        workflow.assess_risk(&exec_summary),
        RiskLevel::Low,
        "executive_summary 不应命中危险关键词 exec",
    );
    let del = Action::CallSkill {
        skill_id: "delete_all_files".to_string(),
        parameters: serde_json::Map::new(),
    };
    assert_eq!(
        workflow.assess_risk(&del),
        RiskLevel::High,
        "delete_all_files 应命中危险关键词",
    );
}
#[test]
fn test_auto_approval_rules() {
    let config = ApprovalWorkflowConfig::default();
    let workflow = ApprovalWorkflow::new(config);

    let safe_action = Action::ReadFile {
        path: "test.txt".to_string(),
    };
    let decision = workflow.check_auto_approval(&safe_action, RiskLevel::Safe);
    assert_eq!(decision, Some(ApprovalDecision::Approve));
}

#[tokio::test]
async fn test_autonomous_mode_auto_approves_medium_risk() {
    // ADR-033：全自主模式自动批准
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Autonomous,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    // git 命令为 Medium 风险
    let action = Action::ExecuteCommand {
        command: "git status".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    assert_eq!(workflow.assess_risk(&action), RiskLevel::Medium);

    // 应立即返回批准，而非等待 300 秒人工响应
    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
    assert_eq!(resp.approved_by, "auto");
    assert!(resp.reason.unwrap().contains("全自主"));

    // 审计记录已写入
    let records = workflow.get_approval_records().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].request.risk_level, RiskLevel::Medium);
    assert_eq!(
        records[0].response.as_ref().unwrap().decision,
        ApprovalDecision::Approve
    );
}

#[tokio::test]
async fn test_confirm_mode_keeps_critical_denied() {
    // ADR-033：确认模式保留 Critical 默认拒绝（全自主模式除黑名单外放行）
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));

    let action = Action::ExecuteCommand {
        command: "rm -rf /".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    assert_eq!(workflow.assess_risk(&action), RiskLevel::Critical);

    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);
    assert_eq!(resp.approved_by, "system");
}

#[tokio::test]
async fn test_autonomous_mode_approves_everything_except_blocked() {
    // ADR-033 全自主模式：除 Deny 规则（黑名单）外全部自动批准，不等待、不追问。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Autonomous,
        auto_approval_rules: vec![
            // 黑名单 Deny（模拟 security_blocked_ 注入）
            AutoApprovalRule {
                name: "security_blocked_rm_rf".to_string(),
                action_pattern: ActionPattern::CommandPattern("rm -rf /".to_string()),
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Deny,
                enabled: true,
            },
        ],
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    // 日常命令（git 为 Medium）→ 直接批准
    let git = Action::ExecuteCommand {
        command: "git status".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    let resp = workflow.request_approval("s1", &git).await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
    assert_eq!(resp.approved_by, "auto");

    // 危险但非黑名单（普通 rm 单文件）→ 也批准（完全放开）
    let rm_file = Action::ExecuteCommand {
        command: "rm file.txt".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    let resp = workflow.request_approval("s1", &rm_file).await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);

    // 黑名单（rm -rf /，Critical）→ 仍强制拒绝（Deny 规则优先）
    let rm_rf = Action::ExecuteCommand {
        command: "rm -rf /".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    let resp = workflow.request_approval("s1", &rm_rf).await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);

    // 多词黑名单前缀匹配：rm -rf /etc 也应拒绝（全目录删除类）
    let rm_rf_etc = Action::ExecuteCommand {
        command: "rm -rf /etc".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    let resp = workflow.request_approval("s1", &rm_rf_etc).await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);
}

#[tokio::test]
async fn test_confirm_mode_denies_without_confirmation() {
    // 确认模式（ADR-033）：Medium 风险无确认时立即拒绝，由上层降级为追问
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));

    let action = Action::ExecuteCommand {
        command: "git status".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    assert_eq!(workflow.assess_risk(&action), RiskLevel::Medium);

    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);
    assert!(resp.reason.as_deref().unwrap().contains("需要用户确认"));
}

#[tokio::test]
async fn test_attended_mode_approves_after_user_confirmation() {
    // 确认模式（ADR-033）：未确认拒绝；用户确认（指纹）后批准。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));

    let action = Action::ExecuteCommand {
        command: "git status".to_string(),
        cwd: None,
        timeout_secs: None,
    };

    // 未确认：拒绝
    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);

    // 用户确认后：批准
    workflow.record_user_confirmation(&action).await;
    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
    assert_eq!(resp.approved_by, "user");
}

#[tokio::test]
async fn test_attended_mode_still_requests_human_approval() {
    // 交互模式（ADR-033；原 wait_for_approval）：
    // Medium 风险应进入待处理队列等待人工审批
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let action = Action::ExecuteCommand {
        command: "git status".to_string(),
        cwd: None,
        timeout_secs: None,
    };

    let wf = workflow.clone();
    let handle =
        tokio::spawn(async move { wf.request_approval("session-1", &action).await.unwrap() });

    // 等待请求进入待处理队列
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let request_id = loop {
        let pending = workflow.get_pending_approvals().await;
        if pending.len() == 1 {
            assert_eq!(pending[0].risk_level, RiskLevel::Medium);
            break pending[0].request_id.clone();
        }
        if tokio::time::Instant::now() > deadline {
            panic!("审批请求未进入待处理队列");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // 模拟人工批准，spawn 的任务应解除等待并返回批准
    workflow
        .respond_to_approval(&request_id, ApprovalDecision::Approve, None, "human", None)
        .await
        .unwrap();
    let resp = handle.await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
    assert_eq!(resp.approved_by, "human");
}

/// 记录挂起通知调用的 mock 通知器。
struct RecordingNotifier {
    calls: tokio::sync::Mutex<Vec<ApprovalRequest>>,
}

#[async_trait::async_trait]
impl ApprovalPendingNotifier for RecordingNotifier {
    async fn on_approval_pending(&self, request: &ApprovalRequest) {
        self.calls.lock().await.push(request.clone());
    }
}

#[tokio::test]
async fn test_approval_pending_notifier_called() {
    // 回归保护：交互模式挂起时必须通知（后台任务等无人值守场景
    // 的审批请求不再静默——用户经通知到审批面板响应后任务恢复）。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        ..Default::default()
    };
    let notifier = Arc::new(RecordingNotifier {
        calls: tokio::sync::Mutex::new(Vec::new()),
    });
    let workflow = Arc::new(ApprovalWorkflow::new(config).with_pending_notifier(notifier.clone()));

    let action = Action::ExecuteCommand {
        command: "git push".to_string(),
        cwd: None,
        timeout_secs: None,
    };

    let wf = workflow.clone();
    let handle =
        tokio::spawn(async move { wf.request_approval("session-bg", &action).await.unwrap() });

    // 等待通知触发
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let request_id = loop {
        let calls = notifier.calls.lock().await;
        if !calls.is_empty() {
            let req = &calls[0];
            assert_eq!(req.session_id, "session-bg", "通知应携带挂起会话 ID");
            assert_eq!(req.risk_level, RiskLevel::Medium);
            break req.request_id.clone();
        }
        drop(calls);
        if tokio::time::Instant::now() > deadline {
            panic!("挂起通知未被触发");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // 通知后审批面板响应 → 任务恢复
    workflow
        .respond_to_approval(&request_id, ApprovalDecision::Approve, None, "human", None)
        .await
        .unwrap();
    let resp = handle.await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
    assert_eq!(resp.approved_by, "human");
}

#[test]
fn test_approval_pending_text_contains_guidance() {
    // 通知文本须含操作描述/风险/请求 ID/超时指引（用户据此决策与导航）
    let request = ApprovalRequest {
        request_id: "req_123".to_string(),
        session_id: "s1".to_string(),
        action_description: "执行命令: rm -rf tmp".to_string(),
        action: Action::ExecuteCommand {
            command: "rm -rf tmp".to_string(),
            cwd: None,
            timeout_secs: None,
        },
        risk_level: RiskLevel::Critical,
        requested_at: chrono::Utc::now(),
        timeout_secs: 300,
    };
    let text = build_approval_pending_text(&request);
    assert!(text.contains("审批请求"));
    assert!(text.contains("rm -rf tmp"));
    assert!(text.contains("critical"), "应包含风险等级（Display 小写）");
    assert!(text.contains("req_123"));
    assert!(text.contains("审批」面板"), "应给出审批面板指引");
    assert!(text.contains("300"), "应包含超时秒数");
}

#[tokio::test]
async fn test_request_approval_no_wait_never_waits() {
    // 回归保护：子任务审批（no_wait）永不等待人工响应——即使交互模式
    // （主循环会挂起），子任务也必须立即拒绝
    // 并携带"主任务授权"标记；不产生挂起请求。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let action = Action::ExecuteCommand {
        command: "cargo test".to_string(),
        cwd: None,
        timeout_secs: None,
    };

    let wf = workflow.clone();
    let fut = async move { wf.request_approval_no_wait("session-1", &action).await };
    let resp = tokio::time::timeout(Duration::from_secs(2), fut)
        .await
        .expect("no_wait 必须立即返回，不得挂起等待")
        .unwrap();

    assert_eq!(resp.decision, ApprovalDecision::Deny);
    assert!(
        resp.reason
            .as_deref()
            .unwrap_or("")
            .contains("子任务操作需要主任务授权"),
        "拒绝原因应携带主任务授权标记: {:?}",
        resp.reason
    );
    assert!(
        workflow.get_pending_approvals().await.is_empty(),
        "子任务审批不应产生挂起请求"
    );
}

// ── T2-6 命令级审批策略：规则引擎补全 ─────────────────────────────

/// 构造命令执行动作。
fn cmd_action(command: &str) -> Action {
    Action::ExecuteCommand {
        command: command.to_string(),
        cwd: None,
        timeout_secs: None,
    }
}

#[test]
fn test_blocked_command_deny_overrides_auto_approve() {
    // 低风险命令命中黑名单 Deny 规则时强制拒绝——
    // 即使默认的 Any + RiskBelow(Medium) 放行规则也会匹配（deny 优先）。
    let config = ApprovalWorkflowConfig {
        auto_approval_rules: vec![
            AutoApprovalRule {
                name: "safe_operations".to_string(),
                action_pattern: ActionPattern::Any,
                condition: ApprovalCondition::RiskBelow(RiskLevel::Medium),
                decision: ApprovalDecision::Approve,
                enabled: true,
            },
            AutoApprovalRule {
                name: "security_blocked_dir".to_string(),
                action_pattern: ActionPattern::CommandPattern("dir".to_string()),
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Deny,
                enabled: true,
            },
        ],
        ..Default::default()
    };
    let workflow = ApprovalWorkflow::new(config);

    let action = cmd_action("dir");
    assert_eq!(
        workflow.assess_risk(&action),
        RiskLevel::Low,
        "dir 应为低风险"
    );
    let decision = workflow.check_auto_approval(&action, RiskLevel::Low);
    assert_eq!(
        decision,
        Some(ApprovalDecision::Deny),
        "黑名单拒绝必须优先于放行"
    );
}

#[test]
fn test_deny_rule_priority_regardless_of_rule_order() {
    // Approve 规则配置在 Deny 规则之前时，Deny 仍必须胜出（评估顺序 deny 优先）
    let config = ApprovalWorkflowConfig {
        auto_approval_rules: vec![
            AutoApprovalRule {
                name: "security_allowed_git".to_string(),
                action_pattern: ActionPattern::CommandPattern("git".to_string()),
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Approve,
                enabled: true,
            },
            AutoApprovalRule {
                name: "security_blocked_git".to_string(),
                action_pattern: ActionPattern::CommandPattern("git".to_string()),
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Deny,
                enabled: true,
            },
        ],
        ..Default::default()
    };
    let workflow = ApprovalWorkflow::new(config);

    let action = cmd_action("git status");
    let decision = workflow.check_auto_approval(&action, RiskLevel::Medium);
    assert_eq!(
        decision,
        Some(ApprovalDecision::Deny),
        "deny 规则优先于 approve 规则"
    );
}

#[tokio::test]
async fn test_blocked_command_denied_even_in_interactive_mode() {
    // 命中 Deny 规则的命令：即使交互模式（否则 Medium 风险
    // 会挂起等待人工），也立即强制拒绝，不进入待处理队列。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        auto_approval_rules: vec![AutoApprovalRule {
            name: "security_blocked_git".to_string(),
            action_pattern: ActionPattern::CommandPattern("git".to_string()),
            condition: ApprovalCondition::Always,
            decision: ApprovalDecision::Deny,
            enabled: true,
        }],
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let resp = workflow
        .request_approval("session-1", &cmd_action("git status"))
        .await
        .unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);
    assert!(resp.reason.unwrap().contains("自动审批规则匹配"));
    assert!(
        workflow.get_pending_approvals().await.is_empty(),
        "拒绝规则命中时不得挂起等待"
    );
}

#[tokio::test]
async fn test_prompt_commands_force_ask_even_in_autonomous_mode() {
    // 总是询问命令：即使全自主模式（否则 Medium 风险自动批准）
    // 也强制走审批——未确认即拒绝。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Autonomous,
        prompt_commands: vec!["git".to_string()],
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let action = cmd_action("git status");
    assert_eq!(workflow.assess_risk(&action), RiskLevel::Medium);
    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(
        resp.decision,
        ApprovalDecision::Deny,
        "prompt 命令不得被无人值守放行"
    );
    assert!(resp.reason.unwrap().contains("需要用户确认"));
}

#[tokio::test]
async fn test_prompt_commands_force_ask_for_safe_risk() {
    // 低风险命令命中总是询问列表时也不得自动放行
    // （默认 safe_operations 规则会放行 Low 风险命令）。
    let config = ApprovalWorkflowConfig {
        prompt_commands: vec!["echo".to_string()],
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let action = cmd_action("echo hi");
    assert_eq!(workflow.assess_risk(&action), RiskLevel::Low);
    let resp = workflow
        .request_approval("session-1", &action)
        .await
        .unwrap();
    assert_eq!(
        resp.decision,
        ApprovalDecision::Deny,
        "prompt 命令不得被 Safe 放行"
    );
}

#[tokio::test]
async fn test_prompt_command_routes_to_human_approval() {
    // 总是询问命令 + 交互模式：强制进入待处理队列等待人工响应
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        prompt_commands: vec!["echo".to_string()],
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let wf = workflow.clone();
    let handle = tokio::spawn(async move {
        wf.request_approval("session-1", &cmd_action("echo hi"))
            .await
            .unwrap()
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let request_id = loop {
        let pending = workflow.get_pending_approvals().await;
        if pending.len() == 1 {
            break pending[0].request_id.clone();
        }
        if tokio::time::Instant::now() > deadline {
            panic!("prompt 命令未进入待处理队列");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    workflow
        .respond_to_approval(&request_id, ApprovalDecision::Approve, None, "user", None)
        .await
        .unwrap();
    let resp = handle.await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
}

// ── T2-6 审批卡编辑命令：审计记录 ───────────────────────────────

#[tokio::test]
async fn test_respond_with_edited_command_records_audit() {
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let wf = workflow.clone();
    let handle = tokio::spawn(async move {
        wf.request_approval("session-1", &cmd_action("git push"))
            .await
            .unwrap()
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let request_id = loop {
        let pending = workflow.get_pending_approvals().await;
        if pending.len() == 1 {
            break pending[0].request_id.clone();
        }
        if tokio::time::Instant::now() > deadline {
            panic!("审批请求未进入待处理队列");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // 用户批准并提供编辑后的命令（纠正/改写）
    workflow
        .respond_to_approval(
            &request_id,
            ApprovalDecision::Approve,
            None,
            "user",
            Some("git push --force-with-lease".to_string()),
        )
        .await
        .unwrap();
    let resp = handle.await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Approve);
    assert_eq!(
        resp.edited_command.as_deref(),
        Some("git push --force-with-lease"),
        "响应应携带编辑后的命令（等待方可见）"
    );

    // 审计记录写入编辑后的命令
    let records = workflow.get_approval_records().await;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].edited_command.as_deref(),
        Some("git push --force-with-lease"),
        "审计记录应包含编辑后的命令"
    );
    assert_eq!(
        records[0]
            .response
            .as_ref()
            .unwrap()
            .edited_command
            .as_deref(),
        Some("git push --force-with-lease")
    );
}

#[tokio::test]
async fn test_respond_deny_ignores_edited_command() {
    // 编辑命令仅对批准语义生效：deny 响应忽略编辑值，审计不记录
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));

    let wf = workflow.clone();
    let handle = tokio::spawn(async move {
        wf.request_approval("session-1", &cmd_action("git push"))
            .await
            .unwrap()
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let request_id = loop {
        let pending = workflow.get_pending_approvals().await;
        if pending.len() == 1 {
            break pending[0].request_id.clone();
        }
        if tokio::time::Instant::now() > deadline {
            panic!("审批请求未进入待处理队列");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    workflow
        .respond_to_approval(
            &request_id,
            ApprovalDecision::Deny,
            None,
            "user",
            Some("git push --force".to_string()),
        )
        .await
        .unwrap();
    let resp = handle.await.unwrap();
    assert_eq!(resp.decision, ApprovalDecision::Deny);
    assert_eq!(resp.edited_command, None, "deny 响应不得携带编辑命令");

    let records = workflow.get_approval_records().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].edited_command, None, "deny 审计不记录编辑命令");
}
