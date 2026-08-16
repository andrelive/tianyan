//! 共享类型定义
//!
//! 本模块包含 API 层通用的数据类型，可在各个 API 子模块间复用。

use serde::{Deserialize, Serialize};

/// 生成带 `session-` 前缀的会话 ID（完整 UUID）。
pub fn short_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 聊天消息角色
///
/// 复用 core 的 [`tianyan::common::types::MessageRole`]（单一真相源），
/// 避免 API 层重复定义导致变体漂移（core 含 Tool 变体）。
pub use tianyan::common::types::MessageRole;

/// 聊天消息
///
/// 表示对话中的单条消息，包含角色、内容和可选的时间戳。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 消息角色
    pub role: MessageRole,
    /// 消息内容
    pub content: String,
    /// 思考过程文本（模型 reasoning；正文在 content，前端分开渲染）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking: Option<String>,
    /// 工具调用卡片（A2 展示契约：名称 + 参数 + 展示意图 + 对应执行结果，
    /// 调用与结果合并渲染；历史消息由 parts 转换，结果为完整内容不截断）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCallWithResult>>,
    /// 图片 data URL 列表（`data:image/png;base64,...`），仅用户消息使用。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
    /// 输出被截断（finish=length：token 上限或流式中断保留部分输出）。
    /// 流式事件经 finish_reason 标记；历史消息由 StructuredMessage.finish 映射。
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub truncated_by_length: bool,
    /// 可选的时间戳（RFC3339 格式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// 历史消息中的工具调用卡片（调用信息与对应执行结果合并渲染）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallWithResult {
    /// 工具调用 ID（关联工具结果）。
    pub id: String,
    /// 工具名称。
    pub name: String,
    /// 参数 JSON 字符串（原始 arguments）。
    pub arguments: String,
    /// 展示意图（A2：generic/read/write/terminal/diff/search/web/skill/knowledge/delegate/code）。
    pub presentation: String,
    /// 对应工具执行结果（完整内容不截断；无结果时为 None）。
    pub result: Option<String>,
}

impl ChatMessage {
    /// 创建新的系统消息
    ///
    /// # Arguments
    /// * `content` - 消息内容
    ///
    /// # Returns
    /// * `Self` - 新创建的系统消息
    pub fn system(content: &str) -> Self {
        Self {
            role: MessageRole::System,
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            timestamp: None,
        }
    }

    /// 创建新的用户消息
    ///
    /// # Arguments
    /// * `content` - 消息内容
    ///
    /// # Returns
    /// * `Self` - 新创建的用户消息
    pub fn user(content: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            timestamp: None,
        }
    }

    /// 创建新的助手消息
    ///
    /// # Arguments
    /// * `content` - 消息内容
    ///
    /// # Returns
    /// * `Self` - 新创建的助手消息
    pub fn assistant(content: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            timestamp: None,
        }
    }
}

/// Token 使用统计
///
/// 记录模型调用时的 token 消耗情况。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// 提示词 token 数量
    pub prompt_tokens: u32,
    /// 完成 token 数量
    pub completion_tokens: u32,
    /// 总 token 数量
    pub total_tokens: u32,
}

impl TokenUsage {
    /// 创建空的 TokenUsage
    ///
    /// # Returns
    /// * `Self` - 所有计数为 0 的 TokenUsage
    pub fn empty() -> Self {
        Self {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }

    /// 创建新的 TokenUsage
    ///
    /// # Arguments
    /// * `prompt_tokens` - 提示词 token 数量
    /// * `completion_tokens` - 完成 token 数量
    /// * `total_tokens` - 总 token 数量
    ///
    /// # Returns
    /// * `Self` - 新创建的 TokenUsage
    pub fn new(prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_role_serialization() {
        assert_eq!(
            serde_json::to_string(&MessageRole::System).unwrap(),
            "\"system\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::User).unwrap(),
            "\"user\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::Assistant).unwrap(),
            "\"assistant\""
        );
    }

    #[test]
    fn test_chat_message_serialization() {
        let msg = ChatMessage::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("user"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_chat_message_with_timestamp() {
        let mut msg = ChatMessage::assistant("Response");
        msg.timestamp = Some("2026-03-17T10:00:00Z".to_string());
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("2026-03-17T10:00:00Z"));
    }

    #[test]
    fn test_token_usage_empty() {
        let usage = TokenUsage::empty();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn test_token_usage_new() {
        let usage = TokenUsage::new(100, 50, 150);
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }
}
