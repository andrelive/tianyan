//! 集成测试的共享测试工厂函数。

use tianyan::agent::session_state::SessionState;
use tianyan::common::types::{MemoryCategory, MemoryEntry, Message, MessageRole};
use tianyan::config::AgentConfig;
use tianyan::planner::types::{Plan, PlannerContext, PlannerOutput, StepResult, Turn};

/// 创建 n 条交替的用户/助手消息。
pub fn test_messages(n: usize) -> Vec<Message> {
    let mut messages = Vec::with_capacity(n);
    for i in 0..n {
        let role = if i % 2 == 0 {
            MessageRole::User
        } else {
            MessageRole::Assistant
        };
        messages.push(Message::new(role, format!("测试消息 {}", i + 1)));
    }
    messages
}

/// 创建包含系统提示词和用户消息的对话。
pub fn test_conversation(user_msg: &str) -> Vec<Message> {
    vec![Message::system("测试系统提示词"), Message::user(user_msg)]
}

/// 创建测试执行轮次。
pub fn test_execution_turns(n: usize) -> Vec<Turn> {
    let mut turns = Vec::with_capacity(n);
    let now = std::time::Instant::now();
    for i in 0..n {
        let plan = Plan::Steps(vec![]);
        let results = vec![StepResult {
            step_id: i as usize,
            success: true,
            output: serde_json::Value::String(format!("结果 {}", i + 1)),
            error: None,
            actual_importance: Some(0.5),
        }];
        turns.push(Turn {
            plan,
            results,
            timestamp: now,
            turn_id: i,
        });
    }
    turns
}

/// 创建最小可行的 PlannerContext。
pub fn test_planner_context() -> PlannerContext {
    PlannerContext {
        conversation: test_messages(4),
        execution_turns: vec![],
        context_window: None,
    }
}

/// 创建带执行历史的 PlannerContext。
pub fn test_planner_context_with_history(turn_count: usize) -> PlannerContext {
    PlannerContext {
        conversation: test_messages(4),
        execution_turns: test_execution_turns(turn_count),
        context_window: None,
    }
}

/// 创建测试用 SessionState。
pub fn test_session_state(session_id: &str) -> SessionState {
    SessionState::new(session_id)
}

/// 创建带对话历史的测试用 SessionState。
pub fn test_session_state_with_messages(session_id: &str, messages: Vec<Message>) -> SessionState {
    let mut state = SessionState::new(session_id);
    for msg in messages {
        state.add_message(msg);
    }
    state
}

/// 创建测试用 AgentConfig。
pub fn test_agent_config() -> AgentConfig {
    AgentConfig::default()
}

/// 创建测试用记忆条目。
pub fn test_memory_entry(id: &str, content: &str, category: MemoryCategory) -> MemoryEntry {
    MemoryEntry::new(id, content, category)
}

/// 验证 PlannerOutput 是否为 Answer 类型。
pub fn assert_answer(output: &PlannerOutput, expected_prefix: &str) {
    match output {
        PlannerOutput::Answer(content) => {
            assert!(
                content.contains(expected_prefix),
                "期望 Answer 包含 '{}'，实际: '{}'",
                expected_prefix,
                content
            );
        }
        PlannerOutput::Clarification { .. } => {
            panic!("期望 Answer，实际是 Clarification");
        }
    }
}

/// 验证 PlannerContext 的对话和轮次正确记录。
pub fn verify_planner_context(
    ctx: &PlannerContext,
    expected_msg_count: usize,
    expected_turn_count: usize,
) {
    assert_eq!(
        ctx.conversation.len(),
        expected_msg_count,
        "期望 {} 条消息，实际 {} 条",
        expected_msg_count,
        ctx.conversation.len()
    );
    assert_eq!(
        ctx.execution_turns.len(),
        expected_turn_count,
        "期望 {} 轮执行，实际 {} 轮",
        expected_turn_count,
        ctx.execution_turns.len()
    );
}
