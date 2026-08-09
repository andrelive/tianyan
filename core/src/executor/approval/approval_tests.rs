use std::sync::Arc;
use std::time::Duration;

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
async fn test_unattended_mode_auto_approves_medium_risk() {
    // 显式开启无人值守模式
    let config = ApprovalWorkflowConfig {
        unattended_mode: true,
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
    assert!(resp.reason.unwrap().contains("无人值守"));

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
async fn test_unattended_mode_keeps_critical_denied() {
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig::default()));

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
async fn test_attended_mode_denies_without_confirmation() {
    // 默认（attended、无审批通道）：Medium 风险应立即拒绝，由上层降级为追问
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig::default()));

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
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig::default()));

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
    // 显式开启 wait_for_approval（GUI 审批通道接入后）：
    // Medium 风险应进入待处理队列等待人工审批
    let config = ApprovalWorkflowConfig {
        unattended_mode: false,
        wait_for_approval: true,
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
        .respond_to_approval(&request_id, ApprovalDecision::Approve, None, "human")
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
    // 回归保护：wait_for_approval 挂起时必须通知（后台任务等无人值守场景
    // 的审批请求不再静默——用户经通知到审批面板响应后任务恢复）。
    let config = ApprovalWorkflowConfig {
        unattended_mode: false,
        wait_for_approval: true,
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
        .respond_to_approval(&request_id, ApprovalDecision::Approve, None, "human")
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
    // 回归保护：子任务审批（no_wait）永不等待人工响应——即使全局
    // wait_for_approval=true（主循环会挂起），子任务也必须立即拒绝
    // 并携带"主任务授权"标记；不产生挂起请求。
    let config = ApprovalWorkflowConfig {
        unattended_mode: false,
        wait_for_approval: true,
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
