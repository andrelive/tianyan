//! 天演智能体系统的会话类型定义。
//!
//! 本模块定义会话相关的核心数据结构。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::common::types::{InjectableContext, StructuredMessage, TianyanUri};

/// 会话 JSONL 首行的会话级状态头部。
///
/// 与消息行区分：`session_header` 标记恒为 1；解析时据此识别头部行
/// （StructuredMessage 不含该字段，天然互斥）。旧格式会话（首行即消息）
/// 兼容加载，仅在下次重写时补齐头部。
///
/// 当前承载 `injectable_snapshot`（soul/rules/memories 前缀快照）：
/// 前缀内容随会话固化——重启后旧会话沿用同一份快照，不重新检索，
/// 保证前缀内容与重启前一致（prompt 缓存不失效、语义不漂移）；
/// 仅在会话首次加载（无快照）与压缩点（会话转换）更新。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    /// 头部标记（恒为 1；用于与消息行区分）。
    pub session_header: Option<u8>,
    /// 注入上下文快照（首次加载时固化，压缩点刷新）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injectable_snapshot: Option<InjectableContext>,
}

impl Default for SessionHeader {
    fn default() -> Self {
        Self {
            session_header: Some(Self::MARKER),
            injectable_snapshot: None,
        }
    }
}

impl SessionHeader {
    /// 头部标记值。
    pub const MARKER: u8 = 1;

    /// 判断该 JSONL 行是否为会话头部行。
    pub fn parse_line(line: &str) -> Option<Self> {
        serde_json::from_str::<SessionHeader>(line)
            .ok()
            .filter(|h| h.session_header == Some(Self::MARKER))
    }

    /// 是否为空头部（无任何快照载荷）。
    pub fn is_empty(&self) -> bool {
        self.injectable_snapshot.is_none()
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
    pub messages: Vec<StructuredMessage>,
    /// 会话摘要（会话结束后生成）。
    pub summary: Option<String>,
    /// 会话标题（可选）。
    pub title: Option<String>,
    /// 会话级状态头部（JSONL 首行，存注入上下文快照）。
    #[serde(default)]
    pub header: SessionHeader,
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
            header: SessionHeader::default(),
        }
    }

    /// 使用生成的 ID 创建新会话。
    pub fn new_with_generated_id() -> Self {
        let id = format!("session_{}", Utc::now().format("%Y%m%d_%H%M%S_%f"));
        Self::new(id)
    }

    /// 向会话添加结构化消息。
    pub fn add_structured_message(&mut self, msg: StructuredMessage) {
        self.messages.push(msg);
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
    use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime};

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
        session.add_structured_message(StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![Part::Text {
                text: "你好".to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "test-session".to_string(),
            finish: None,
            compression_marker: false,
        });
        assert_eq!(session.message_count(), 1);
    }

    #[test]
    fn test_session_end() {
        let mut session = Session::new("test-session");
        session.end();
        assert!(session.is_ended());
        assert!(session.duration_seconds().is_some());
    }

    #[test]
    fn test_session_uri() {
        let session = Session::new("test-123");
        let uri = session.uri();
        assert_eq!(uri.to_string(), "tianyan://session/test-123");
    }
}
