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
