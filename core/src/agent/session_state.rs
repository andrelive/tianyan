use std::time::Instant;

use crate::common::types::{InjectableContext, StructuredMessage};

/// 会话状态容器（conversation 为唯一真相源）。
#[derive(Debug, Clone)]
pub struct SessionState {
    /// 会话 ID。
    pub session_id: String,
    /// 结构化消息（持久化格式）。
    pub structured_messages: Vec<StructuredMessage>,
    /// 可注入的上下文（由 ContextPipeline 填充）。
    pub injectable_context: InjectableContext,
    /// 最后活动时间。
    pub last_activity: Instant,
    /// 待持久化记忆。
    pub pending_memories: Vec<crate::common::types::MemoryEntry>,
}

impl SessionState {
    /// 创建新的会话状态。
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            structured_messages: Vec::new(),
            injectable_context: InjectableContext::new(),
            last_activity: Instant::now(),
            pending_memories: Vec::new(),
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
        let mut msg = StructuredMessage::user_with_images(self.session_id.clone(), content, images);
        // 维持消息链：父消息指向会话当前最后一条消息
        msg.parent_id = self.structured_messages.last().map(|m| m.id.clone());
        self.structured_messages.push(msg);
        self.last_activity = Instant::now();
    }

    /// 添加结构化消息（内存态与存储层一致：完整链，不裁剪）。
    ///
    /// 上下文预算由压缩机制（token 阈值 + 摘要替换）控制；组装层从
    /// 最后一个压缩点开始组装，内存态保留完整链供回退/重做/快照索引。
    pub fn add_structured_message(&mut self, msg: StructuredMessage) {
        self.structured_messages.push(msg);
        self.last_activity = Instant::now();
    }

    /// 消息总数。
    pub fn message_count(&self) -> usize {
        self.structured_messages.len()
    }

    /// 清理过期消息（无操作：内存态不裁剪，完整链保留）。
    pub fn cleanup(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{MessageRole, MessageTime, Part, PartTime};

    #[test]
    fn test_session_state_new() {
        let state = SessionState::new("test-session");
        assert_eq!(state.session_id, "test-session");
        assert!(state.structured_messages.is_empty());
    }

    #[test]
    fn test_session_state_add_messages() {
        let mut state = SessionState::new("test-session");
        state.add_user_message("Hello");
        assert_eq!(state.structured_messages.len(), 1);
        assert_eq!(state.structured_messages[0].role, MessageRole::User);
    }

    #[test]
    fn test_session_state_keeps_full_chain() {
        // 内存态不裁剪：完整链保留（回退/重做/快照索引依赖完整链）
        let mut state = SessionState::new("test-session");
        for i in 0..6000 {
            state.add_user_message(format!("Message {}", i));
        }
        state.cleanup();
        assert_eq!(state.structured_messages.len(), 6000, "内存态应保留完整链");
    }
}
