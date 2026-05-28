use serde::{Deserialize, Serialize};

use super::tool::ToolCall;

/// 对话中的消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MessageRole {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::System => write!(f, "system"),
            MessageRole::User => write!(f, "user"),
            MessageRole::Assistant => write!(f, "assistant"),
            MessageRole::Tool => write!(f, "tool"),
        }
    }
}

/// 对话中的一条消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息发送者的角色
    pub role: MessageRole,
    /// 消息内容
    pub content: String,
    /// 当 role 为 Assistant 时，模型发出的工具调用列表。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 当 role 为 Tool 时，对应哪个 tool_call 的 ID。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// 模型推理内容（reasoning_content），仅 DeepSeek 等支持思维链的模型使用。
    /// 上一轮 assistant 含 tool_calls 时，下一轮必须原样保留。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

impl Message {
    /// 创建新消息。
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// 创建系统消息。
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(MessageRole::System, content)
    }

    /// 创建用户消息。
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(MessageRole::User, content)
    }

    /// 创建助手消息。
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    /// 创建携带工具调用的助手消息。
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// 创建工具结果消息。
    ///
    /// - `tool_call_id` - 对应 Assistant 消息中 `ToolCall.id`
    /// - `content` - 工具执行结果（通常为 JSON 字符串）
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tool::{FunctionCall, ToolCallType};
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message::user("Hello, world!");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello, world!");
    }

    #[test]
    fn test_message_system() {
        let msg = Message::system("System prompt");
        assert_eq!(msg.role, MessageRole::System);
        assert_eq!(msg.content, "System prompt");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("Assistant response");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Assistant response");
    }

    #[test]
    fn test_message_role_display() {
        assert_eq!(format!("{}", MessageRole::System), "system");
        assert_eq!(format!("{}", MessageRole::User), "user");
        assert_eq!(format!("{}", MessageRole::Assistant), "assistant");
        assert_eq!(format!("{}", MessageRole::Tool), "tool");
    }

    #[test]
    fn test_message_role_default() {
        let role = MessageRole::default();
        assert_eq!(role, MessageRole::User);
    }

    #[test]
    fn test_message_tool_call_roundtrip() {
        let msg = Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_123".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "search".to_string(),
                    arguments: r#"{"q":"hello"}"#.to_string(),
                },
            }],
        );
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, MessageRole::Assistant);
        assert!(deserialized.tool_calls.is_some());
        assert_eq!(deserialized.tool_calls.as_ref().unwrap()[0].id, "call_123");
    }

    #[test]
    fn test_message_tool_role() {
        let msg = Message::tool("call_123", r#"{"result":"ok"}"#);
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn test_message_backward_compat() {
        // Old-format JSON (no tool_calls / tool_call_id) should deserialize
        let json = r#"{"role":"user","content":"hello"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "hello");
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
    }
}
