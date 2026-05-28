//! 天演智能体系统的会话类型定义。
//!
//! 本模块定义会话相关的核心数据结构。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::common::types::{Message, MessageRole, TianyanUri};

/// JSONL 格式的消息记录，用于会话持久化。
///
/// 此结构体专门用于 JSONL 文件格式，支持高效的追加写入。
/// 每条记录占一行，便于流式读取和写入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    /// 消息发送者的角色。
    pub role: MessageRole,
    /// 消息内容。
    pub content: String,
    /// 消息时间戳。
    pub timestamp: DateTime<Utc>,
}

impl MessageRecord {
    /// 创建新的消息记录。
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            timestamp: Utc::now(),
        }
    }

    /// 从 Message 转换为 MessageRecord。
    pub fn from_message(message: &Message) -> Self {
        Self {
            role: message.role,
            content: message.content.clone(),
            timestamp: Utc::now(),
        }
    }

    /// 转换为 Message 类型。
    pub fn to_message(&self) -> Message {
        Message {
            role: self.role,
            content: self.content.clone(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// 序列化为 JSONL 行（带换行符）。
    pub fn to_jsonl_line(&self) -> Result<String, serde_json::Error> {
        let mut json = serde_json::to_string(self)?;
        json.push('\n');
        Ok(json)
    }
}

/// 表示对话或交互的记忆会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// 唯一会话标识符。
    pub session_id: String,
    /// 会话创建时间戳。
    pub created_at: DateTime<Utc>,
    /// 会话结束时间戳（如果已结束）。
    pub ended_at: Option<DateTime<Utc>>,
    /// 会话中的消息。
    pub messages: Vec<Message>,
    /// 会话摘要（会话结束后生成）。
    pub summary: Option<String>,
    /// 会话标题（可选）。
    pub title: Option<String>,
}

impl Session {
    /// 使用给定 ID 创建新会话。
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            created_at: Utc::now(),
            ended_at: None,
            messages: Vec::new(),
            summary: None,
            title: None,
        }
    }

    /// 使用生成的 ID 创建新会话。
    pub fn new_with_generated_id() -> Self {
        let id = format!("session_{}", Utc::now().format("%Y%m%d_%H%M%S_%f"));
        Self::new(id)
    }

    /// 向会话添加消息。
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// 添加用户消息。
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(Message::user(content));
    }

    /// 添加助手消息。
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(Message::assistant(content));
    }

    /// 添加系统消息。
    pub fn add_system_message(&mut self, content: impl Into<String>) {
        self.messages.push(Message::system(content));
    }

    /// 结束会话。
    pub fn end(&mut self) {
        self.ended_at = Some(Utc::now());
    }

    /// 检查会话是否已结束。
    pub fn is_ended(&self) -> bool {
        self.ended_at.is_some()
    }

    /// 获取会话持续时间（秒）。
    pub fn duration_seconds(&self) -> Option<i64> {
        self.ended_at
            .map(|end| (end - self.created_at).num_seconds())
    }

    /// 获取消息数量。
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// 设置会话标题。
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// 获取此会话的 URI。
    pub fn uri(&self) -> TianyanUri {
        TianyanUri::new(
            crate::common::types::ContextNamespace::Session,
            vec![self.session_id.clone()],
        )
    }
}

/// 从会话中提取的关键信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyInfo {
    /// 识别到的用户偏好。
    pub preferences: Vec<ExtractedPreference>,
    /// 做出的重要决定。
    pub decisions: Vec<ExtractedDecision>,
    /// 提到的实体。
    pub entities: Vec<ExtractedEntity>,
    /// 讨论的主题。
    pub topics: Vec<String>,
    /// 识别到的行动项。
    pub action_items: Vec<String>,
}

/// 从对话中提取的偏好。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedPreference {
    /// 偏好键（例如 "coding_style"、"communication_style"）。
    pub key: String,
    /// 偏好值。
    pub value: String,
    /// 置信度分数（0.0 - 1.0）。
    pub confidence: f32,
    /// 来源消息索引。
    pub source_message_idx: Option<usize>,
}

/// 从对话中提取的决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedDecision {
    /// 决定描述。
    pub description: String,
    /// 决定的理由。
    pub rationale: Option<String>,
    /// 重要性分数（0.0 - 1.0）。
    pub importance: f32,
    /// 相关实体。
    pub related_entities: Vec<String>,
}

/// 从对话中提取的实体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    /// 实体名称。
    pub name: String,
    /// 实体类型（人物、项目、概念等）。
    pub entity_type: String,
    /// 关于实体的附加信息。
    pub info: Option<String>,
}

/// 会话元数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// 会话来源（cli、api 等）。
    pub source: Option<String>,
    /// 会话标签。
    pub tags: Vec<String>,
    /// 自定义元数据字段。
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = Session::new("test-session");
        assert_eq!(session.session_id, "test-session");
        assert!(!session.is_ended());
        assert_eq!(session.message_count(), 0);
    }

    #[test]
    fn test_session_messages() {
        let mut session = Session::new("test-session");
        session.add_user_message("你好");
        session.add_assistant_message("你好！");
        assert_eq!(session.message_count(), 2);
    }

    #[test]
    fn test_session_end() {
        let mut session = Session::new("test-session");
        session.end();
        assert!(session.is_ended());
        assert!(session.duration_seconds().is_some());
    }

    #[test]
    fn test_message_record() {
        let msg = Message::user("测试消息");
        let record = MessageRecord::from_message(&msg);
        assert_eq!(record.role, MessageRole::User);
        assert_eq!(record.content, "测试消息");
    }

    #[test]
    fn test_session_uri() {
        let session = Session::new("test-123");
        let uri = session.uri();
        assert_eq!(uri.to_string(), "tianyan://session/test-123");
    }
}
