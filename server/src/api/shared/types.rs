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
    /// 消息 ID（服务端 msg_xxx；回退/重做的定位键——前端索引与服务端列表
    /// 错位时按 ID 删除；流式本地消息经 user_message_id 事件同步后获得真实 ID）。
    /// 请求方向（ChatRequest.message）不携带；响应方向历史消息必有、
    /// 错误/非流式响应为 None（序列化时跳过）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
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
    /// 输出被截断（finish=length：token 上限保留部分输出）。
    /// 流式事件经 finish_reason 标记；历史消息由 StructuredMessage.finish 映射。
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub truncated_by_length: bool,
    /// 流式中断（finish=interrupted：网络/服务中断保留部分输出；区别于
    /// token 上限截断——配置的 max_tokens 远未触顶时不该显示"已达上限"）。
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub interrupted: bool,
    /// 本条消息的 token 用量（历史消息由持久化 usage 映射；前端按会话独立
    /// 计算上下文占用 / 缓存命中）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<TokenUsage>,
    /// 可选的时间戳（RFC3339 格式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// 消息时间线段（历史与流式共用同一渲染管线）：由
    /// StructuredMessage.parts 的顺序生成——思考/正文/工具调用的真实
    /// 到达顺序（方案 B：后端权威时间线，消除历史/流式显示分叉）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub segments: Option<Vec<MessageSegment>>,
}

/// 消息时间线段（与前端 MessageSegment 类型对齐：type=thinking/text/tool）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageSegment {
    /// 思考增量。
    Thinking {
        /// 思考文本。
        text: String,
    },
    /// 正文增量。
    Text {
        /// 文本内容。
        text: String,
    },
    /// 工具调用段（id 关联消息级 tool_calls 卡片；前端按 id 合并执行结果）。
    Tool {
        /// 工具调用信息。
        tool_call: SegmentToolCall,
    },
}

/// 工具调用段内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentToolCall {
    /// 调用 ID（关联 ToolCallWithResult.id）。
    pub id: String,
    /// 工具名称。
    pub name: String,
    /// 调用参数 JSON。
    pub arguments: String,
    /// 展示意图（A2；历史/流式一致的卡片图标与标签）。
    pub presentation: String,
}

impl ChatMessage {
    /// 从结构化消息的 parts 生成时间线段（历史加载与流式边界事件共用；
    /// 顺序 = 持久化 parts 顺序 = 真实到达顺序）。
    ///
    /// - Part::Reasoning → Thinking 段
    /// - Part::Text → Text 段
    /// - Part::ToolCall → Tool 段（结果挂到 tool_calls 卡片，不产生段）
    /// - Part::ToolResult / Part::Image → 不产生段（结果合并/图片独立渲染）
    pub(crate) fn segments_from_parts(
        role: &MessageRole,
        parts: &[tianyan::common::types::Part],
    ) -> Option<Vec<MessageSegment>> {
        // 时间线语义只属于 assistant 消息（思考/工具调用仅 assistant 产生）
        if !matches!(role, MessageRole::Assistant) {
            return None;
        }
        let mut segments = Vec::new();
        for p in parts {
            match p {
                tianyan::common::types::Part::Reasoning { text, .. } => {
                    segments.push(MessageSegment::Thinking { text: text.clone() });
                }
                tianyan::common::types::Part::Text { text, .. } => {
                    segments.push(MessageSegment::Text { text: text.clone() });
                }
                tianyan::common::types::Part::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    segments.push(MessageSegment::Tool {
                        tool_call: SegmentToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                            presentation: tianyan::agent::ToolRegistry::default_presentation(name)
                                .as_str()
                                .to_string(),
                        },
                    });
                }
                _ => {}
            }
        }
        if segments.is_empty() {
            None
        } else {
            Some(segments)
        }
    }
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
    /// 执行耗时（毫秒；由 Part::ToolResult.time 差值推导，缺失时为 None）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<i64>,
    /// 执行失败原因（由 Part::ToolResult.error 透传；None = 成功）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl ChatMessage {
    /// 从核心消息构造 API 消息（流式边界事件用，与历史加载同构的轻量版）：
    /// id/role/content/thinking/images/usage/truncated/timestamp；
    /// tool_calls 不填——流式期间前端已累积完整工具卡片（含结果），
    /// 历史加载走 sessions 服务的完整转换（跨消息合并工具结果）。
    pub(crate) fn from_structured_light(m: &tianyan::common::types::StructuredMessage) -> Self {
        let mut thinking = String::new();
        let mut content = String::new();
        let mut images = Vec::new();
        for p in &m.parts {
            match p {
                tianyan::common::types::Part::Text { text, .. } => {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(text);
                }
                tianyan::common::types::Part::Reasoning { text, .. } => {
                    if !thinking.is_empty() {
                        thinking.push('\n');
                    }
                    thinking.push_str(text);
                }
                tianyan::common::types::Part::Image { url, .. } => images.push(url.clone()),
                _ => {}
            }
        }
        Self {
            id: Some(m.id.clone()),
            role: m.role,
            content,
            thinking: if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            tool_calls: None,
            images: if images.is_empty() {
                None
            } else {
                Some(images)
            },
            truncated_by_length: m.finish.as_deref() == Some("length"),
            interrupted: m.finish.as_deref() == Some("interrupted"),
            usage: (m.tokens.total > 0 || m.tokens.input > 0).then_some(TokenUsage {
                prompt_tokens: m.tokens.input as u32,
                completion_tokens: m.tokens.output as u32,
                total_tokens: m.tokens.total as u32,
                cache_read: m.tokens.cache.read as u32,
                cache_write: m.tokens.cache.write as u32,
            }),
            timestamp: None,
            // 时间线：与历史加载同构（方案 B）——流式边界事件携带服务端
            // 权威 segments，前端 applyServerMessage 以服务端为准
            segments: Self::segments_from_parts(&m.role, &m.parts),
        }
    }

    /// 创建新的系统消息
    ///
    /// # Arguments
    /// * `content` - 消息内容
    ///
    /// # Returns
    /// * `Self` - 新创建的系统消息
    pub fn system(content: &str) -> Self {
        Self {
            id: None,
            role: MessageRole::System,
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            interrupted: false,
            usage: None,
            timestamp: None,
            segments: None,
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
            id: None,
            role: MessageRole::User,
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            interrupted: false,
            usage: None,
            timestamp: None,
            segments: None,
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
            id: None,
            role: MessageRole::Assistant,
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            interrupted: false,
            usage: None,
            timestamp: None,
            segments: None,
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
    /// 缓存命中（读取）token 数（提供商返回缓存明细时才有意义，否则为 0）。
    #[serde(default)]
    pub cache_read: u32,
    /// 缓存写入 token 数（提供商一般不下发，保持 0）。
    #[serde(default)]
    pub cache_write: u32,
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
            cache_read: 0,
            cache_write: 0,
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
            cache_read: 0,
            cache_write: 0,
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
    fn test_segments_from_parts_preserves_order() {
        use tianyan::common::types::{MessageRole, Part, StructuredMessage};
        let mut sm = StructuredMessage::system("sess-1", "占位");
        sm.role = MessageRole::Assistant;
        sm.parts = vec![
            Part::Reasoning {
                text: "先想一步".to_string(),
                time: Default::default(),
            },
            Part::Text {
                text: "工具前的正文".to_string(),
                time: Default::default(),
            },
            Part::ToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
                time: Default::default(),
            },
            Part::Text {
                text: "工具后的正文".to_string(),
                time: Default::default(),
            },
        ];
        let segments = ChatMessage::segments_from_parts(&sm.role, &sm.parts).expect("segments");
        // 顺序 = parts 顺序（真实到达顺序），不按类型重排
        assert_eq!(segments.len(), 4);
        match &segments[0] {
            MessageSegment::Thinking { text } => assert_eq!(text, "先想一步"),
            _ => panic!("expected thinking"),
        }
        match &segments[1] {
            MessageSegment::Text { text } => assert_eq!(text, "工具前的正文"),
            _ => panic!("expected text"),
        }
        match &segments[2] {
            MessageSegment::Tool { tool_call } => {
                assert_eq!(tool_call.id, "call-1");
                assert_eq!(tool_call.name, "read_file");
                assert_eq!(tool_call.presentation, "read");
            }
            _ => panic!("expected tool"),
        }
        match &segments[3] {
            MessageSegment::Text { text } => assert_eq!(text, "工具后的正文"),
            _ => panic!("expected text"),
        }
        // 序列化契约：type 标签 snake_case（对齐前端 MessageSegment）
        let json = serde_json::to_string(&segments).unwrap();
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("\"type\":\"tool\""));
    }

    #[test]
    fn test_segments_none_for_plain_messages() {
        let sm = tianyan::common::types::StructuredMessage::system("sess-1", "系统消息");
        assert!(ChatMessage::segments_from_parts(&sm.role, &sm.parts).is_none());
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
