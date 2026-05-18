use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::agent::types::ClarificationQuestion;
use crate::common::types::{Message, MessageRole};
use crate::context::types::ContextWindow;
use crate::session::Session;

const MAX_CONVERSATION_MESSAGES: usize = 100;
const KEEP_RECENT_MESSAGES: usize = 50;

/// 会话状态容器（conversation 为唯一真相源）。
#[derive(Debug, Clone)]
pub struct SessionState {
    /// 会话 ID。
    pub session_id: String,
    /// 对话历史（含 user / assistant / tool 消息）。
    pub conversation: Vec<Message>,
    /// 当前目标。
    pub current_goal: Option<String>,
    /// 待追问问题。
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,
    /// 最后活动时间。
    pub last_activity: Instant,
    /// 上下文窗口。
    pub context_window: Option<ContextWindow>,
    /// 待持久化记忆。
    pub pending_memories: Vec<crate::common::types::MemoryEntry>,
    /// 总 token 数。
    pub total_tokens: usize,
    /// 开始时间。
    pub start_time: Instant,
}

impl SessionState {
    /// 创建新的会话状态。
    pub fn new(session_id: &str) -> Self {
        let now = Instant::now();
        Self {
            session_id: session_id.to_string(),
            conversation: Vec::new(),
            current_goal: None,
            pending_clarification: None,
            last_activity: now,
            context_window: None,
            pending_memories: Vec::new(),
            total_tokens: 0,
            start_time: now,
        }
    }

    /// 添加用户消息。
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.conversation.push(Message::user(content));
        self.last_activity = Instant::now();
    }

    /// 添加助手消息。
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.conversation.push(Message::assistant(content));
        self.last_activity = Instant::now();
    }

    /// 添加消息并裁剪历史。
    pub fn add_message(&mut self, message: Message) {
        self.conversation.push(message);
        self.trim_conversation();
        self.last_activity = Instant::now();
    }

    /// 获取最近 n 条消息。
    pub fn recent_messages(&self, n: usize) -> Vec<Message> {
        self.conversation
            .iter()
            .rev()
            .take(n)
            .rev()
            .cloned()
            .collect()
    }

    /// 消息总数。
    pub fn message_count(&self) -> usize {
        self.conversation.len()
    }

    /// 转换为 Session。
    pub fn to_session(&self) -> Session {
        let mut session = Session::new(&self.session_id);
        for msg in &self.conversation {
            match msg.role {
                MessageRole::User => session.add_user_message(&msg.content),
                MessageRole::Assistant => session.add_assistant_message(&msg.content),
                MessageRole::System => session.add_system_message(&msg.content),
                MessageRole::Tool => {}
            }
        }
        session
    }

    /// 构建 Prompt 上下文。
    pub fn build_prompt_context(&self, current_input: &str) -> String {
        if let Some(ref window) = self.context_window {
            crate::context::assembly::assemble_prompt(window, &self.conversation, current_input)
        } else {
            format!("## 当前输入\n\n{}\n\n## 你的执行计划\n", current_input)
        }
    }

    /// 清理过期消息。
    pub fn cleanup(&mut self) {
        self.trim_conversation();
    }

    fn trim_conversation(&mut self) {
        if self.conversation.len() <= MAX_CONVERSATION_MESSAGES {
            return;
        }
        let has_system_msg =
            !self.conversation.is_empty() && self.conversation[0].role == MessageRole::System;
        let start = if has_system_msg {
            let keep_from = self.conversation.len().saturating_sub(KEEP_RECENT_MESSAGES);
            if keep_from <= 1 {
                return;
            }
            1
        } else {
            let keep_from = self.conversation.len().saturating_sub(KEEP_RECENT_MESSAGES);
            if keep_from == 0 {
                return;
            }
            0
        };
        let keep_from = self.conversation.len().saturating_sub(KEEP_RECENT_MESSAGES);
        if keep_from > start {
            self.conversation.drain(start..keep_from);
        }
    }

    /// 获取对话引用。
    pub fn get_conversation(&self) -> &[Message] {
        &self.conversation
    }
}

/// 多会话状态管理器。
#[derive(Debug, Clone)]
pub struct SessionStateManager {
    states: Arc<RwLock<HashMap<String, SessionState>>>,
}

impl SessionStateManager {
    /// 创建新的管理器。
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取或创建会话状态并执行闭包。
    pub async fn with_state<F, R>(&self, session_id: &str, f: F) -> R
    where
        F: FnOnce(&mut SessionState) -> R,
    {
        let mut states = self.states.write().await;
        let state = states
            .entry(session_id.to_string())
            .or_insert_with(|| SessionState::new(session_id));
        f(state)
    }

    /// 只读访问会话状态。
    pub async fn with_state_read<F, R>(&self, session_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&SessionState) -> R,
    {
        let states = self.states.read().await;
        states.get(session_id).map(f)
    }

    /// 删除会话状态。
    pub async fn remove(&self, session_id: &str) {
        let mut states = self.states.write().await;
        states.remove(session_id);
    }

    /// 获取所有会话 ID。
    pub async fn get_all_session_ids(&self) -> Vec<String> {
        let states = self.states.read().await;
        states.keys().cloned().collect()
    }

    /// 清理过期会话。
    pub async fn cleanup_expired(&self, max_inactive_duration_secs: u64) {
        let mut states = self.states.write().await;
        let duration = std::time::Duration::from_secs(max_inactive_duration_secs);
        states.retain(|_, state| state.last_activity.elapsed() < duration);
    }
}

impl Default for SessionStateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state_new() {
        let state = SessionState::new("test-session");
        assert_eq!(state.session_id, "test-session");
        assert!(state.conversation.is_empty());
        assert!(state.current_goal.is_none());
        assert!(state.pending_clarification.is_none());
    }

    #[test]
    fn test_session_state_add_messages() {
        let mut state = SessionState::new("test-session");
        state.add_user_message("Hello");
        assert_eq!(state.conversation.len(), 1);
        assert_eq!(state.conversation[0].role, MessageRole::User);
        state.add_assistant_message("Hi there!");
        assert_eq!(state.conversation.len(), 2);
        assert_eq!(state.conversation[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_session_state_cleanup() {
        let mut state = SessionState::new("test-session");
        for i in 0..101 {
            state.add_user_message(format!("Message {}", i));
        }
        state.cleanup();
        assert!(state.conversation.len() <= KEEP_RECENT_MESSAGES);
        assert!(!state.conversation.is_empty());
    }

    #[test]
    fn test_session_state_build_prompt_context() {
        let mut state = SessionState::new("test-session");
        state.add_user_message("Hello");
        state.add_assistant_message("Hi there!");
        let prompt = state.build_prompt_context("What's next?");
        assert!(prompt.contains("## 当前输入"));
        assert!(prompt.contains("What's next"));
    }

    #[tokio::test]
    async fn test_session_state_manager() {
        let manager = SessionStateManager::new();
        let session_id = manager
            .with_state("session1", |state| state.session_id.clone())
            .await;
        assert_eq!(session_id, "session1");
        let exists = manager.with_state_read("session1", |_| true).await;
        assert!(exists.is_some());
        manager.remove("session1").await;
        let exists_after_remove = manager.with_state_read("session1", |_| true).await;
        assert!(exists_after_remove.is_none());
    }
}
