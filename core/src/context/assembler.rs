//! 上下文组装器。
//!
//! 负责将存储层的 StructuredMessage 和 InjectableContext 组装为
//! 传输层的 Vec<Message>，优化 DeepSeek 前缀缓存命中率。

use crate::common::types::InjectableContext;
use crate::common::types::{
    FunctionCall, Message, MessageRole, Part, PartTime, StructuredMessage, TokenUsage,
    ToolCall as CoreToolCall, ToolCallType,
};
use std::borrow::Cow;

/// 上下文组装器——纯函数，无副作用。
pub struct ContextAssembler;

impl ContextAssembler {
    /// 将结构化消息和注入上下文组装为 LLM 传输格式。
    ///
    /// 输出顺序（缓存最优）：
    /// - messages[0]: injectable.soul（system）
    /// - messages[1]: rules + memories（system）
    /// - messages[2..N]: 历史对话（含当前用户输入——调用方已把用户消息
    ///   写入 structured_messages，组装器不再单独接收 current_input）
    pub fn assemble(
        structured_messages: &[StructuredMessage],
        injectable: &InjectableContext,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 注入 soul
        if !injectable.soul.is_empty() {
            messages.push(Message::system(&injectable.soul));
        }

        // 注入项目指令（AGENTS.md）——soul 之后、rules/memories 之前
        if !injectable.project_instructions.is_empty() {
            messages.push(Message::system(&injectable.project_instructions));
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

        // 历史对话（含当前用户输入）：从最后一个压缩点（compression_marker）
        // 开始组装——压缩点之前的原始消息已被摘要替代，不再发给 LLM；
        // 无压缩点时从头组装。压缩点本身（摘要消息）包含在组装范围内。
        let start_idx = structured_messages
            .iter()
            .rposition(|m| m.compression_marker)
            .unwrap_or(0);
        // 工具对连续性规范化（见 `normalize_tool_pairs`）：插队的通知/用户消息
        // 重排到结果之后、悬空调用补合成结果、孤立结果丢弃。上游要求
        // `assistant(tool_calls)` 后必须**紧跟**对应 tool 消息，否则整条会话
        // 此后每次请求都被拒（错误链常驻）。
        for sm in Self::normalize_tool_pairs(&structured_messages[start_idx..]) {
            messages.extend(Self::structured_to_messages(sm.as_ref()));
        }

        messages
    }

    /// 将单个 StructuredMessage 转换为 1~N 条传输层 Message。
    ///
    /// 压缩点（`compression_marker`）以 **user 角色**锚定（DSH 式）：摘要作为
    /// 后续请求中的 user 消息，保证"压缩后的组装视图恒有 ≥1 条 user"——全
    /// system 请求会被云 API 判为无用户输入而返回空输出。旧数据中摘要持久化为
    /// system 角色，此处按 marker 统一归一为 user（新数据落库即 user，转换层
    /// 同时兼容两代数据）。
    pub(crate) fn structured_to_messages(sm: &StructuredMessage) -> Vec<Message> {
        let mut messages = Vec::new();

        // 压缩点归一：旧数据 system → user（见函数文档）。
        let role = if sm.compression_marker {
            MessageRole::User
        } else {
            sm.role
        };

        match role {
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
                            // 无条件保留：天演作为 agent 总是携带 tools 参数，
                            // DeepSeek 思考模型要求携带 tools 的请求必须完整回传
                            // reasoning_content（即使该轮未实际进行工具调用）。
                            reasoning_content = Some(text.clone());
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
                        tool_duration_ms: None,
                        tool_error: None,
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

    /// **工具对连续性规范化**（组装视图，ADR-043）。
    ///
    /// 上游（OpenAI 兼容协议）要求：assistant 消息带 `tool_calls` 时，其**紧随**
    /// 的消息必须是对应的 tool 结果。天演的链上有两种断裂（2026-09-24 全库实测：
    /// 3.7 万条消息中 89 处顺序断裂 + 11 处调用悬空）：
    ///
    /// 1. **插队**：长工具执行期间（实测数十秒）异步通知（system）或用户消息落库，
    ///    插在 `assistant(tool_calls)` 与 tool 结果之间；
    /// 2. **中断残留**：工具被中断（用户停止 / 进程中断）→ 结果永不落库，调用悬空。
    ///
    /// 两者都会让请求被拒（`An assistant message with 'tool_calls' must be followed
    /// by tool messages`），且**错误链常驻**——该会话此后每次请求都失败。
    ///
    /// 规范化只作用于**请求组装视图**：库与缓存不动，结果确定（同一链每次得到
    /// 同一视图）→ 前缀缓存不受影响、可反复调用：
    /// - 已存在的 tool 结果**提前**到其调用之后（其余消息相对顺序不变）；
    /// - 悬空调用**丢弃**（该 assistant 消息的其余内容照常保留），并**补一条合成
    ///   结果**（"调用被中断"）——让模型看到已发生的事实，而不是凭空消失的调用；
    /// - 无任何调用认领的**孤立 tool 结果**丢弃（同样会触发上游 400）。
    pub(crate) fn normalize_tool_pairs(
        chain: &[StructuredMessage],
    ) -> Vec<Cow<'_, StructuredMessage>> {
        use std::collections::{HashMap, HashSet};

        // Pass 1：为每个 assistant(tool_calls) 的调用认领链上第一个未被认领的结果位置
        let mut claimed: HashSet<usize> = HashSet::new();
        let mut plan: HashMap<usize, Vec<(String, Option<usize>)>> = HashMap::new();
        for (i, sm) in chain.iter().enumerate() {
            if sm.role != MessageRole::Assistant {
                continue;
            }
            let calls: Vec<String> = sm
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::ToolCall { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            if calls.is_empty() {
                continue;
            }
            let mut entries = Vec::with_capacity(calls.len());
            for call_id in calls {
                let pos = (i + 1..chain.len())
                    .find(|&j| !claimed.contains(&j) && is_tool_result_of(&chain[j], &call_id));
                if let Some(j) = pos {
                    claimed.insert(j);
                }
                entries.push((call_id, pos));
            }
            plan.insert(i, entries);
        }

        // Pass 2：输出（结果提前 → 悬空调用补合成结果 → 孤立结果丢弃）
        let mut out: Vec<Cow<'_, StructuredMessage>> = Vec::with_capacity(chain.len());
        for (i, sm) in chain.iter().enumerate() {
            if claimed.contains(&i) {
                continue; // 已被提前到其调用之后
            }
            match plan.get(&i) {
                Some(entries) => {
                    let dangling: Vec<&str> = entries
                        .iter()
                        .filter(|(_, pos)| pos.is_none())
                        .map(|(id, _)| id.as_str())
                        .collect();
                    if dangling.is_empty() {
                        out.push(Cow::Borrowed(sm));
                    } else {
                        out.push(Cow::Owned(without_tool_calls(sm, &dangling)));
                    }
                    for (_, pos) in entries.iter() {
                        if let Some(j) = pos {
                            out.push(Cow::Borrowed(&chain[*j]));
                        }
                    }
                    for call_id in dangling {
                        out.push(Cow::Owned(synthetic_interrupted_result(sm, call_id)));
                    }
                }
                None => {
                    // 孤立工具结果（无任何调用认领）→ 丢弃；其余消息原样保留
                    if !has_tool_result(sm) {
                        out.push(Cow::Borrowed(sm));
                    }
                }
            }
        }
        out
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

/// 该消息是否承载指定调用的工具结果。
fn is_tool_result_of(sm: &StructuredMessage, call_id: &str) -> bool {
    sm.parts
        .iter()
        .any(|p| matches!(p, Part::ToolResult { tool_call_id, .. } if tool_call_id == call_id))
}

/// 该消息是否承载工具结果（不判 role——容忍旧数据的 role 漂移）。
fn has_tool_result(sm: &StructuredMessage) -> bool {
    sm.parts
        .iter()
        .any(|p| matches!(p, Part::ToolResult { .. }))
}

/// 复制一条消息并去掉指定 `tool_calls`（其余 parts 顺序不变）。
fn without_tool_calls(sm: &StructuredMessage, drop_ids: &[&str]) -> StructuredMessage {
    let mut out = sm.clone();
    out.parts.retain(|p| match p {
        Part::ToolCall { id, .. } => !drop_ids.contains(&id.as_str()),
        _ => true,
    });
    out
}

/// 合成「调用被中断」的工具结果（**仅请求视图**，不落库）。
///
/// 让模型看到「该调用发生过、被中断无结果」这一事实——比静默丢弃调用更接近真实
/// 语义（也避免模型凭空重试同一动作）。
fn synthetic_interrupted_result(sm: &StructuredMessage, call_id: &str) -> StructuredMessage {
    let mut out = StructuredMessage::system(sm.session_id.clone(), "");
    out.id = format!("synth-{call_id}");
    out.role = MessageRole::Tool;
    out.parts = vec![Part::ToolResult {
        tool_call_id: call_id.to_string(),
        content: "[结果] 工具调用被中断，未产生结果".to_string(),
        error: Some("调用被中断".to_string()),
        time: PartTime::default(),
    }];
    out
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
    fn test_assemble_soul_and_user_message() {
        let injectable = InjectableContext {
            soul: "You are a helpful assistant.".to_string(),
            ..Default::default()
        };
        let sm = make_text_msg("msg_1", MessageRole::User, "Hello", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable);
        assert_eq!(messages.len(), 2); // soul + user message
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "You are a helpful assistant.");
        assert_eq!(messages[1].role, MessageRole::User);
        assert_eq!(messages[1].content, "Hello");
    }

    #[test]
    fn test_assemble_with_history() {
        let injectable = InjectableContext::default();
        let sm1 = make_text_msg("msg_1", MessageRole::User, "Hi", "ses_1");
        let sm2 = make_text_msg("msg_2", MessageRole::User, "Hello again", "ses_1");
        let messages = ContextAssembler::assemble(&[sm1, sm2], &injectable);
        // 2 user messages in history (no injectable, so no system messages)
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].content, "Hi");
        assert_eq!(messages[1].content, "Hello again");
    }

    #[test]
    fn test_assemble_starts_from_last_compression_marker() {
        // 存储层返回完整链（含压缩点前历史）；组装层从最后一个压缩点开始：
        // 压缩点之前的原始消息不再发给 LLM，压缩点（摘要）本身包含在组装内。
        let injectable = InjectableContext::default();
        let pre1 = make_text_msg("pre_1", MessageRole::User, "早期消息 1", "ses_1");
        let pre2 = make_text_msg("pre_2", MessageRole::Assistant, "早期回复", "ses_1");
        let mut marker =
            make_text_msg("cmp_1", MessageRole::System, "[对话摘要] 早期摘要", "ses_1");
        marker.compression_marker = true;
        let post1 = make_text_msg("post_1", MessageRole::User, "近期消息", "ses_1");
        let post2 = make_text_msg("post_2", MessageRole::Assistant, "近期回复", "ses_1");

        let messages = ContextAssembler::assemble(&[pre1, pre2, marker, post1, post2], &injectable);
        // 组装从压缩点开始：摘要 + 近期消息（早期消息不发给 LLM）
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "[对话摘要] 早期摘要");
        // 压缩点 user 锚定（DSH 式）：旧数据（system 角色）经转换层归一为 user
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].content, "近期消息");
        assert_eq!(messages[2].content, "近期回复");
    }

    #[test]
    fn test_compression_marker_role_normalized_to_user() {
        // 两代数据（旧 system / 新 user）转换后一律为 user 锚定；
        // 非压缩点的普通 system 消息不受影响。
        let mut old_marker =
            make_text_msg("cmp_old", MessageRole::System, "[对话摘要] 旧", "ses_1");
        old_marker.compression_marker = true;
        let mut new_marker = make_text_msg("cmp_new", MessageRole::User, "[对话摘要] 新", "ses_1");
        new_marker.compression_marker = true;
        let notify = make_text_msg("sys_1", MessageRole::System, "通知", "ses_1");

        assert_eq!(
            ContextAssembler::structured_to_messages(&old_marker)[0].role,
            MessageRole::User,
            "旧数据（system 角色摘要）应归一为 user"
        );
        assert_eq!(
            ContextAssembler::structured_to_messages(&new_marker)[0].role,
            MessageRole::User,
            "新数据（user 角色摘要）保持 user"
        );
        assert_eq!(
            ContextAssembler::structured_to_messages(&notify)[0].role,
            MessageRole::System,
            "普通 system 消息不受归一影响"
        );
    }

    #[test]
    fn test_assemble_without_marker_includes_all() {
        // 无压缩点时从头组装（完整链全部发给 LLM）
        let injectable = InjectableContext::default();
        let sm1 = make_text_msg("msg_1", MessageRole::User, "Hi", "ses_1");
        let sm2 = make_text_msg("msg_2", MessageRole::Assistant, "Hello", "ses_1");
        let messages = ContextAssembler::assemble(&[sm1, sm2], &injectable);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Hi");
        assert_eq!(messages[1].content, "Hello");
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
    fn test_structured_to_messages_reasoning_without_tool_call_is_kept() {
        // 天演作为 agent 总是携带 tools 参数，DeepSeek 思考模型要求携带 tools
        // 的请求必须完整回传 reasoning_content（即使该轮未实际进行工具调用）——
        // 无 tool_call 的轮次也必须保留思考内容。
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
        assert_eq!(msg.reasoning_content.as_deref(), Some("The answer is 42"));
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
            tool_duration_ms: None,
            tool_error: None,
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
        let sm = make_text_msg("msg_1", MessageRole::User, "Test", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable);
        // soul + (rules+memories merged) + user message
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("helpful"));
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Always read"));
        assert!(messages[1].content.contains("Result style"));
        assert_eq!(messages[2].role, MessageRole::User);
        assert_eq!(messages[2].content, "Test");
    }

    #[test]
    fn test_assemble_injects_project_instructions_after_soul() {
        let injectable = InjectableContext {
            soul: "You are helpful.".to_string(),
            project_instructions: "项目规范：禁止使用 unsafe。".to_string(),
            rules_and_experiences: vec!["Always read before writing.".to_string()],
            ..Default::default()
        };
        let sm = make_text_msg("msg_1", MessageRole::User, "Test", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable);
        // soul + project_instructions + (rules merged) + user message
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].content, "You are helpful.");
        assert_eq!(messages[1].content, "项目规范：禁止使用 unsafe。");
        assert!(messages[2].content.contains("Always read"));
        assert_eq!(messages[3].role, MessageRole::User);
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
                error: None,
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
        let sm1 = make_text_msg("msg_1", MessageRole::User, "Hi", "ses_1");
        let sm2 = make_text_msg("msg_2", MessageRole::User, "Hello", "ses_1");
        let messages = ContextAssembler::assemble(&[sm1, sm2], &injectable);
        // Only user messages, no system messages
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::User);
    }

    #[test]
    fn test_assemble_soul_only() {
        let injectable = InjectableContext {
            soul: "You are helpful.".to_string(),
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable);
        // Only soul, no messages to append
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
            tool_duration_ms: None,
            tool_error: None,
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

    // ── ADR-043：工具对连续性规范化（组装视图）───────────────────────

    /// 带工具调用的 assistant 消息。
    fn assistant_with_call(sid: &str, call_id: &str, text: &str) -> StructuredMessage {
        let mut sm = StructuredMessage::assistant(sid, text);
        sm.parts.push(Part::ToolCall {
            id: call_id.to_string(),
            name: "execute_command".to_string(),
            arguments: "{}".to_string(),
            time: PartTime::default(),
        });
        sm
    }

    /// role = Tool 的工具结果消息。
    fn tool_result(sid: &str, call_id: &str, text: &str) -> StructuredMessage {
        let mut sm = StructuredMessage::user(sid, "");
        sm.role = MessageRole::Tool;
        sm.parts = vec![Part::ToolResult {
            tool_call_id: call_id.to_string(),
            content: text.to_string(),
            error: None,
            time: PartTime::default(),
        }];
        sm
    }

    fn normalize(chain: &[StructuredMessage]) -> Vec<StructuredMessage> {
        ContextAssembler::normalize_tool_pairs(chain)
            .into_iter()
            .map(|sm| sm.into_owned())
            .collect()
    }

    /// 规范化后的**传输层**序列必须满足：assistant(tool_calls) 后紧邻 Tool 消息。
    fn assert_pairs_wellformed(chain: &[StructuredMessage]) {
        let msgs: Vec<Message> = ContextAssembler::normalize_tool_pairs(chain)
            .iter()
            .flat_map(|sm| ContextAssembler::structured_to_messages(sm.as_ref()))
            .collect();
        for (i, m) in msgs.iter().enumerate() {
            if m.role == MessageRole::Assistant
                && m.tool_calls.as_ref().is_some_and(|c| !c.is_empty())
            {
                let next = msgs.get(i + 1).expect("tool_calls 之后必须有消息");
                assert_eq!(
                    next.role,
                    MessageRole::Tool,
                    "assistant(tool_calls) 后必须紧跟 tool 消息（上游硬要求）：{msgs:?}"
                );
            }
        }
    }

    /// 形态 A：通知（system）插在调用与结果之间 → 结果提前，通知后移。
    #[test]
    fn test_normalize_moves_inserted_notice_after_result() {
        let chain = vec![
            assistant_with_call("s1", "c1", "跑命令"),
            StructuredMessage::system("s1", "[后台服务就绪] node ..."),
            tool_result("s1", "c1", "exit_code=0"),
        ];
        assert_pairs_wellformed(&chain);
        let out = normalize(&chain);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, MessageRole::Assistant);
        assert_eq!(out[1].role, MessageRole::Tool, "结果提前到调用之后");
        assert_eq!(out[2].role, MessageRole::System, "插队的通知后移到结果之后");
        assert!(
            is_tool_result_of(&out[1], "c1"),
            "提前的必须是同一次调用的结果"
        );
    }

    /// 形态 B：中断残留（调用悬空）→ 丢弃调用 + 补合成结果（模型可见事实）。
    #[test]
    fn test_normalize_synthesizes_result_for_dangling_call() {
        let chain = vec![
            assistant_with_call("s1", "c1", "跑命令"),
            StructuredMessage::user("s1", "怎么中断了？继续"),
        ];
        assert_pairs_wellformed(&chain);
        let out = normalize(&chain);
        assert_eq!(out.len(), 3, "补一条合成结果");
        assert_eq!(out[0].role, MessageRole::Assistant);
        assert!(
            !out[0]
                .parts
                .iter()
                .any(|p| matches!(p, Part::ToolCall { .. })),
            "悬空调用应从 assistant 上移除"
        );
        assert!(out[0]
            .parts
            .iter()
            .any(|p| matches!(p, Part::Text { text, .. } if text.contains("跑命令"))));
        assert_eq!(out[1].role, MessageRole::Tool);
        assert!(is_tool_result_of(&out[1], "c1"));
        assert_eq!(out[2].role, MessageRole::User, "用户消息顺序不变");
    }

    /// 孤立工具结果（无任何调用认领）→ 丢弃（否则同样触发上游 400）。
    #[test]
    fn test_normalize_drops_orphan_tool_result() {
        let chain = vec![
            tool_result("s1", "c1", "孤儿结果"),
            StructuredMessage::user("s1", "继续"),
        ];
        assert_pairs_wellformed(&chain);
        let out = normalize(&chain);
        assert_eq!(out.len(), 1, "孤立结果被丢弃");
        assert_eq!(out[0].role, MessageRole::User);
    }

    /// 一次调用多个工具、部分有结果 → 有结果的保留并提前，悬空的补合成结果。
    #[test]
    fn test_normalize_partial_calls() {
        let mut assistant = assistant_with_call("s1", "c1", "并行两个");
        assistant.parts.push(Part::ToolCall {
            id: "c2".to_string(),
            name: "read_file".to_string(),
            arguments: "{}".to_string(),
            time: PartTime::default(),
        });
        let chain = vec![
            assistant,
            tool_result("s1", "c1", "第一个结果"),
            StructuredMessage::user("s1", "接着来"),
        ];
        assert_pairs_wellformed(&chain);
        let out = normalize(&chain);
        assert_eq!(out.len(), 4, "assistant + 已有结果 + 合成结果 + 用户消息");
        let calls = match &out[0]
            .parts
            .iter()
            .find(|p| matches!(p, Part::ToolCall { .. }))
        {
            Some(Part::ToolCall { id, .. }) => id.clone(),
            _ => panic!("应保留有结果的调用"),
        };
        assert_eq!(calls, "c1", "只保留有结果的调用");
        assert!(is_tool_result_of(&out[1], "c1"));
        assert!(is_tool_result_of(&out[2], "c2"), "悬空调用补合成结果");
        assert_eq!(out[3].role, MessageRole::User);
    }

    /// 规范链不受影响（幂等：对规范化结果再跑一次结果相同）。
    #[test]
    fn test_normalize_keeps_wellformed_chain_and_is_idempotent() {
        let chain = vec![
            StructuredMessage::user("s1", "问题"),
            assistant_with_call("s1", "c1", "跑命令"),
            tool_result("s1", "c1", "ok"),
            StructuredMessage::system("s1", "通知"),
        ];
        let out = normalize(&chain);
        assert_eq!(out.len(), chain.len(), "规范链不被增删");
        for (a, b) in out.iter().zip(chain.iter()) {
            assert_eq!(a.role, b.role);
            assert_eq!(a.id, b.id, "原链消息顺序不变");
        }
        let again = normalize(&out);
        assert_eq!(again.len(), out.len());
        for (a, b) in again.iter().zip(out.iter()) {
            assert_eq!(a.id, b.id, "二次规范化结果一致（幂等）");
        }
    }

    /// 端到端：`assemble` 产出的序列在任意断裂链下都满足工具对连续性。
    #[test]
    fn test_assemble_never_breaks_tool_pairs() {
        let chain = vec![
            StructuredMessage::user("s1", "u0"),
            assistant_with_call("s1", "c1", "a1"),
            StructuredMessage::system("s1", "[后台服务就绪] 插队通知"),
            tool_result("s1", "c1", "r1"),
            assistant_with_call("s1", "c2", "a2"),
            StructuredMessage::user("s1", "u3（中断后新消息）"),
        ];
        let msgs = ContextAssembler::assemble(&chain, &InjectableContext::default());
        for (i, m) in msgs.iter().enumerate() {
            if m.role == MessageRole::Assistant
                && m.tool_calls.as_ref().is_some_and(|c| !c.is_empty())
            {
                assert_eq!(
                    msgs[i + 1].role,
                    MessageRole::Tool,
                    "assemble 输出必须满足上游工具对要求：{msgs:?}"
                );
            }
        }
    }
}
