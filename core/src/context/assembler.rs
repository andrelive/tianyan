//! 上下文组装器。
//!
//! 负责将存储层的 StructuredMessage 和 InjectableContext 组装为
//! 传输层的 Vec<Message>，优化 DeepSeek 前缀缓存命中率。

use crate::common::types::InjectableContext;
use crate::common::types::{
    FunctionCall, Message, MessageRole, Part, PartTime, StructuredMessage, TokenUsage,
    ToolCall as CoreToolCall, ToolCallType,
};
use crate::context::SessionImages;
use std::borrow::Cow;

/// 上下文组装器——纯函数，无副作用。
pub struct ContextAssembler;

impl ContextAssembler {
    /// 将结构化消息和注入上下文组装为 LLM 传输格式。
    ///
    /// **无图版本**：tool 结果里的图片引用不做加载（见 [`Self::assemble_with`]）；
    /// 不知道 `data_dir` 的调用点（core 内单测等）用它，行为与引入图片能力前一致。
    ///
    /// 输出顺序（缓存最优）：
    /// 1. injectable.soul（system）
    /// 2. 当前环境——工作目录 + 相对路径口径（system；工作目录未绑定/为空时整段缺席）
    /// 3. injectable.project_instructions（AGENTS.md，system）
    /// 4. rules + memories（合并为一条 system）
    /// 5. 历史对话（含当前用户输入——调用方已把用户消息写入 structured_messages，
    ///    组装器不再单独接收 current_input）
    ///
    /// 2 必须排在 3 之前：AGENTS.md 里写的是相对路径，环境段先立锚点模型才用得上。
    /// 各段缺席时后续段整体前移（每段各自判空，不占位）。
    pub fn assemble(
        structured_messages: &[StructuredMessage],
        injectable: &InjectableContext,
    ) -> Vec<Message> {
        Self::assemble_with(structured_messages, injectable, None)
    }

    /// 将结构化消息和注入上下文组装为 LLM 传输格式（可携带 tool 结果图片）。
    ///
    /// 与 [`Self::assemble`] 的唯一差别：`images` 非 `None` 时，Tool 分支会按
    /// [`Part::ToolResult`] 原文里的引用（`session_images/<name>`）加载图片字节，
    /// 把该 tool 结果组装成携带图片的同一条消息（ADR-050：tool 结果直通图片）。
    ///
    /// `images` 为 `None` 或引用解析不出图片时，行为与 [`Self::assemble`] 逐字节一致
    /// （纯文本 tool 结果零变化）。引用解析是 best effort：副本缺失 / 超限 /
    /// magic 不符 → 该结果保持纯文本，不报错。
    pub fn assemble_with(
        structured_messages: &[StructuredMessage],
        injectable: &InjectableContext,
        images: Option<&SessionImages>,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 注入 soul
        if !injectable.soul.is_empty() {
            messages.push(Message::system(&injectable.soul));
        }

        // 注入当前环境（工作目录 + 相对路径口径 [+ 异常警示]）——soul 之后、
        // AGENTS.md 之前：AGENTS.md 正文里的相对路径（如
        // `docs/architecture/decisions/`）需要先立基准，模型才知道该从哪儿起算。
        //
        // 两类内容任一存在就注入本段：
        // - 工作目录本身（未绑定 / 空串时不给，避免渲染出空 code span）；
        // - 异常警示（绑定目录失效 / 无可用目录）——**这段即便没有可用目录也必须
        //   出现**：它正是「模型别瞎试、去提醒用户绑定工作区」的唯一信息源。
        let working_directory = injectable
            .working_directory
            .as_deref()
            .filter(|d| !d.is_empty());
        let warning = injectable
            .working_directory_warning
            .as_deref()
            .filter(|w| !w.is_empty());
        if working_directory.is_some() || warning.is_some() {
            messages.push(Message::system(render_environment(
                working_directory,
                warning,
            )));
        }

        // 注入项目指令（AGENTS.md）——环境段之后、rules/memories 之前
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
            messages.extend(Self::structured_to_messages(sm.as_ref(), images));
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
    ///
    /// `images` 非 `None` 时，Tool 结果会按原文里的图片引用加载字节并组装成
    /// 携带图片的同一条消息（ADR-050）；解析不出图片时保持纯文本（best effort）。
    pub(crate) fn structured_to_messages(
        sm: &StructuredMessage,
        images: Option<&SessionImages>,
    ) -> Vec<Message> {
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
                        let urls = images.map(|i| i.resolve(content)).unwrap_or_default();
                        if urls.is_empty() {
                            messages.push(Message::tool(tool_call_id.clone(), content.clone()));
                        } else {
                            messages.push(Message::tool_with_images(
                                tool_call_id.clone(),
                                content.clone(),
                                urls,
                            ));
                        }
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
    /// - 悬空调用**保留**在 assistant 上（`tool_calls` 原样），并**补一条 id 对齐的
    ///   合成结果**（"调用被中断"）——上游两个方向同时硬约束：**每个调用都要有
    ///   结果**、**每条结果都要能找到调用**；删调用会让补出来的结果变成孤立 tool
    ///   （严格上游报 `Messages with role 'tool' must be a response to a preceding
    ///   message with 'tool_calls'`，见 ADR-043 修订）；
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
                    // 悬空调用**保留**（不删 `tool_calls`）：上游同时要求「每个调用都要
                    // 有结果」与「每条结果都要能找到调用」，删调用会让下面补的合成结果
                    // 变成没有宿主的孤立 tool → 400（ADR-043 修订）。
                    out.push(Cow::Borrowed(sm));
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

/// 渲染「当前环境」系统段：工作目录 + 相对路径解析口径（+ 异常警示）。
///
/// 措辞克制（这段进每次请求）：只讲模型会用到的两件事——基准在哪、
/// 相对路径优于绝对路径的理由。不重复工具描述里已说明的平台信息。
///
/// `warning` 来自 `WorkingDirectory::warning()`（绑定目录失效 / 无可用目录）：
/// 非 `None` 时**前置**在段首，让模型先看到「这不是你该反复试错的事，而该提醒
/// 用户」。它与 `working_directory` 来自**同一个** `WorkingDirectory` 值，
/// 因此「探测 AGENTS.md 的根」与「告诉模型的目录」不会分叉。
///
/// 无可用目录（`working_directory = None` + `warning = Some`）时只渲染警示——
/// 没有可给的路径，就不给「工作目录：\`\`」这种空 code span。
///
/// 纯函数（无 I/O、无状态），内容只依赖入参——工作目录在会话生命周期内不变，
/// 故同一会话每次请求渲染逐字节一致，不引起 prompt 缓存抖动。
fn render_environment(working_directory: Option<&str>, warning: Option<&str>) -> String {
    let mut text = String::from("## 当前环境\n");
    if let Some(warning) = warning {
        text.push_str(&format!("\n{warning}\n"));
    }
    if let Some(working_directory) = working_directory {
        text.push_str(&format!(
            "\n工作目录：`{working_directory}`\n\
             \n\
             所有相对路径（如 `src/main.rs`、`./docs`）都基于此目录解析——`read_file` / `glob` / \
             `grep` / `list_dir` / `execute_command`（未显式指定 cwd 时）同一口径。\
             **结论里优先写相对路径**：绝对路径会随会话永久落库，换机器或搬目录后即失效。\n"
        ));
    }
    text
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
            ContextAssembler::structured_to_messages(&old_marker, None)[0].role,
            MessageRole::User,
            "旧数据（system 角色摘要）应归一为 user"
        );
        assert_eq!(
            ContextAssembler::structured_to_messages(&new_marker, None)[0].role,
            MessageRole::User,
            "新数据（user 角色摘要）保持 user"
        );
        assert_eq!(
            ContextAssembler::structured_to_messages(&notify, None)[0].role,
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
        let messages = ContextAssembler::structured_to_messages(&sm, None);
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
        let messages = ContextAssembler::structured_to_messages(&sm, None);
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

    // ── 当前环境段（工作目录注入）─────────────────────────────────

    /// `render_environment` 的三个信息点必须都在：工作目录路径本身、
    /// 「相对路径基于此目录」的口径、「结论里优先写相对路径」的指引。
    ///
    /// 判别力：少任何一条，模型就回到「不知道基准」或「继续硬编码绝对路径」。
    #[test]
    fn test_render_environment_contains_path_and_relative_guidance() {
        let text = render_environment(Some("F:\\work\\git\\tianyan"), None);

        assert!(
            text.starts_with("## 当前环境"),
            "环境段标题固定（便于模型识别与人工排查）：{text}"
        );
        assert!(
            text.contains("F:\\work\\git\\tianyan"),
            "必须给出工作目录本身：{text}"
        );
        assert!(
            text.contains("相对路径") && text.contains("都基于此目录解析"),
            "必须说明相对路径以该目录为基准：{text}"
        );
        assert!(
            text.contains("优先写相对路径"),
            "必须给出「结论写相对路径」的指引（绝对路径落库后换机即失效）：{text}"
        );
        // 相对路径口径覆盖模型实际会用到的读取/搜索/命令类工具
        for tool in ["read_file", "glob", "grep", "list_dir", "execute_command"] {
            assert!(text.contains(tool), "口径须点名 {tool}：{text}");
        }
        // 克制：平台/shell 细节由工具描述承担，不在这里重复（每轮都在前缀里）
        for noise in ["PowerShell", "pwsh", "bash", "cmd.exe", "Windows", "Linux"] {
            assert!(
                !text.contains(noise),
                "环境段不应重复平台信息（出现 {noise}）：{text}"
            );
        }
        // 正常态（无警示）不得出现 ⚠️：这段每轮都在前缀里，噪声会稀释真警示
        assert!(!text.contains("⚠️"), "正常态不应有警示：{text}");
        // 逐字节确定：同入参同输出（前缀缓存依赖这一点）
        assert_eq!(
            render_environment(Some("F:\\work\\git\\tianyan"), None),
            text,
            "渲染必须是纯函数（前缀稳定性）"
        );
        // 与「引入警示前」逐字节一致：这段进每次请求，任何空白改动都会让存量
        // 会话的 prompt 前缀缓存整体失效（成本是每次请求全量重算）。
        assert_eq!(
            text,
            "## 当前环境\n\n工作目录：`F:\\work\\git\\tianyan`\n\n所有相对路径（如 `src/main.rs`、\
             `./docs`）都基于此目录解析——`read_file` / `glob` / `grep` / `list_dir` / \
             `execute_command`（未显式指定 cwd 时）同一口径。**结论里优先写相对路径**：\
             绝对路径会随会话永久落库，换机器或搬目录后即失效。\n",
            "正常态环境段必须与引入警示前的渲染逐字节一致（存量会话前缀缓存）"
        );
    }

    // ── 工作目录异常警示（L3：环境段如实反映状态）───────────────────

    /// 绑定目录失效 + 已回退到默认：警示在前、两个路径都在（失效的 + 兜底的）。
    ///
    /// 判别力：把警示塞在段尾或只给一个路径时，模型很可能只读到「工作目录：X」
    /// 就继续在 X 里写（X 已被删），或只看到警示却不知道现在该在哪儿干活。
    #[test]
    fn test_render_environment_with_bound_invalid_warning() {
        let warning = "⚠️ 本会话绑定的工作目录 `D:\\gone` 已不存在（可能被删除、移动或未挂载），\
                       已回退到全局默认工作目录。";
        let text = render_environment(Some("F:\\workspace"), Some(warning));

        assert!(text.starts_with("## 当前环境"), "{text}");
        let warn_pos = text.find("⚠️").expect("警示必须在环境段里");
        let wd_pos = text
            .find("工作目录：`F:\\workspace`")
            .expect("仍要给出兜底目录");
        assert!(
            warn_pos < wd_pos,
            "警示必须前置（先看到事实，再看到现在在哪儿）：{text}"
        );
        assert!(text.contains("D:\\gone"), "必须指名失效的绑定目录：{text}");
        assert!(
            text.contains("F:\\workspace"),
            "必须给出实际生效的兜底目录：{text}"
        );
        // 兜底目录仍然带着相对路径口径（模型要在这里继续干活）
        assert!(text.contains("都基于此目录解析"), "{text}");
    }

    /// 无任何可用目录：只渲染警示，不渲染空的工作目录行。
    #[test]
    fn test_render_environment_unavailable_has_no_path_line() {
        let warning = "⚠️ 没有可用工作目录，相对路径操作将失败。请让用户为该会话绑定工作区。";
        let text = render_environment(None, Some(warning));

        assert!(text.starts_with("## 当前环境"), "{text}");
        assert!(text.contains("⚠️"), "{text}");
        assert!(text.contains("请让用户为该会话绑定工作区"), "{text}");
        assert!(
            !text.contains("工作目录：`"),
            "无可用目录时不得渲染空的工作目录行（空 code span 只会误导）：{text}"
        );
        assert!(
            !text.contains("都基于此目录解析"),
            "没有基准就别说「基于此目录」：{text}"
        );
    }

    /// **核心契约**：注入顺序 = soul → 当前环境 → AGENTS.md → rules+memories。
    ///
    /// 环境段必须排在 AGENTS.md 之前：AGENTS.md 里的相对路径（如
    /// `docs/architecture/decisions/`）要先有基准才可解析。
    #[test]
    fn test_assemble_order_soul_environment_agents_then_rules() {
        let workdir = "F:\\work\\git\\tianyan";
        let injectable = InjectableContext {
            soul: "SOUL-人格".to_string(),
            working_directory: Some(workdir.to_string()),
            project_instructions: "AGENTS-MD-项目规范".to_string(),
            rules_and_experiences: vec!["RULE-先读后写".to_string()],
            memories: vec!["MEMORY-用户偏好 Rust".to_string()],
            ..Default::default()
        };
        let sm = make_text_msg("msg_1", MessageRole::User, "用户输入", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable);

        assert_eq!(
            messages.len(),
            5,
            "soul + 环境 + AGENTS.md + rules/memories + 用户消息：{messages:?}"
        );
        assert_eq!(messages[0].content, "SOUL-人格");
        assert!(
            messages[1].content.starts_with("## 当前环境") && messages[1].content.contains(workdir),
            "第 2 段必须是当前环境（soul 之后、AGENTS.md 之前）：{}",
            messages[1].content
        );
        assert_eq!(
            messages[2].content, "AGENTS-MD-项目规范",
            "AGENTS.md 紧跟环境段"
        );
        assert!(
            messages[3].content.contains("RULE-先读后写")
                && messages[3].content.contains("MEMORY-用户偏好 Rust"),
            "rules+memories 合并为最后一条 system 段：{}",
            messages[3].content
        );
        assert_eq!(messages[4].role, MessageRole::User);
        assert_eq!(messages[4].content, "用户输入");
        for (i, m) in messages[..4].iter().enumerate() {
            assert_eq!(m.role, MessageRole::System, "第 {i} 段应为 system");
        }
    }

    /// 未绑定工作目录（`None`）→ 不出现环境段，其余段顺序不变。
    ///
    /// 环境段缺席是「整段不注入」而不是「留一条空 system」——不占 token，
    /// 也不改变后面各段的相对次序。
    #[test]
    fn test_assemble_without_working_directory_skips_environment() {
        let injectable = InjectableContext {
            soul: "SOUL-人格".to_string(),
            working_directory: None,
            project_instructions: "AGENTS-MD-项目规范".to_string(),
            rules_and_experiences: vec!["RULE-先读后写".to_string()],
            memories: vec!["MEMORY-用户偏好 Rust".to_string()],
            ..Default::default()
        };
        let sm = make_text_msg("msg_1", MessageRole::User, "用户输入", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable);

        assert_eq!(
            messages.len(),
            4,
            "soul + AGENTS.md + rules/memories + 用户消息（无环境段）：{messages:?}"
        );
        assert!(
            !messages.iter().any(|m| m.content.contains("## 当前环境")),
            "未绑定工作目录时不得注入环境段"
        );
        assert_eq!(messages[0].content, "SOUL-人格");
        assert_eq!(messages[1].content, "AGENTS-MD-项目规范");
        assert!(messages[2].content.contains("RULE-先读后写"));
        assert_eq!(messages[3].role, MessageRole::User);
    }

    /// 空串工作目录（历史数据 / 绑定异常）同样不注入——渲染出来会是个空 code span，
    /// 不如不给；行为与 `None` 一致。
    #[test]
    fn test_assemble_empty_working_directory_skips_environment() {
        let injectable = InjectableContext {
            soul: "SOUL-人格".to_string(),
            working_directory: Some(String::new()),
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable);
        assert_eq!(messages.len(), 1, "只有 soul：{messages:?}");
        assert!(!messages[0].content.contains("## 当前环境"));
    }

    /// 无可用工作目录但有警示 → 环境段**仍要注入**（且只有警示）。
    ///
    /// 判别力：注入条件若只看 `working_directory`，警示永远发不出去——模型在
    /// 「没有基准」的会话里既不知道基准，也不知道该提醒用户，只能瞎试。
    #[test]
    fn test_assemble_injects_warning_without_working_directory() {
        let warning = "⚠️ 没有可用工作目录，相对路径操作将失败。请让用户为该会话绑定工作区。";
        let injectable = InjectableContext {
            soul: "SOUL-人格".to_string(),
            working_directory: None,
            working_directory_warning: Some(warning.to_string()),
            project_instructions: "AGENTS-MD-项目规范".to_string(),
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable);

        assert_eq!(
            messages.len(),
            3,
            "soul + 环境(仅警示) + AGENTS.md：{messages:?}"
        );
        assert!(
            messages[1].content.starts_with("## 当前环境") && messages[1].content.contains("⚠️"),
            "第 2 段必须是环境警示：{}",
            messages[1].content
        );
        assert_eq!(messages[2].content, "AGENTS-MD-项目规范");
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
        let messages = ContextAssembler::structured_to_messages(&sm, None);
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
        let messages = ContextAssembler::structured_to_messages(&sm, None);
        assert_eq!(messages.len(), 1, "文本+图片应合并为一条用户消息");
        let msg = &messages[0];
        assert_eq!(msg.content, "看这张图");
        assert_eq!(msg.image_urls(), vec!["data:image/png;base64,AAAA"]);
    }

    // ── ADR-050：tool 结果携带图片（组装期按引用加载）──────────────────

    /// 造一条 tool 结果消息（原文为 `view_image` 的返回 JSON）。
    fn tool_result_msg(sid: &str, call_id: &str, content: &str) -> StructuredMessage {
        let mut sm = StructuredMessage::user(sid, "");
        sm.role = MessageRole::Tool;
        sm.parts = vec![Part::ToolResult {
            tool_call_id: call_id.to_string(),
            content: content.to_string(),
            error: None,
            time: PartTime::default(),
        }];
        sm
    }

    /// 组装期把 tool 结果里的引用加载为 data URL，生成**同一条**带图 tool 消息
    /// （不是追加的合成消息），content 保留原文。
    #[test]
    fn test_structured_to_messages_tool_result_with_images() {
        let dir = tempfile::tempdir().unwrap();
        let images_dir = dir.path().join(crate::executor::image::SESSION_IMAGES_DIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        std::fs::write(images_dir.join("x.png"), b"\x89PNG\r\n\x1A\nrest").unwrap();
        let images = SessionImages::new(&images_dir);

        let content = r#"{"image":"session_images/x.png","sha256":"abc","mime":"image/png"}"#;
        let sm = tool_result_msg("ses_1", "call_1", content);
        let messages = ContextAssembler::structured_to_messages(&sm, Some(&images));
        assert_eq!(messages.len(), 1, "图片与 tool 原文是同一条消息");
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(msg.content, content, "content 保留 tool 结果原文");
        let urls = msg.image_urls();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].starts_with("data:image/png;base64,"));
    }

    /// 引用存在但副本缺失 → 降级为纯文本 tool 消息（`content_parts` 为 None，best effort）。
    #[test]
    fn test_structured_to_messages_tool_result_missing_copy_is_text() {
        let dir = tempfile::tempdir().unwrap();
        let images_dir = dir.path().join(crate::executor::image::SESSION_IMAGES_DIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        let images = SessionImages::new(&images_dir);

        let content = r#"{"image":"session_images/missing.png"}"#;
        let sm = tool_result_msg("ses_1", "call_1", content);
        let messages = ContextAssembler::structured_to_messages(&sm, Some(&images));
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].content_parts.is_none(),
            "副本缺失应降级为纯文本工具结果"
        );
        assert_eq!(messages[0].content, content);
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

    /// 规范化后的**传输层**序列必须同时满足上游两个方向的硬约束：
    /// 1. 前向：`assistant(tool_calls)` 后紧邻 Tool 消息；
    /// 2. 反向：每条 Tool 的 `tool_call_id` 必须属于**紧邻其前**那条 assistant 的
    ///    `tool_calls`（否则报 `Messages with role 'tool' must be a response to a
    ///    preceding message with 'tool_calls'`）。
    ///
    /// 反向断言是 ADR-043 修订补的判别力：此前只查前向，于是「删掉悬空调用、
    /// 补一条无人认领的合成结果」被测试当成期望固化下来，严格上游一路 400。
    fn assert_messages_wellformed(msgs: &[Message]) {
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
        for (i, m) in msgs.iter().enumerate() {
            if m.role != MessageRole::Tool {
                continue;
            }
            let call_id = m.tool_call_id.as_deref().unwrap_or_default();
            // 同一次调用的多个结果连续排列，向前跳过同组 tool 消息找发起方
            let owner = msgs[..i]
                .iter()
                .rev()
                .find(|prev| prev.role != MessageRole::Tool)
                .and_then(|prev| prev.tool_calls.as_ref())
                .is_some_and(|calls| calls.iter().any(|c| c.id == call_id));
            assert!(
                owner,
                "tool 消息的 tool_call_id 必须对应前置 assistant 的调用（上游硬要求）：{msgs:?}"
            );
        }
    }

    fn assert_pairs_wellformed(chain: &[StructuredMessage]) {
        let msgs: Vec<Message> = ContextAssembler::normalize_tool_pairs(chain)
            .iter()
            .flat_map(|sm| ContextAssembler::structured_to_messages(sm.as_ref(), None))
            .collect();
        assert_messages_wellformed(&msgs);
    }

    /// 判别力自检：断言必须能抓住「孤立 tool」——否则测试会把错误形态当成期望。
    #[test]
    #[should_panic(expected = "tool 消息的 tool_call_id 必须对应前置 assistant 的调用")]
    fn test_wellformed_assertion_rejects_orphan_tool() {
        // 模拟旧行为：悬空调用被删（assistant 上已无 tool_calls），只剩合成结果
        let assistant = Message::assistant("");
        let orphan = Message::tool("c1", "[结果] 工具调用被中断，未产生结果");
        assert_messages_wellformed(&[assistant, orphan]);
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
            out[0]
                .parts
                .iter()
                .any(|p| matches!(p, Part::ToolCall { id, .. } if id == "c1")),
            "悬空调用必须保留（删掉会让合成结果变成没有宿主的孤立 tool）"
        );
        assert!(out[0]
            .parts
            .iter()
            .any(|p| matches!(p, Part::Text { text, .. } if text.contains("跑命令"))));
        assert_eq!(out[1].role, MessageRole::Tool);
        assert!(is_tool_result_of(&out[1], "c1"));
        assert_eq!(out[2].role, MessageRole::User, "用户消息顺序不变");
    }

    /// **回归（ADR-043 修订）**：悬空调用必须与合成结果成对保留——删调用会让合成
    /// 结果变成孤立 tool，严格上游（opencode→DeepSeek 实测）报
    /// `Messages with role 'tool' must be a response to a preceding message with
    /// 'tool_calls'`，且该会话此后每次请求都 400。
    #[test]
    fn test_normalize_dangling_call_stays_paired_with_synthetic_result() {
        let chain = vec![
            StructuredMessage::user("s1", "u0"),
            assistant_with_call("s1", "c1", "跑命令"),
            StructuredMessage::user("s1", "中断了，继续"),
        ];
        let out = normalize(&chain);
        let assistant = out
            .iter()
            .find(|sm| sm.role == MessageRole::Assistant)
            .expect("assistant 应保留");
        let calls: Vec<&str> = assistant
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, vec!["c1"], "悬空调用不得从 assistant 上删除");

        let msgs: Vec<Message> = out
            .iter()
            .flat_map(|sm| ContextAssembler::structured_to_messages(sm, None))
            .collect();
        assert_messages_wellformed(&msgs);
        assert!(
            msgs.iter()
                .any(|m| m.role == MessageRole::Tool && m.tool_call_id.as_deref() == Some("c1")),
            "合成结果必须带原调用的 tool_call_id"
        );
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
        let calls: Vec<String> = out[0]
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec!["c1", "c2"],
            "悬空调用同样保留（只补结果、不删调用）"
        );
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
        assert_messages_wellformed(&msgs);
    }
}
