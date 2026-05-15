use serde::{Deserialize, Serialize};

use crate::api::shared::types::ChatMessage;

/// 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadata>,
}

/// 会话元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// 创建会话请求
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default = "default_session_title")]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<String>,
}

fn default_session_title() -> String {
    "新对话".to_string()
}

impl CreateSessionRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        let trimmed = self.title.trim();
        if trimmed.is_empty() {
            return Err("标题不能为空".to_string());
        }
        if trimmed.len() > 100 {
            return Err("标题长度不能超过 100 个字符".to_string());
        }
        if let Some(ref msg) = self.initial_message {
            if msg.len() > 10_000 {
                return Err("初始消息长度不能超过 10000 个字符".to_string());
            }
        }
        Ok(())
    }
}

/// 创建会话响应
#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session: Session,
}

/// 列出会话响应
#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<Session>,
    pub total: usize,
}

/// 会话消息响应
#[derive(Debug, Serialize)]
pub struct SessionMessagesResponse {
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
}

/// 会话详情响应
#[derive(Debug, Serialize)]
pub struct SessionDetail {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<ChatMessage>,
}

/// 删除会话响应
#[derive(Debug, Serialize)]
pub struct DeleteSessionResponse {
    pub success: bool,
    pub message: String,
}

/// 更新标题请求
#[derive(Debug, Deserialize)]
pub struct UpdateTitleRequest {
    pub title: String,
}

impl UpdateTitleRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        let trimmed = self.title.trim();
        if trimmed.is_empty() {
            return Err("标题不能为空".to_string());
        }
        if trimmed.len() > 100 {
            return Err("标题长度不能超过 100 个字符".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_serialization() {
        let session = Session {
            id: "test-123".to_string(),
            title: "测试会话".to_string(),
            created_at: "2026-02-20T10:00:00Z".to_string(),
            updated_at: "2026-02-20T10:30:00Z".to_string(),
            message_count: 5,
            metadata: None,
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("测试会话"));
    }

    #[test]
    fn test_create_session_request_deserialization() {
        let json = r#"{"title": "我的会话", "initial_message": "Hello"}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, "我的会话");
        assert_eq!(req.initial_message, Some("Hello".to_string()));
    }

    #[test]
    fn test_create_session_request_default_title() {
        let json = r#"{}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, "新对话");
    }
}
