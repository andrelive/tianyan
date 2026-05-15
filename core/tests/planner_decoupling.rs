//! Planner 与 SessionState 解耦的集成测试。
//!
//! 验证 PlannerContext 与 SessionState 之间的转换逻辑：
//! 1. SessionState → PlannerContext 正确捕获会话状态
//! 2. PlannerMutation → SessionState::apply_mutations 正确应用变更
//! 3. PlannerContext 构建 Prompt 功能

mod common;

use tianyan::common::types::MessageRole;
use tianyan::planner::types::{
    ClarificationQuestion, Plan, PlannerContext, PlannerMutation, QuestionType, StepResult,
};

use common::factory::{
    test_messages, test_planner_context, test_session_state, test_session_state_with_messages,
    verify_planner_context,
};

#[test]
fn test_session_state_to_planner_context_conversion() {
    let messages = test_messages(6);
    let state = test_session_state_with_messages("test-session-1", messages.clone());

    let ctx = state.to_planner_context();

    assert_eq!(ctx.conversation.len(), messages.len());
    for (i, (expected, actual)) in messages.iter().zip(ctx.conversation.iter()).enumerate() {
        assert_eq!(expected.role, actual.role, "消息 {} 的角色不匹配", i);
        assert_eq!(expected.content, actual.content, "消息 {} 的内容不匹配", i);
    }
    assert!(ctx.execution_turns.is_empty());
    assert!(ctx.context_window.is_none());
}

#[test]
fn test_planner_context_empty_state() {
    let ctx = test_planner_context();
    verify_planner_context(&ctx, 4, 0);
}

#[test]
fn test_planner_context_with_history_state() {
    let ctx = common::factory::test_planner_context_with_history(3);
    verify_planner_context(&ctx, 4, 3);
    for (i, turn) in ctx.execution_turns.iter().enumerate() {
        assert_eq!(turn.results.len(), 1, "轮次 {} 应有 1 个结果", i);
        assert!(turn.results[0].success, "轮次 {} 的结果应为成功", i);
    }
}

#[test]
fn test_planner_context_build_prompt_no_window() {
    let ctx = test_planner_context();
    let prompt = ctx.build_prompt("测试用户输入");

    assert!(prompt.contains("测试用户输入"));
    assert!(prompt.contains("执行计划"), "无上下文窗口时应包含默认提示");
}

#[test]
fn test_apply_mutations_add_turn() {
    let mut state = test_session_state("test-session-2");

    let plan = Plan::Steps(vec![]);
    let results = vec![StepResult {
        step_id: 0,
        success: true,
        output: serde_json::Value::String("执行完成".to_string()),
        error: None,
        actual_importance: Some(0.7),
    }];

    let mutations = vec![PlannerMutation::AddTurn(plan.clone(), results.clone())];
    state.apply_mutations(mutations);

    let ctx = state.to_planner_context();
    assert_eq!(ctx.execution_turns.len(), 1);
    assert_eq!(ctx.execution_turns[0].results.len(), 1);
    assert!(ctx.execution_turns[0].results[0].success);
}

#[test]
fn test_apply_mutations_add_message() {
    let mut state = test_session_state("test-session-3");

    let mutations = vec![PlannerMutation::AddAssistantMessage(
        "助手回答内容".to_string(),
    )];
    state.apply_mutations(mutations);

    let ctx = state.to_planner_context();
    assert_eq!(ctx.conversation.len(), 1);
    assert_eq!(ctx.conversation[0].role, MessageRole::Assistant);
    assert_eq!(ctx.conversation[0].content, "助手回答内容");
}

#[test]
fn test_apply_mutations_clarification() {
    let mut state = test_session_state("test-session-4");

    let questions = vec![ClarificationQuestion {
        question: "需要更多信息吗？".to_string(),
        question_type: QuestionType::Confirmation,
        options: None,
        required: true,
    }];

    let mutations = vec![PlannerMutation::SetPendingClarification(questions.clone())];
    state.apply_mutations(mutations);

    assert!(state.pending_clarification.is_some(), "应设置待处理追问");
    let pending = state.pending_clarification.as_ref().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].question, "需要更多信息吗？");
}

#[test]
fn test_apply_mutations_composite() {
    let mut state = test_session_state("test-session-5");

    let plan = Plan::Steps(vec![]);
    let results = vec![StepResult {
        step_id: 0,
        success: true,
        output: serde_json::Value::String("完成".to_string()),
        error: None,
        actual_importance: None,
    }];

    let mutations = vec![
        PlannerMutation::AddTurn(plan, results),
        PlannerMutation::AddAssistantMessage("综合回答".to_string()),
    ];
    state.apply_mutations(mutations);

    let ctx = state.to_planner_context();
    assert_eq!(ctx.execution_turns.len(), 1);
    assert_eq!(ctx.conversation.len(), 1);
    assert_eq!(ctx.conversation[0].content, "综合回答");
}

#[test]
fn test_planner_context_round_trip() {
    let original_state = test_session_state_with_messages("round-trip-test", test_messages(4));
    let original_ctx = original_state.to_planner_context();

    let mut new_state = test_session_state("reconstructed");
    for msg in &original_ctx.conversation {
        new_state.add_message(msg.clone());
    }

    if !original_ctx.execution_turns.is_empty() {
        let mutations: Vec<PlannerMutation> = original_ctx
            .execution_turns
            .iter()
            .map(|t| PlannerMutation::AddTurn(t.plan.clone(), t.results.clone()))
            .collect();
        new_state.apply_mutations(mutations);
    }

    let reconstructed_ctx = new_state.to_planner_context();
    assert_eq!(
        reconstructed_ctx.conversation.len(),
        original_ctx.conversation.len()
    );
    assert_eq!(
        reconstructed_ctx.execution_turns.len(),
        original_ctx.execution_turns.len()
    );
}

#[test]
fn test_planner_context_execution_summary() {
    let turns = common::factory::test_execution_turns(2);
    let ctx = PlannerContext {
        conversation: vec![],
        execution_turns: turns,
        context_window: None,
    };

    let summary = ctx.build_execution_summary();
    assert!(summary.is_array());
    let arr = summary.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}
