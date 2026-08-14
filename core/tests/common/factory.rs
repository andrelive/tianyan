//! 集成测试的共享测试工厂函数。

use tianyan::agent::session_state::SessionState;
use tianyan::common::types::{MemoryCategory, MemoryEntry, Message, MessageRole};
use tianyan::config::AgentConfig;

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

/// 创建测试用 SessionState。
pub fn test_session_state(session_id: &str) -> SessionState {
    SessionState::new(session_id)
}

/// 创建带对话历史的测试用 SessionState。
pub fn test_session_state_with_messages(session_id: &str, messages: Vec<Message>) -> SessionState {
    let mut state = SessionState::new(session_id);
    for msg in messages {
        state.add_user_message(msg.content.clone());
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
