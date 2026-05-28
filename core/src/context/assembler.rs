//! 上下文组装器。
//!
//! 负责将存储层的 StructuredMessage 和 InjectableContext 组装为
//! 传输层的 Vec<Message>，优化 DeepSeek 前缀缓存命中率。

use crate::agent::session_state::InjectableContext;
use crate::common::types::{
    DetailedTokenUsage, FunctionCall, Message, MessageRole, MessageTime, Part, PartTime,
    StructuredMessage, ToolCall as CoreToolCall, ToolCallType,
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
                for part in &sm.parts {
                    if let Part::Text { text, .. } = part {
                        messages.push(Message::user(text.clone()));
                    }
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
                        Part::ToolCall { id, name, arguments, .. } => {
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
                    }
                }

                if !content.is_empty() || !tool_calls.is_empty() {
                    let msg = Message {
                        role: MessageRole::Assistant,
                        content,
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
                    if let Part::ToolResult { tool_call_id, content, .. } = part {
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
    pub fn message_to_structured(
        msg: &Message,
        session_id: &str,
        parent_id: Option<&str>,
    ) -> StructuredMessage {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let default_time = PartTime::default();
        let id = format!("msg_{}", now_ms);
        let role = msg.role;

        let mut parts = Vec::new();

        match role {
            MessageRole::Tool => {
                parts.push(Part::ToolResult {
                    tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
                    content: msg.content.clone(),
                    time: default_time,
                });
            }
            _ => {
                if let Some(ref reasoning) = msg.reasoning_content {
                    if !reasoning.is_empty() {
                        parts.push(Part::Reasoning {
                            text: reasoning.clone(),
                            time: default_time.clone(),
                        });
                    }
                }

                if !msg.content.is_empty() {
                    parts.push(Part::Text {
                        text: msg.content.clone(),
                        time: default_time.clone(),
                    });
                }

                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        parts.push(Part::ToolCall {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                            time: default_time.clone(),
                        });
                    }
                }
            }
        }

        StructuredMessage {
            id,
            parent_id: parent_id.map(|s| s.to_string()),
            role,
            parts,
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now_ms,
                completed: now_ms,
            },
            session_id: session_id.to_string(),
            finish: None,
        }
    }
}
