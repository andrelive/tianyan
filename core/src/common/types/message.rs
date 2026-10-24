use serde::{Deserialize, Serialize};

use super::content_part::ContentPart;
use super::tool::ToolCall;

/// 对话中的消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MessageRole {
    /// 系统角色。
    System,
    /// 用户角色。
    #[default]
    User,
    /// 助手角色。
    Assistant,
    /// 工具角色。
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
    /// 多模态内容片段（用户消息携带图片时使用；ADR-050 之后 tool 结果携带图片
    /// 时也用它——`content` 保持为纯文本（可为空），片段中的图片对 LLM 可见）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content_parts: Option<Vec<ContentPart>>,
    /// 当 role 为 Assistant 时，模型发出的工具调用列表。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 当 role 为 Tool 时，对应哪个 tool_call 的 ID。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// 工具执行耗时（毫秒；仅 role 为 Tool 时使用，消息级计时元数据）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_duration_ms: Option<i64>,
    /// 工具执行失败原因（仅 role 为 Tool 时使用；None = 成功）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_error: Option<String>,
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
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_duration_ms: None,
            tool_error: None,
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

    /// 创建携带图片的用户消息。
    ///
    /// - `content` - 用户文本（可为空，此时由图片充当全部内容）
    /// - `images` - 图片 data URL 列表（`data:image/png;base64,...`），
    ///   按顺序排在文本片段之后。
    pub fn user_with_images(content: impl Into<String>, images: Vec<String>) -> Self {
        let mut parts: Vec<ContentPart> = Vec::new();
        let content = content.into();
        if !content.is_empty() {
            parts.push(ContentPart::text(&content));
        }
        parts.extend(images.into_iter().map(ContentPart::image));
        Self {
            role: MessageRole::User,
            content,
            content_parts: Some(parts),
            tool_calls: None,
            tool_call_id: None,
            tool_duration_ms: None,
            tool_error: None,
            reasoning_content: None,
        }
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
            content_parts: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            tool_duration_ms: None,
            tool_error: None,
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
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            tool_duration_ms: None,
            tool_error: None,
            reasoning_content: None,
        }
    }

    /// 创建携带执行元数据的工具结果消息（消息级计时/成败信息，
    /// 持久化后供历史回放展示——Part::ToolResult 的 time 与 error 输入源）。
    ///
    /// - `tool_call_id` - 对应 Assistant 消息中 `ToolCall.id`
    /// - `content` - 工具执行结果（通常为 JSON 字符串）
    /// - `duration_ms` - 执行耗时（毫秒；None 表示未知）
    /// - `error` - 失败原因（None = 成功）
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        duration_ms: Option<i64>,
        error: Option<String>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            tool_duration_ms: duration_ms,
            tool_error: error,
            reasoning_content: None,
        }
    }

    /// 创建携带图片的工具结果消息（ADR-050：tool 结果直通图片）。
    /// `content_parts = [text(content), image(…), …]`（content 是 tool 结果原文，
    /// 空时也保留 text 片段——tool 消息必须有 content）。
    ///
    /// - `tool_call_id` - 对应 Assistant 消息中 `ToolCall.id`
    /// - `content` - 工具执行结果原文（通常为 JSON 字符串）
    /// - `images` - 图片 data URL 列表（`data:image/png;base64,...`），按顺序
    ///   排在文本片段之后
    pub fn tool_with_images(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        images: Vec<String>,
    ) -> Self {
        let mut msg = Self::tool(tool_call_id, content);
        msg.attach_images(images);
        msg
    }

    /// 给消息附加图片片段（ADR-050）：`content_parts = [text(self.content), image(…)]`。
    ///
    /// **形状单点**：本轮工具循环直接复用消息 Vec（不经 `assemble`），
    /// `SessionImages::hydrate` 因此在写入 Vec 前调本方法；组装层的
    /// `tool_with_images` 也委托本方法——两处必须逐字节一致，否则同一段历史
    /// 在"本轮"与"下一轮重新组装"之间会产生前缀分叉。
    ///
    /// `images` 为空时不改动消息（纯文本工具结果保持原样）。
    pub fn attach_images(&mut self, images: Vec<String>) {
        if images.is_empty() {
            return;
        }
        let mut parts: Vec<ContentPart> = Vec::with_capacity(images.len() + 1);
        parts.push(ContentPart::text(&self.content));
        parts.extend(images.into_iter().map(ContentPart::image));
        self.content_parts = Some(parts);
    }

    /// 提取多模态片段中的图片 URL 列表（无则返回空）。
    pub fn image_urls(&self) -> Vec<String> {
        self.content_parts
            .as_ref()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.image_url.as_ref().map(|u| u.url.clone()))
                    .collect()
            })
            .unwrap_or_default()
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
        assert!(msg.content_parts.is_none(), "旧格式 JSON 无 content_parts");
    }

    #[test]
    fn test_message_user_with_images_creates_parts() {
        let msg =
            Message::user_with_images("看这张图", vec!["data:image/png;base64,AAAA".to_string()]);
        assert_eq!(msg.content, "看这张图");
        let parts = msg.content_parts.as_ref().expect("应携带 content_parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].content_type, "text");
        assert_eq!(parts[0].text.as_deref(), Some("看这张图"));
        assert_eq!(parts[1].content_type, "image_url");
        assert_eq!(
            parts[1].image_url.as_ref().unwrap().url,
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn test_message_user_with_images_empty_text_only_images() {
        // 文本为空时仅生成图片片段（图片消息以图为主体）
        let msg = Message::user_with_images("", vec!["data:image/png;base64,BBBB".to_string()]);
        assert_eq!(msg.content, "");
        let parts = msg.content_parts.as_ref().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].content_type, "image_url");
    }

    #[test]
    fn test_message_image_urls_extraction() {
        let plain = Message::user("hi");
        assert!(plain.image_urls().is_empty());

        let with_images = Message::user_with_images(
            "text",
            vec![
                "data:image/png;base64,1".to_string(),
                "data:image/jpeg;base64,2".to_string(),
            ],
        );
        assert_eq!(with_images.image_urls().len(), 2);
    }

    #[test]
    fn test_message_content_parts_roundtrip() {
        let msg =
            Message::user_with_images("hello", vec!["data:image/png;base64,AAAA".to_string()]);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.content, "hello");
        assert_eq!(restored.image_urls(), vec!["data:image/png;base64,AAAA"]);
    }

    #[test]
    fn test_message_tool_with_images_keeps_text_and_image_parts() {
        // ADR-050：tool 结果直通图片——原文是 text 片段，图片按序排在后面
        let original = r#"{"image":"session_images/a.png","mime":"image/png"}"#;
        let msg = Message::tool_with_images(
            "call_img",
            original,
            vec![
                "data:image/png;base64,AAAA".to_string(),
                "data:image/jpeg;base64,BBBB".to_string(),
            ],
        );
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_img"));
        assert_eq!(msg.content, original, "content 保留 tool 结果原文");
        let parts = msg.content_parts.as_ref().expect("应携带 content_parts");
        assert_eq!(parts.len(), 3, "1 个文本片段 + 2 个图片片段");
        assert_eq!(parts[0].content_type, "text");
        assert_eq!(parts[0].text.as_deref(), Some(original));
        assert_eq!(
            msg.image_urls(),
            vec![
                "data:image/png;base64,AAAA".to_string(),
                "data:image/jpeg;base64,BBBB".to_string()
            ]
        );
    }

    #[test]
    fn test_message_tool_with_images_empty_content_keeps_text_part() {
        // tool 消息必须有 content：原文为空时也保留 text 片段
        let msg =
            Message::tool_with_images("call_x", "", vec!["data:image/png;base64,CCCC".to_string()]);
        let parts = msg.content_parts.as_ref().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].content_type, "text");
        assert_eq!(parts[0].text.as_deref(), Some(""));
        assert_eq!(parts[1].content_type, "image_url");
    }
}
