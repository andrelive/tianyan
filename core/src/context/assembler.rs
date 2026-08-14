//! 上下文组装器。
//!
//! 负责将存储层的 StructuredMessage 和 InjectableContext 组装为
//! 传输层的 Vec<Message>，优化 DeepSeek 前缀缓存命中率。

use crate::common::types::InjectableContext;
use crate::common::types::{
    FunctionCall, Message, MessageRole, Part, StructuredMessage, TokenUsage,
    ToolCall as CoreToolCall, ToolCallType,
};

/// 上下文组装器——纯函数，无副作用。
pub struct ContextAssembler;

impl ContextAssembler {
    /// 将结构化消息和注入上下文组装为 LLM 传输格式。
    ///
    /// 输出顺序（缓存最优）：
    /// - messages[0]: injectable.soul（system）
    /// - messages[1]: rules + memories（system）
    /// - messages[2..N-1]: 历史对话
    /// - messages[N-1]: 当前用户输入（如果 current_input 非空）
    pub fn assemble(
        structured_messages: &[StructuredMessage],
        injectable: &InjectableContext,
        current_input: &str,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 注入 soul
        if !injectable.soul.is_empty() {
            messages.push(Message::system(&injectable.soul));
        }

        // 注入 rules + memories
        let mut context_parts: Vec<String> = Vec::new();
        if !injectable.rules_and_experiences.is_empty() {
            context_parts.push("## 经验与方法论\n".to_string());
            for rule in &injectable.rules_and_experiences {
                context_parts.push(format!("- {}\n", rule));
            }
        }
        if !injectable.memories.is_empty() {
            context_parts.push("## 用户画像与记忆\n".to_string());
            for mem in &injectable.memories {
                context_parts.push(format!("- {}\n", mem));
            }
        }
        if !context_parts.is_empty() {
            messages.push(Message::system(context_parts.concat()));
        }

        // 历史对话
        for sm in structured_messages {
            messages.extend(Self::structured_to_messages(sm));
        }

        // 当前用户输入
        if !current_input.is_empty() {
            messages.push(Message::user(current_input));
        }

        messages
    }

    /// 将单个 StructuredMessage 转换为 1~N 条传输层 Message。
    pub(crate) fn structured_to_messages(sm: &StructuredMessage) -> Vec<Message> {
        let mut messages = Vec::new();

        match sm.role {
            MessageRole::User => {
                let mut text = String::new();
                let mut images: Vec<String> = Vec::new();
                for part in &sm.parts {
                    match part {
                        Part::Text { text: t, .. } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        Part::Image { url, .. } => images.push(url.clone()),
                        _ => {}
                    }
                }
                if images.is_empty() {
                    if !text.is_empty() {
                        messages.push(Message::user(text));
                    }
                } else {
                    messages.push(Message::user_with_images(text, images));
                }
            }
            MessageRole::Assistant => {
                let has_tool_calls = sm.parts.iter().any(|p| matches!(p, Part::ToolCall { .. }));

                let mut content = String::new();
                let mut tool_calls: Vec<CoreToolCall> = Vec::new();
                let mut reasoning_content: Option<String> = None;

                for part in &sm.parts {
                    match part {
                        Part::Text { text, .. } => {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        }
                        Part::Reasoning { text, .. } => {
                            if has_tool_calls {
                                reasoning_content = Some(text.clone());
                            }
                        }
                        Part::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            tool_calls.push(CoreToolCall {
                                id: id.clone(),
                                call_type: ToolCallType::Function,
                                function: FunctionCall {
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                },
                            });
                        }
                        Part::ToolResult { .. } => {}
                        // Assistant 消息不应携带图片；异常数据静默忽略。
                        Part::Image { .. } => {}
                    }
                }

                if !content.is_empty() || !tool_calls.is_empty() {
                    let msg = Message {
                        role: MessageRole::Assistant,
                        content,
                        content_parts: None,
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                        tool_call_id: None,
                        reasoning_content,
                    };
                    messages.push(msg);
                }
            }
            MessageRole::Tool => {
                for part in &sm.parts {
                    if let Part::ToolResult {
                        tool_call_id,
                        content,
                        ..
                    } = part
                    {
                        messages.push(Message::tool(tool_call_id.clone(), content.clone()));
                    }
                }
            }
            MessageRole::System => {
                for part in &sm.parts {
                    if let Part::Text { text, .. } = part {
                        messages.push(Message::system(text.clone()));
                    }
                }
            }
        }

        messages
    }

    /// 将 LLM 返回的传输层 Message 转回 StructuredMessage（用于持久化）。
    ///
    /// 角色从 `msg.role` 直接派生。对于 Tool 消息，生成 `Part::ToolResult`；
    /// 对于其他角色，分别提取 reasoning content、text content 和 tool calls。
    /// `token_usage` 来自 LLM 响应的实际 token 统计，为 `None` 时使用默认值。
    ///
    /// 实现委托给 [`StructuredMessage::from_message`]——转换逻辑的唯一实现点
    /// （会话创建、历史重放等路径共用，保证图片等非文本内容全保真）。
    pub fn message_to_structured(
        msg: &Message,
        session_id: &str,
        parent_id: Option<&str>,
        token_usage: Option<TokenUsage>,
    ) -> StructuredMessage {
        StructuredMessage::from_message(msg, session_id, parent_id, token_usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::InjectableContext;
    use crate::common::types::{
        DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
    };

    fn make_text_msg(
        id: &str,
        role: MessageRole,
        text: &str,
        session_id: &str,
    ) -> StructuredMessage {
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role,
            parts: vec![Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: false,
        }
    }

    #[test]
    fn test_assemble_empty_session() {
        let injectable = InjectableContext {
            soul: "You are a helpful assistant.".to_string(),
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable, "Hello");
        assert_eq!(messages.len(), 2); // soul + current input
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "You are a helpful assistant.");
        assert_eq!(messages[1].role, MessageRole::User);
        assert_eq!(messages[1].content, "Hello");
    }

    #[test]
    fn test_assemble_with_history() {
        let injectable = InjectableContext::default();
        let sm = make_text_msg("msg_1", MessageRole::User, "Hi", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable, "Hello again");
        // 1 user history + 1 current input (no injectable, so no system messages)
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].content, "Hi");
        assert_eq!(messages[1].content, "Hello again");
    }

    #[test]
    fn test_structured_to_messages_reasoning_with_tool_call() {
        let sm = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::Reasoning {
                    text: "I need to read the file first".to_string(),
                    time: PartTime::default(),
                },
                Part::ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"foo.rs"}"#.to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
            compression_marker: false,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Assistant);
        assert!(msg.tool_calls.is_some());
        assert!(msg.reasoning_content.is_some());
        assert_eq!(
            msg.reasoning_content.as_ref().unwrap(),
            "I need to read the file first"
        );
    }

    #[test]
    fn test_structured_to_messages_reasoning_without_tool_call_is_discarded() {
        let sm = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::Reasoning {
                    text: "The answer is 42".to_string(),
                    time: PartTime::default(),
                },
                Part::Text {
                    text: "42".to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
            compression_marker: false,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "42");
        assert!(msg.reasoning_content.is_none());
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn test_message_to_structured_roundtrip() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: "Hello".to_string(),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };
        let sm = ContextAssembler::message_to_structured(&msg, "ses_1", None, None);
        assert_eq!(sm.role, MessageRole::Assistant);
        assert_eq!(sm.session_id, "ses_1");
        assert!(sm.parts.iter().any(|p| matches!(p, Part::Text { .. })));
    }

    #[test]
    fn test_assemble_with_rules_and_memories() {
        let injectable = InjectableContext {
            soul: "You are helpful.".to_string(),
            rules_and_experiences: vec!["Always read before writing.".to_string()],
            memories: vec!["User prefers Result style.".to_string()],
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable, "Test");
        // soul + (rules+memories merged) + current input
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("helpful"));
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Always read"));
        assert!(messages[1].content.contains("Result style"));
    }

    #[test]
    fn test_structured_to_messages_tool_result() {
        let sm = StructuredMessage {
            id: "msg_tool".to_string(),
            parent_id: None,
            role: MessageRole::Tool,
            parts: vec![Part::ToolResult {
                tool_call_id: "call_1".to_string(),
                content: r#"{"result":"ok"}"#.to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
            compression_marker: false,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_ref().unwrap(), "call_1");
    }

    #[test]
    fn test_empty_injectable_skips_system_messages() {
        let injectable = InjectableContext::default();
        let sm = make_text_msg("msg_1", MessageRole::User, "Hi", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable, "Hello");
        // Only user history + current input, no system messages
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::User);
    }

    #[test]
    fn test_empty_current_input_no_extra_message() {
        let injectable = InjectableContext {
            soul: "You are helpful.".to_string(),
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable, "");
        // Only soul, no current input appended
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_message_to_structured_tool_role_creates_tool_result_part() {
        let msg = Message {
            role: MessageRole::Tool,
            content: r#"{"result":"ok"}"#.to_string(),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            reasoning_content: None,
        };
        let sm = ContextAssembler::message_to_structured(&msg, "ses_1", None, None);
        assert_eq!(sm.role, MessageRole::Tool);
        assert_eq!(sm.parts.len(), 1);
        match &sm.parts[0] {
            Part::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(content, r#"{"result":"ok"}"#);
            }
            _ => panic!("expected ToolResult part"),
        }
    }

    #[test]
    fn test_structured_to_messages_user_with_image_replays_parts() {
        let sm = StructuredMessage {
            id: "msg_img".to_string(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![
                Part::Text {
                    text: "看这张图".to_string(),
                    time: PartTime::default(),
                },
                Part::Image {
                    url: "data:image/png;base64,AAAA".to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
            compression_marker: false,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1, "文本+图片应合并为一条用户消息");
        let msg = &messages[0];
        assert_eq!(msg.content, "看这张图");
        assert_eq!(msg.image_urls(), vec!["data:image/png;base64,AAAA"]);
    }

    #[test]
    fn test_message_to_structured_user_with_images_creates_image_part() {
        let msg =
            Message::user_with_images("看这张图", vec!["data:image/png;base64,AAAA".to_string()]);
        let sm = ContextAssembler::message_to_structured(&msg, "ses_1", None, None);
        let images: Vec<&String> = sm
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Image { url, .. } => Some(url),
                _ => None,
            })
            .collect();
        assert_eq!(images, vec!["data:image/png;base64,AAAA"]);
    }

    #[test]
    fn test_message_to_structured_plain_user_no_image_part() {
        let msg = Message::user("hello");
        let sm = ContextAssembler::message_to_structured(&msg, "ses_1", None, None);
        assert!(
            !sm.parts.iter().any(|p| matches!(p, Part::Image { .. })),
            "纯文本消息不应产生 Image part"
        );
    }
}
