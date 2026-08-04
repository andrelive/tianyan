use std::time::Instant;

use chrono::Utc;

use crate::agent::types::ClarificationQuestion;
use crate::common::types::{
    DetailedTokenUsage, InjectableContext, MessageRole, MessageTime, Part, PartTime,
    StructuredMessage,
};

const MAX_CONVERSATION_MESSAGES: usize = 100;
const KEEP_RECENT_MESSAGES: usize = 50;

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

    /// 清理过期消息。
    pub fn cleanup(&mut self) {
        self.trim_conversation();
    }

    fn trim_conversation(&mut self) {
        if self.structured_messages.len() <= MAX_CONVERSATION_MESSAGES {
            return;
        }
        // 优先找到最近的 compression_marker 位置，避免截断标记
        let last_marker_pos = self
            .structured_messages
            .iter()
            .rposition(|m| m.compression_marker);
        let keep_from = self
            .structured_messages
            .len()
            .saturating_sub(KEEP_RECENT_MESSAGES);
        if keep_from == 0 {
            return;
        }
        // 如果有 compression_marker，只截断到 marker 之前，保留 marker 及之后所有消息
        if let Some(marker_pos) = last_marker_pos {
            if marker_pos < keep_from {
                self.structured_messages.drain(0..=marker_pos);
                return;
            }
        }
        // 无 marker 或 marker 在保留范围内：正常截断
        self.structured_messages.drain(0..keep_from);
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
}
