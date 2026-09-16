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
    /// 消息在会话链上的**位置**（ADR-035 §4：分段加载与事件流按 seq 对齐）。
    ///
    /// 语义是位置而非稳定 ID——`rewrite`（删除/回退）后重排。前端「按 seq 落位」
    /// 只适用于**追加**；`rewrite` 由 `MessagesRewritten` 事件整体替换窗口。
    /// 旧调用点（无链上位置上下文）为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    /// 消息角色
    pub role: MessageRole,
    /// 前端生成的用户消息临时 id（ADR-031 乐观渲染定位键）——**仅请求方向**
    /// 携带；落库后经 UserMessageId 确认事件回显真实 id，此后弃用（不持久化）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    /// 纯文本正文——**仅请求方向**（ChatRequest.message：用户输入）承载；
    /// 响应方向（历史/快照/广播/边界事件）为 None 序列化跳过——正文在
    /// segments 的 Text 段（单一事实源，渲染/复制/判断前端一律从 segments 取）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 思考过程文本（模型 reasoning；正文在 segments，前端分开渲染）。
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
    /// 压缩摘要标记（system 角色；压缩点消息）。前端据此识别摘要消息：
    /// 圆环占用对其特殊处理（压缩请求真实 input 是压缩前上下文，不能直接
    /// 作"压缩后占用"），会话消耗统计据此区分摘要消耗入账。
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub compression_marker: bool,
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
    /// - Part::Reasoning → Thinking 段（仅 assistant 产生）
    /// - Part::Text → Text 段（所有角色：用户/助手/系统通知的正文都进时间线，
    ///   前端渲染统一走 SegmentBlocks——用户消息不再依赖"无 segments 兜底"）
    /// - Part::ToolCall → Tool 段（结果挂到 tool_calls 卡片，不产生段）
    /// - Part::ToolResult / Part::Image → 不产生段（结果合并/图片独立渲染）
    pub(crate) fn segments_from_parts(
        _role: &MessageRole,
        parts: &[tianyan::common::types::Part],
    ) -> Option<Vec<MessageSegment>> {
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
    /// 提取核心消息 parts 的展示字段（Text→正文、Reasoning→思考、Image→图片）。
    ///
    /// 流式边界事件（from_structured_light）与历史加载（sessions 服务）共用
    /// 同一合并语义：正文/思考各自以换行拼接，图片单独收集。
    pub(crate) fn extract_parts(
        parts: &[tianyan::common::types::Part],
    ) -> (String, String, Vec<String>) {
        let mut thinking = String::new();
        let mut content = String::new();
        let mut images = Vec::new();
        for p in parts {
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
        (content, thinking, images)
    }

    /// 从核心消息构造 API 消息（**仅流式边界事件**用，轻量版）：
    /// id/role/content/thinking/images/usage/truncated/timestamp；
    /// tool_calls 不填——流式期间前端已累积完整工具卡片（含结果）。
    ///
    /// ⚠️ 权威历史视图（含事件订阅快照的 replace 兜底）必须用
    /// [`ChatMessage::messages_from_structured`]（跨消息合并工具结果）；
    /// 用它构造快照会让历史工具卡片丢结果、永久显示"运行中"（T0-7）。
    pub(crate) fn from_structured_light(m: &tianyan::common::types::StructuredMessage) -> Self {
        let (_, thinking, images) = Self::extract_parts(&m.parts);
        Self {
            seq: None,
            id: Some(m.id.clone()),
            role: m.role,
            user_message_id: None,
            content: None,
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
            compression_marker: m.compression_marker,
            timestamp: None,
            // 时间线：与历史加载同构（方案 B）——流式边界事件携带服务端
            // 权威 segments，前端 applyServerMessage 以服务端为准
            segments: Self::segments_from_parts(&m.role, &m.parts),
        }
    }

    /// 从核心会话消息列表构造 API 展示消息（**完整转换**——历史加载
    /// `SessionService::get_session_detail` 与事件订阅快照
    /// （`subscribe_events`）共用同一语义）。
    ///
    /// 与 [`ChatMessage::from_structured_light`]（轻量版：流式期间前端已累积
    /// 完整工具卡片）不同，本函数产出**权威历史视图**：
    ///
    /// - **跨消息合并工具结果**：JSONL 中结果位于独立 `role=tool` 消息
    ///   （`Part::ToolResult`），按 `tool_call_id` 挂到对应调用卡片
    ///   （`result` + `duration_ms` + `error`）；无对应调用的孤立结果输出
    ///   结果-only 卡片；
    /// - `role=Tool` 映射为 `Assistant`（结果已并入卡片，前端不消费 tool 角色）；
    /// - 无任何展示内容（正文/思考/工具卡片/图片皆空）的 `role=tool` 消息跳过
    ///   （避免空气泡）；assistant 空消息**保留**——前端唤醒轮询以"出现新的
    ///   assistant 消息"为停止信号，过滤会导致指示器永不停止；
    /// - 正文/思考/图片合并语义与 [`Self::extract_parts`] 一致。
    ///
    /// 为什么必须同源：订阅快照是前端打开/重连会话时的 **replace 权威兜底**；
    /// 若快照走轻量转换（`tool_calls: None`），历史工具卡片丢结果，
    /// 前端按"无结果"渲染成永久"运行中"转圈（误导为仍在执行）。
    pub(crate) fn messages_from_structured(
        messages: Vec<tianyan::common::types::StructuredMessage>,
    ) -> Vec<ChatMessage> {
        // 无链上位置上下文（订阅快照即时转换等）：seq 留空。
        Self::convert_messages(messages.into_iter().map(|m| (None, m)).collect())
    }

    /// 带链上位置的历史转换（ADR-035 §8：分段加载 / 订阅快照按 seq 对齐）。
    pub(crate) fn messages_from_structured_with_seq(
        messages: Vec<(i64, tianyan::common::types::StructuredMessage)>,
    ) -> Vec<ChatMessage> {
        Self::convert_messages(messages.into_iter().map(|(s, m)| (Some(s), m)).collect())
    }

    /// 转换核心（`Option<i64>` = 链上位置，None 表示无位置上下文）。
    fn convert_messages(
        messages: Vec<(Option<i64>, tianyan::common::types::StructuredMessage)>,
    ) -> Vec<ChatMessage> {
        /// 工具执行结果元数据（跨消息合并用）。
        struct ToolResultMeta {
            content: String,
            duration_ms: Option<i64>,
            error: Option<String>,
        }

        // 先扫一遍收集结果：结果消息与调用消息的先后顺序不作假设。
        let mut result_map: std::collections::HashMap<String, ToolResultMeta> =
            std::collections::HashMap::new();
        for (_, m) in &messages {
            for p in &m.parts {
                if let tianyan::common::types::Part::ToolResult {
                    tool_call_id,
                    content,
                    error,
                    time,
                } = p
                {
                    let duration_ms = if time.end > 0 && time.start > 0 {
                        Some((time.end - time.start).max(0))
                    } else {
                        None
                    };
                    result_map.insert(
                        tool_call_id.clone(),
                        ToolResultMeta {
                            content: content.clone(),
                            duration_ms,
                            error: error.clone(),
                        },
                    );
                }
            }
        }

        messages
            .into_iter()
            .filter_map(|(seq, m)| {
                let (content, thinking, images) = Self::extract_parts(&m.parts);
                let mut tool_calls: Vec<ToolCallWithResult> = Vec::new();
                for p in &m.parts {
                    match p {
                        tianyan::common::types::Part::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            // 结果已跨消息收集：此处取出挂到卡片上
                            // （含耗时/失败元数据，前端卡片显示 ✓/✗ + 耗时）
                            let meta = result_map.remove(id);
                            tool_calls.push(ToolCallWithResult {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                presentation: tianyan::agent::ToolRegistry::default_presentation(
                                    name,
                                )
                                .as_str()
                                .to_string(),
                                result: meta.as_ref().map(|r| r.content.clone()),
                                duration_ms: meta.as_ref().and_then(|r| r.duration_ms),
                                error: meta.as_ref().and_then(|r| r.error.clone()),
                            });
                        }
                        // guard：remove 返回 None → 结果已被前置调用卡片消费
                        // （已合并进调用卡片）→ 落到 `_` 跳过；Some → 结果仍在
                        // map 中（无对应调用，孤立结果）→ 输出结果-only 卡片。
                        tianyan::common::types::Part::ToolResult {
                            tool_call_id,
                            content: result_content,
                            error,
                            time,
                        } if result_map.remove(tool_call_id).is_some() => {
                            let duration_ms = if time.end > 0 && time.start > 0 {
                                Some((time.end - time.start).max(0))
                            } else {
                                None
                            };
                            tool_calls.push(ToolCallWithResult {
                                id: tool_call_id.clone(),
                                name: String::new(),
                                arguments: String::new(),
                                presentation: "generic".to_string(),
                                result: Some(result_content.clone()),
                                duration_ms,
                                error: error.clone(),
                            });
                        }
                        _ => {}
                    }
                }
                // 工具结果被合并进调用卡片后，原 role=tool 消息无任何可展示
                // 内容 → 跳过该消息（避免空气泡）；孤立结果消息保留。
                // 注意：assistant 空消息（唤醒轮空输出等）**必须保留**。
                if content.is_empty()
                    && thinking.is_empty()
                    && tool_calls.is_empty()
                    && images.is_empty()
                    && m.role == MessageRole::Tool
                {
                    return None;
                }
                // 历史消息携带持久化 usage（DetailedTokenUsage → API TokenUsage）：
                // 前端按会话独立计算上下文占用 / 缓存命中，避免跨会话串值。
                let usage = (m.tokens.total > 0 || m.tokens.input > 0).then_some(TokenUsage {
                    prompt_tokens: m.tokens.input as u32,
                    completion_tokens: m.tokens.output as u32,
                    total_tokens: m.tokens.total as u32,
                    cache_read: m.tokens.cache.read as u32,
                    cache_write: m.tokens.cache.write as u32,
                });
                Some(ChatMessage {
                    // 消息 ID：回退/重做的定位键（前端按 ID 调删除/恢复）
                    id: Some(m.id),
                    // 链上位置（位置语义：rewrite 后重排）
                    seq,
                    // Tool 角色在 API 层映射为 Assistant（与旧 core_bridge 转换
                    // 一致），工具结果已合并进 tool_calls.result，前端不消费 tool 角色。
                    role: match m.role {
                        MessageRole::Tool => MessageRole::Assistant,
                        role => role,
                    },
                    user_message_id: None,
                    content: None,
                    thinking: if thinking.is_empty() {
                        None
                    } else {
                        Some(thinking)
                    },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    images: if images.is_empty() {
                        None
                    } else {
                        Some(images)
                    },
                    // finish=length（token 上限或流式中断）：历史加载与流式
                    // 渲染一致地显示截断提示。
                    truncated_by_length: m.finish.as_deref() == Some("length"),
                    interrupted: m.finish.as_deref() == Some("interrupted"),
                    usage,
                    // 压缩摘要标记：历史加载与压缩响应一致（前端识别摘要消息）
                    compression_marker: m.compression_marker,
                    timestamp: None,
                    // 时间线（方案 B）：parts 顺序 = 真实到达顺序——历史与
                    // 流式共用同一渲染管线（前端 SegmentBlocks）
                    segments: Self::segments_from_parts(&m.role, &m.parts),
                })
            })
            .collect()
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
            seq: None,
            id: None,
            role: MessageRole::System,
            user_message_id: None,
            // 构造器同时填 content（请求方向：用户输入契约）与 segments
            // （响应方向：正文进时间线 Text 段——渲染/复制前端一律从
            // segments 取，content 仅输入侧消费）
            content: Some(content.to_string()),
            segments: Some(vec![MessageSegment::Text {
                text: content.to_string(),
            }]),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            interrupted: false,
            usage: None,
            compression_marker: false,
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
            seq: None,
            id: None,
            role: MessageRole::User,
            user_message_id: None,
            content: Some(content.to_string()),
            segments: Some(vec![MessageSegment::Text {
                text: content.to_string(),
            }]),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            interrupted: false,
            usage: None,
            compression_marker: false,
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
            seq: None,
            id: None,
            role: MessageRole::Assistant,
            user_message_id: None,
            content: Some(content.to_string()),
            segments: Some(vec![MessageSegment::Text {
                text: content.to_string(),
            }]),
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            interrupted: false,
            usage: None,
            compression_marker: false,
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
    fn test_segments_for_plain_messages() {
        // 所有角色的正文都进时间线（Text 段）——用户/系统消息不再依赖
        // "无 segments 兜底"，前端渲染统一走 SegmentBlocks
        let sm = tianyan::common::types::StructuredMessage::system("sess-1", "系统消息");
        let segs = ChatMessage::segments_from_parts(&sm.role, &sm.parts).expect("segments");
        assert_eq!(segs.len(), 1);
        match &segs[0] {
            MessageSegment::Text { text } => {
                assert_eq!(text, "系统消息");
            }
            other => panic!("应为 Text 段，实际: {other:?}"),
        }
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
