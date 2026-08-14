use std::time::Instant;

use chrono::Utc;

use crate::agent::types::ClarificationQuestion;
use crate::common::types::{
    DetailedTokenUsage, InjectableContext, MessageRole, MessageTime, Part, PartTime,
    StructuredMessage,
};
use crate::session::{KEEP_RECENT_MESSAGES, MAX_SESSION_MESSAGES};

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
        self.push_user_message(content.into(), Vec::new());
    }

    /// 添加携带图片的用户消息（`images` 为 data URL 列表）。
    pub fn add_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        images: Vec<String>,
    ) {
        self.push_user_message(content.into(), images);
    }

    /// 用户消息内部构造：文本 + 可选图片 → 持久化 parts。
    fn push_user_message(&mut self, content: String, images: Vec<String>) {
        let mut parts = vec![Part::Text {
            text: content,
            time: PartTime::default(),
        }];
        for url in images {
            parts.push(Part::Image {
                url,
                time: PartTime::default(),
            });
        }
        let msg = StructuredMessage {
            id: format!("msg_{}", Utc::now().timestamp_millis()),
            parent_id: self.structured_messages.last().map(|m| m.id.clone()),
            role: MessageRole::User,
            parts,
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

    /// 消息总数。
    pub fn message_count(&self) -> usize {
        self.structured_messages.len()
    }

    /// 清理过期消息。
    pub fn cleanup(&mut self) {
        self.trim_conversation();
    }

    fn trim_conversation(&mut self) {
        if self.structured_messages.len() <= MAX_SESSION_MESSAGES {
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
        // 如果有 compression_marker，只截断到 marker 之前，保留 marker（摘要消息）及之后所有消息
        if let Some(marker_pos) = last_marker_pos {
            if marker_pos < keep_from {
                // 修复：原实现 drain(0..=marker_pos) 会把摘要消息本身一并删除，
                // 导致早期摘要从上下文消失；应为 drain(0..marker_pos)。
                self.structured_messages.drain(0..marker_pos);
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

    /// 回归测试：早期存在 compression_marker 时，裁剪必须保留摘要消息本身
    /// （修复 drain(0..=marker_pos) 误删 marker 的 bug）。
    #[test]
    fn test_trim_conversation_keeps_marker_message() {
        let mut state = SessionState::new("test-session");
        // 早期消息 + 摘要 marker
        for i in 0..30 {
            state.add_user_message(format!("early {}", i));
        }
        let marker = StructuredMessage {
            id: "msg_marker".to_string(),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![Part::Text {
                text: "[对话摘要] 早期对话已压缩".to_string(),
                time: PartTime::default(),
            }],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "test-session".to_string(),
            finish: None,
            compression_marker: true,
        };
        state.add_structured_message(marker);
        // 后续消息撑过 MAX_SESSION_MESSAGES，使 marker 落在保留窗口之前
        for i in 0..80 {
            state.add_user_message(format!("later {}", i));
        }
        state.cleanup();

        assert!(
            state
                .structured_messages
                .iter()
                .any(|m| m.compression_marker),
            "摘要消息（compression_marker）不应被裁剪掉"
        );
        // marker 之前的 early 消息必须全部删除；marker 之后的 later 消息全部保留
        assert!(
            state
                .structured_messages
                .iter()
                .all(|m| m.compression_marker || !m.parts.is_empty()),
            "裁剪后消息不应为空"
        );
        let texts: Vec<String> = state
            .structured_messages
            .iter()
            .flat_map(|m| {
                m.parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            texts.iter().all(|t| !t.starts_with("early ")),
            "marker 之前的消息应被删除，但残留: {:?}",
            texts
        );
        assert!(
            texts.iter().any(|t| t.starts_with("later ")),
            "marker 之后的消息应保留"
        );
    }
}
