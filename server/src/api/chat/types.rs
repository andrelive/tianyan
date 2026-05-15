use serde::{Deserialize, Serialize};

use crate::api::shared::types::{ChatMessage, MessageRole, TokenUsage};

/// 对话完成请求
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub session_id: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_max_tokens() -> u32 {
    2048
}

impl ChatRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref id) = self.session_id {
            if id.trim().is_empty() {
                return Err("session_id 不能为空".to_string());
            }
        }
        if self.messages.is_empty() {
            return Err("messages 不能为空".to_string());
        }
        if self.messages.iter().any(|m| m.content.trim().is_empty()) {
            return Err("消息内容不能为空".to_string());
        }
        if !(0.0..=2.0).contains(&self.temperature) {
            return Err("temperature 必须在 0.0 到 2.0 之间".to_string());
        }
        if self.max_tokens == 0 || self.max_tokens > 32_768 {
            return Err("max_tokens 必须在 1 到 32768 之间".to_string());
        }
        Ok(())
    }
}

/// 对话完成响应（非流式）
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub id: String,
    pub session_id: String,
    pub message: ChatMessage,
    pub usage: TokenUsage,
}

impl ChatResponse {
    /// 创建错误响应
    pub fn error(message: &str) -> Self {
        Self {
            id: format!("chatcmpl-{}", crate::api::shared::short_uuid()),
            session_id: "error".to_string(),
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: message.to_string(),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            },
            usage: TokenUsage::empty(),
        }
    }
}

/// 技能调用信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCallInfo {
    pub skill_id: String,
    pub skill_name: String,
    pub success: bool,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

/// 重新生成消息请求
#[derive(Debug, Deserialize)]
pub struct RegenerateRequest {
    pub session_id: String,
    pub message_index: usize,
}

impl RegenerateRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if self.session_id.trim().is_empty() {
            return Err("session_id 不能为空".to_string());
        }
        Ok(())
    }
}

/// 编辑消息请求
#[derive(Debug, Deserialize)]
pub struct EditMessageRequest {
    pub session_id: String,
    pub message_index: usize,
    pub new_content: String,
}

impl EditMessageRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if self.session_id.trim().is_empty() {
            return Err("session_id 不能为空".to_string());
        }
        if self.new_content.trim().is_empty() {
            return Err("新消息内容不能为空".to_string());
        }
        if self.new_content.len() > 10_000 {
            return Err("消息内容长度不能超过 10000 个字符".to_string());
        }
        Ok(())
    }
}

/// SSE 流式事件
#[derive(Debug, Serialize)]
pub struct ChatStreamEvent {
    pub id: String,
    pub session_id: String,
    pub delta: String,
    pub finish_reason: Option<String>,
    pub chunk_type: tianyan::agent::StreamChunkType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_calls: Option<Vec<SkillCallInfo>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::shared::types::MessageRole;

    #[test]
    fn test_chat_request_deserialization() {
        let json = r#"{
            "session_id": "test-session",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": false,
            "temperature": 0.7,
            "max_tokens": 100
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.session_id, Some("test-session".to_string()));
        assert_eq!(req.messages.len(), 1);
        assert!(!req.stream);
    }

    #[test]
    fn test_chat_response_serialization() {
        let response = ChatResponse {
            id: "chatcmpl-123".to_string(),
            session_id: "session-456".to_string(),
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: "Hello!".to_string(),
                timestamp: Some("2026-02-20T10:00:00Z".to_string()),
            },
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("chatcmpl-123"));
        assert!(json.contains("Hello!"));
    }

    #[test]
    fn test_chat_stream_event_serialization() {
        let event = ChatStreamEvent {
            id: "chatcmpl-1".to_string(),
            session_id: "session-123".to_string(),
            delta: "Hello".to_string(),
            finish_reason: None,
            chunk_type: tianyan::agent::StreamChunkType::Answer,
            skill_calls: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Hello"));
        assert!(json.contains("chatcmpl-1"));
    }
}
