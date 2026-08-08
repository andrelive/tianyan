use serde::{Deserialize, Serialize};

use crate::api::shared::types::ChatMessage;

/// 会话压缩响应。
#[derive(Debug, Serialize)]
pub struct CompressSessionResponse {
    /// 是否实际发生了压缩（消息不足 / token 未超阈值时为 false）。
    pub compressed: bool,
}

/// 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// 会话标识
    pub id: String,
    /// 会话标题
    pub title: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 消息数量
    pub message_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 会话元数据
    pub metadata: Option<SessionMetadata>,
}

/// 会话元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 使用的模型名称
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 标签列表
    pub tags: Option<Vec<String>>,
}

/// 列出会话响应
#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    /// 会话列表
    pub sessions: Vec<Session>,
    /// 会话总数
    pub total: usize,
}

/// 会话消息响应
#[derive(Debug, Serialize)]
pub struct SessionMessagesResponse {
    /// 会话标识
    pub session_id: String,
    /// 消息列表
    pub messages: Vec<ChatMessage>,
}

/// 会话详情响应
#[derive(Debug, Serialize)]
pub struct SessionDetail {
    /// 会话标识
    pub id: String,
    /// 会话标题
    pub title: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 消息列表
    pub messages: Vec<ChatMessage>,
}

/// 删除会话响应
#[derive(Debug, Serialize)]
pub struct DeleteSessionResponse {
    /// 是否成功
    pub success: bool,
    /// 响应消息
    pub message: String,
}

/// 更新标题请求
#[derive(Debug, Deserialize)]
pub struct UpdateTitleRequest {
    /// 新标题
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

/// 删除消息请求 —— 删除指定索引的消息及其后的所有消息。
#[derive(Debug, Deserialize)]
pub struct DeleteMessageRequest {
    /// 要删除的消息索引（该消息及其后的消息都会被删除）。
    pub message_index: usize,
}

/// 重做请求 —— 恢复被回退的消息与工作区文件。
#[derive(Debug, Deserialize)]
pub struct RedoRequest {
    /// 回退时的消息索引。
    pub message_index: usize,
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
}
