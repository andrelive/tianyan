use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::agent::types::ClarificationQuestion;
use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage};

const MAX_CONVERSATION_MESSAGES: usize = 100;
const KEEP_RECENT_MESSAGES: usize = 50;

/// 可注入的上下文内容，由 ContextPipeline 填充，由 ContextAssembler 组装使用。
#[derive(Debug, Clone, Default)]
pub struct InjectableContext {
    /// 智能体核心人格（soul.md）。
    pub soul: String,
    /// 经验与方法论。
    pub rules_and_experiences: Vec<String>,
    /// 用户画像与环境事实。
    pub memories: Vec<String>,
    /// 最后更新时间。
    pub last_updated: DateTime<Utc>,
}

impl InjectableContext {
    /// 创建空的注入上下文。
    pub fn new() -> Self {
        Self {
            last_updated: Utc::now(),
            ..Default::default()
        }
    }
}

/// 会话状态容器（conversation 为唯一真相源）。
#[derive(Debug, Clone)]
pub struct SessionState {
    /// 会话 ID。
    pub session_id: String,
    /// 结构化消息（持久化格式）。
    pub structured_messages: Vec<StructuredMessage>,
    /// 可注入的上下文（由 ContextPipeline 填充）。
    pub injectable_context: InjectableContext,
    /// 当前目标。
    pub current_goal: Option<String>,
    /// 待追问问题。
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,
    /// 最后活动时间。
    pub last_activity: Instant,
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
            structured_messages: Vec::new(),
            injectable_context: InjectableContext::new(),
            current_goal: None,
            pending_clarification: None,
            last_activity: now,
            pending_memories: Vec::new(),
            total_tokens: 0,
            start_time: now,
        }
    }

    /// 添加用户消息。
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        let msg = StructuredMessage {
            id: format!("msg_{}", Utc::now().timestamp_millis()),
            parent_id: self.structured_messages.last().map(|m| m.id.clone()),
            role: MessageRole::User,
            parts: vec![Part::Text {
                text: content.into(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: self.session_id.clone(),
            finish: None,
            compression_marker: false,
        };
        self.structured_messages.push(msg);
        self.trim_conversation();
        self.last_activity = Instant::now();
    }

    /// 添加结构化消息并裁剪历史。
    pub fn add_structured_message(&mut self, msg: StructuredMessage) {
        self.structured_messages.push(msg);
        self.trim_conversation();
        self.last_activity = Instant::now();
    }

    /// 获取最近 n 条消息。
    pub fn recent_messages(&self, n: usize) -> Vec<&StructuredMessage> {
        self.structured_messages
            .iter()
            .rev()
            .take(n)
            .rev()
            .collect()
    }

    /// 消息总数。
    pub fn message_count(&self) -> usize {
        self.structured_messages.len()
    }

    // 转换为 Session（待 Task 7 完成 Session 类型更新后恢复）。
    // pub fn to_session(&self) -> Session {
    //     let mut session = Session::new(&self.session_id);
    //     session.messages = self.structured_messages.clone();
    //     session
    // }

    /// 清理过期消息。
    pub fn cleanup(&mut self) {
        self.trim_conversation();
    }

    fn trim_conversation(&mut self) {
        if self.structured_messages.len() <= MAX_CONVERSATION_MESSAGES {
            return;
        }
        let keep_from = self
            .structured_messages
            .len()
            .saturating_sub(KEEP_RECENT_MESSAGES);
        if keep_from > 0 {
            self.structured_messages.drain(0..keep_from);
        }
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
        assert!(state.structured_messages.is_empty());
        assert!(state.current_goal.is_none());
    }

    #[test]
    fn test_session_state_add_messages() {
        let mut state = SessionState::new("test-session");
        state.add_user_message("Hello");
        assert_eq!(state.structured_messages.len(), 1);
        assert_eq!(state.structured_messages[0].role, MessageRole::User);
    }

    #[test]
    fn test_session_state_cleanup() {
        let mut state = SessionState::new("test-session");
        for i in 0..101 {
            state.add_user_message(format!("Message {}", i));
        }
        state.cleanup();
        assert!(state.structured_messages.len() <= KEEP_RECENT_MESSAGES);
        assert!(!state.structured_messages.is_empty());
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
