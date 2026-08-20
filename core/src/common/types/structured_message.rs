use serde::{Deserialize, Serialize};

use super::message::{Message, MessageRole};
use super::token::TokenUsage;

/// Token 使用详情，含缓存命中信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetailedTokenUsage {
    /// 输入 Token 数。
    #[serde(default)]
    pub input: usize,
    /// 输出 Token 数。
    #[serde(default)]
    pub output: usize,
    /// 推理 Token 数。
    #[serde(default)]
    pub reasoning: usize,
    /// 缓存命中统计。
    #[serde(default)]
    pub cache: CacheUsage,
    /// 总 Token 数。
    #[serde(default)]
    pub total: usize,
}

/// 缓存 Token 统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheUsage {
    /// 读取缓存 Token 数。
    #[serde(default)]
    pub read: usize,
    /// 写入缓存 Token 数。
    #[serde(default)]
    pub write: usize,
}

/// Part 级别的时间戳（毫秒）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartTime {
    /// 开始时间（毫秒）。
    #[serde(default)]
    pub start: i64,
    /// 结束时间（毫秒）。
    #[serde(default)]
    pub end: i64,
}

/// Message 级别的时间戳。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageTime {
    /// 创建时间戳。
    #[serde(default)]
    pub created: i64,
    /// 完成时间戳。
    #[serde(default)]
    pub completed: i64,
}

/// 结构化消息中的内容块。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Part {
    /// 文本内容。
    #[serde(rename = "text")]
    Text {
        /// 文本内容。
        text: String,
        /// 时间戳。
        #[serde(default)]
        time: PartTime,
    },
    /// 推理内容。
    #[serde(rename = "reasoning")]
    Reasoning {
        /// 推理文本。
        text: String,
        /// 时间戳。
        #[serde(default)]
        time: PartTime,
    },
    /// 工具调用。
    #[serde(rename = "tool_call")]
    ToolCall {
        /// 调用 ID。
        id: String,
        /// 工具名称。
        name: String,
        /// 调用参数。
        arguments: String,
        /// 时间戳。
        #[serde(default)]
        time: PartTime,
    },
    /// 工具执行结果。
    #[serde(rename = "tool_result")]
    ToolResult {
        /// 对应的工具调用 ID。
        tool_call_id: String,
        /// 执行结果内容。
        content: String,
        /// 执行失败原因（None = 成功；旧数据缺失视为成功）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// 时间戳。
        #[serde(default)]
        time: PartTime,
    },
    /// 用户消息中的图片（URL 或 `data:image/png;base64,...` data URL）。
    #[serde(rename = "image")]
    Image {
        /// 图片 URL 或 data URL。
        url: String,
        /// 时间戳。
        #[serde(default)]
        time: PartTime,
    },
}

/// 面向持久化的结构化消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredMessage {
    /// 消息唯一 ID。
    pub id: String,
    /// 父消息 ID。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_id: Option<String>,
    /// 消息角色。
    pub role: MessageRole,
    /// 内容块列表。
    pub parts: Vec<Part>,
    /// Token 使用详情。
    #[serde(default)]
    pub tokens: DetailedTokenUsage,
    /// 成本（美元）。
    #[serde(default)]
    pub cost: f64,
    /// 模型 ID。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model_id: Option<String>,
    /// 时间戳信息。
    #[serde(default)]
    pub time: MessageTime,
    /// 会话 ID。
    pub session_id: String,
    /// 完成原因。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish: Option<String>,
    /// 会话压缩标记。
    #[serde(default)]
    pub compression_marker: bool,
}

impl StructuredMessage {
    /// 构造纯文本单 part 消息（`system` / `user` / `assistant` 共用内部实现）。
    ///
    /// 统一规则：`id = msg_{now}`（now 为 epoch 毫秒）、
    /// `time.created == time.completed == now`、token/成本/模型/finish/压缩标记
    /// 取默认值、`parent_id = None`。
    fn text_message(
        role: MessageRole,
        session_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id: format!("msg_{now}"),
            parent_id: None,
            role,
            parts: vec![Part::Text {
                text: text.into(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now,
                completed: now,
            },
            session_id: session_id.into(),
            finish: None,
            compression_marker: false,
        }
    }

    /// 创建系统消息（单文本 part）。
    pub fn system(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::text_message(MessageRole::System, session_id, text)
    }

    /// 创建用户消息（单文本 part）。
    pub fn user(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::text_message(MessageRole::User, session_id, text)
    }

    /// 创建携带图片的用户消息（文本 part + 每张图片一个 `Part::Image`）。
    ///
    /// - `session_id` - 会话 ID
    /// - `text` - 用户文本（可为空，此时仍保留文本 part 占位）
    /// - `images` - 图片 URL / data URL 列表，按顺序排在文本 part 之后
    pub fn user_with_images(
        session_id: impl Into<String>,
        text: impl Into<String>,
        images: Vec<String>,
    ) -> Self {
        let mut msg = Self::user(session_id, text);
        for url in images {
            msg.parts.push(Part::Image {
                url,
                time: PartTime::default(),
            });
        }
        msg
    }

    /// 创建助手消息（单文本 part）。
    pub fn assistant(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::text_message(MessageRole::Assistant, session_id, text)
    }

    /// 将传输层 `Message` 全保真转换为 `StructuredMessage`（用于持久化）。
    ///
    /// 角色从 `msg.role` 直接派生。对于 Tool 消息，生成 `Part::ToolResult`；
    /// 对于其他角色，分别提取 reasoning content、text content、content_parts
    /// 中的图片（→ `Part::Image`）和 tool calls。
    /// `token_usage` 来自 LLM 响应的实际 token 统计，为 `None` 时使用默认值。
    ///
    /// 全链路唯一的 Message→StructuredMessage 转换实现点（会话组装器、
    /// 会话创建等路径共用，保证图片等非文本内容不丢失）。
    pub fn from_message(
        msg: &Message,
        session_id: &str,
        parent_id: Option<&str>,
        token_usage: Option<TokenUsage>,
    ) -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let default_time = PartTime::default();
        let role = msg.role;

        let mut parts = Vec::new();

        match role {
            MessageRole::Tool => {
                // 计时接线：工具执行耗时写入 Part 级时间戳（start = 完成时刻 - 耗时），
                // 失败原因结构化存储（此前 time 恒为 0、失败只能从 content 解析）。
                let duration = msg.tool_duration_ms.unwrap_or(0).max(0);
                parts.push(Part::ToolResult {
                    tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
                    content: msg.content.clone(),
                    error: msg.tool_error.clone(),
                    time: PartTime {
                        start: now_ms - duration,
                        end: now_ms,
                    },
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

                // 多模态片段中的图片 → Part::Image（持久化 data URL，供历史重放）
                if let Some(ref content_parts) = msg.content_parts {
                    for cp in content_parts {
                        if let Some(image_url) = &cp.image_url {
                            parts.push(Part::Image {
                                url: image_url.url.clone(),
                                time: default_time.clone(),
                            });
                        }
                    }
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

        Self {
            id: format!("msg_{now_ms}"),
            parent_id: parent_id.map(|s| s.to_string()),
            role,
            parts,
            tokens: token_usage
                .map(|tu| DetailedTokenUsage {
                    input: tu.prompt_tokens,
                    output: tu.completion_tokens,
                    total: tu.total_tokens,
                    // 缓存明细必须保留：历史加载/全局统计依赖 cache.read/write，
                    // 此前 ..Default::default() 把缓存命中信息丢弃（刷新后显示 0）
                    cache: CacheUsage {
                        read: tu.cache_read,
                        write: tu.cache_write,
                    },
                    ..Default::default()
                })
                .unwrap_or_default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now_ms,
                completed: now_ms,
            },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::content_part::ContentPart;
    use super::super::tool::{FunctionCall, ToolCall, ToolCallType};
    use super::*;

    #[test]
    fn test_structured_message_jsonl_roundtrip() {
        let sm = StructuredMessage {
            id: "msg-1".to_string(),
            parent_id: Some("msg-0".to_string()),
            role: MessageRole::Assistant,
            parts: vec![
                Part::Reasoning {
                    text: "I need to read the file".to_string(),
                    time: PartTime::default(),
                },
                Part::ToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"foo.rs"}"#.to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: DetailedTokenUsage {
                input: 100,
                output: 50,
                total: 150,
                ..Default::default()
            },
            cost: 0.001,
            model_id: Some("gpt-4".to_string()),
            time: MessageTime {
                created: 1000,
                completed: 2000,
            },
            session_id: "ses-1".to_string(),
            finish: Some("stop".to_string()),
            compression_marker: false,
        };

        let json = serde_json::to_string(&sm).unwrap();
        let restored: StructuredMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, "msg-1");
        assert_eq!(restored.parent_id.as_deref(), Some("msg-0"));
        assert_eq!(restored.role, MessageRole::Assistant);
        assert_eq!(restored.parts.len(), 2);
        assert!(matches!(restored.parts[0], Part::Reasoning { .. }));
        assert!(matches!(restored.parts[1], Part::ToolCall { .. }));
        assert_eq!(restored.tokens.total, 150);
        assert!((restored.cost - 0.001).abs() < 0.0001);
        assert_eq!(restored.model_id.as_deref(), Some("gpt-4"));
        assert_eq!(restored.session_id, "ses-1");
        assert!(!restored.compression_marker);
    }

    #[test]
    fn test_compression_marker_default_false() {
        let json = r#"{"id":"m1","role":"user","parts":[],"session_id":"s1"}"#;
        let sm: StructuredMessage = serde_json::from_str(json).unwrap();
        assert!(
            !sm.compression_marker,
            "compression_marker should default to false"
        );
    }

    #[test]
    fn test_compression_marker_true_roundtrip() {
        let sm = StructuredMessage {
            id: "cmp-1".to_string(),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![Part::Text {
                text: "Summary".to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses-1".to_string(),
            finish: None,
            compression_marker: true,
        };
        let json = serde_json::to_string(&sm).unwrap();
        assert!(json.contains("\"compression_marker\":true"));
        let restored: StructuredMessage = serde_json::from_str(&json).unwrap();
        assert!(restored.compression_marker);
    }

    #[test]
    fn test_part_text_serialization() {
        let part = Part::Text {
            text: "Hello".to_string(),
            time: PartTime::default(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello\""));
        let restored: Part = serde_json::from_str(&json).unwrap();
        match restored {
            Part::Text { text, .. } => assert_eq!(text, "Hello"),
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn test_part_tool_call_serialization() {
        let part = Part::ToolCall {
            id: "tc-1".to_string(),
            name: "execute_command".to_string(),
            arguments: r#"{"cmd":"ls"}"#.to_string(),
            time: PartTime::default(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        let restored: Part = serde_json::from_str(&json).unwrap();
        match restored {
            Part::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                assert_eq!(id, "tc-1");
                assert_eq!(name, "execute_command");
                assert_eq!(arguments, r#"{"cmd":"ls"}"#);
            }
            _ => panic!("expected ToolCall variant"),
        }
    }

    #[test]
    fn test_part_tool_result_serialization() {
        let part = Part::ToolResult {
            tool_call_id: "tc-1".to_string(),
            content: "result data".to_string(),
            error: None,
            time: PartTime::default(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"tool_result\""));
        let restored: Part = serde_json::from_str(&json).unwrap();
        match restored {
            Part::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                assert_eq!(tool_call_id, "tc-1");
                assert_eq!(content, "result data");
            }
            _ => panic!("expected ToolResult variant"),
        }
    }

    #[test]
    fn test_detailed_token_usage_default() {
        let usage = DetailedTokenUsage::default();
        assert_eq!(usage.input, 0);
        assert_eq!(usage.total, 0);
    }

    #[test]
    fn test_empty_parts_still_serializes() {
        let sm = StructuredMessage {
            id: "m".to_string(),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "s".to_string(),
            finish: None,
            compression_marker: false,
        };
        let json = serde_json::to_string(&sm).unwrap();
        assert!(json.contains("\"parts\":[]"));
    }

    // ── 构造函数（T5）────────────────────────────────────────────

    #[test]
    fn test_system_constructor_invariants() {
        let sm = StructuredMessage::system("ses_1", "你好");
        assert_eq!(sm.role, MessageRole::System);
        assert_eq!(sm.session_id, "ses_1");
        assert!(sm.id.starts_with("msg_"), "id 应以 msg_ 开头: {}", sm.id);
        assert!(sm.parent_id.is_none(), "parent_id 默认为 None");
        assert_eq!(sm.parts.len(), 1);
        match &sm.parts[0] {
            Part::Text { text, .. } => assert_eq!(text, "你好"),
            other => panic!("应为 Text part，实际: {:?}", other),
        }
        assert_eq!(
            sm.time.created, sm.time.completed,
            "时间戳 created == completed"
        );
        assert!(sm.time.created > 0, "时间戳应为 epoch 毫秒");
        assert_eq!(sm.tokens.input, 0);
        assert_eq!(sm.tokens.output, 0);
        assert_eq!(sm.tokens.total, 0);
        assert_eq!(sm.cost, 0.0);
        assert!(sm.model_id.is_none());
        assert!(sm.finish.is_none());
        assert!(!sm.compression_marker);
    }

    #[test]
    fn test_user_constructor() {
        let sm = StructuredMessage::user("ses_1", "hello");
        assert_eq!(sm.role, MessageRole::User);
        assert_eq!(sm.session_id, "ses_1");
        assert_eq!(sm.parts.len(), 1);
        assert!(matches!(&sm.parts[0], Part::Text { text, .. } if text == "hello"));
    }

    #[test]
    fn test_assistant_constructor() {
        let sm = StructuredMessage::assistant("ses_1", "reply");
        assert_eq!(sm.role, MessageRole::Assistant);
        assert_eq!(sm.session_id, "ses_1");
        assert!(matches!(&sm.parts[0], Part::Text { text, .. } if text == "reply"));
    }

    #[test]
    fn test_user_with_images_constructs_image_parts() {
        let urls = vec![
            "data:image/png;base64,AAAA".to_string(),
            "data:image/jpeg;base64,BBBB".to_string(),
        ];
        let sm = StructuredMessage::user_with_images("ses_1", "看图", urls.clone());
        assert_eq!(sm.role, MessageRole::User);
        assert_eq!(sm.parts.len(), 3, "文本 + 2 图片应产生 3 个 part");
        match &sm.parts[0] {
            Part::Text { text, .. } => assert_eq!(text, "看图"),
            other => panic!("第一个 part 应为文本: {:?}", other),
        }
        for (i, url) in urls.iter().enumerate() {
            match &sm.parts[i + 1] {
                Part::Image { url: u, .. } => assert_eq!(u, url, "图片 part 顺序应保持"),
                other => panic!("应为 Image part: {:?}", other),
            }
        }
    }

    #[test]
    fn test_from_message_fidelity_tool_message() {
        let msg = Message::tool("call_1", r#"{"result":"ok"}"#);
        let sm = StructuredMessage::from_message(&msg, "ses_1", Some("parent_1"), None);
        assert_eq!(sm.role, MessageRole::Tool);
        assert_eq!(sm.session_id, "ses_1");
        assert_eq!(sm.parent_id.as_deref(), Some("parent_1"));
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
            other => panic!("Tool 消息应生成 ToolResult part: {:?}", other),
        }
    }

    #[test]
    fn test_from_message_tool_result_timing_and_error_wiring() {
        // 计时/成败接线：tool_duration_ms → Part::ToolResult.time（start = 完成 - 耗时），
        // tool_error → error 字段（消息级元数据的持久化落点）。
        let msg = Message::tool_result("call_t", r#"{"result":"ok"}"#, Some(1200), None);
        let sm = StructuredMessage::from_message(&msg, "ses_1", None, None);
        let (start, end, error) = match &sm.parts[0] {
            Part::ToolResult { time, error, .. } => (time.start, time.end, error),
            other => panic!("应为 ToolResult: {:?}", other),
        };
        assert_eq!(error, &None, "成功时 error 应为 None");
        assert!(end > 0, "end 应为真实完成时刻");
        assert_eq!(end - start, 1200, "耗时差应等于 duration_ms");

        let failed = Message::tool_result(
            "call_t",
            r#"{"error":"boom"}"#,
            Some(50),
            Some("权限拒绝".into()),
        );
        let sm2 = StructuredMessage::from_message(&failed, "ses_1", None, None);
        let (start2, end2, error2) = match &sm2.parts[0] {
            Part::ToolResult { time, error, .. } => (time.start, time.end, error),
            other => panic!("应为 ToolResult: {:?}", other),
        };
        assert_eq!(error2.as_deref(), Some("权限拒绝"));
        assert_eq!(end2 - start2, 50);

        // 旧数据兼容：无元数据的 tool() 构造 → error None、time 为完成时刻自身
        let legacy = Message::tool("call_1", r#"{"result":"ok"}"#);
        let sm3 = StructuredMessage::from_message(&legacy, "ses_1", None, None);
        let Part::ToolResult {
            time: t3,
            error: er3,
            ..
        } = &sm3.parts[0]
        else {
            panic!("应为 ToolResult")
        };
        assert!(er3.is_none(), "旧数据缺失 error 应视为成功");
        assert_eq!(t3.start, t3.end, "无耗时信息时 start == end");
    }

    #[test]
    fn test_from_message_fidelity_assistant_reasoning_text_image_tool_call_order() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: "你好".to_string(),
            content_parts: Some(vec![
                ContentPart::text("你好"),
                ContentPart::image("data:image/png;base64,IMG1"),
            ]),
            tool_calls: Some(vec![ToolCall {
                id: "call_9".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.rs"}"#.to_string(),
                },
            }]),
            tool_duration_ms: None,
            tool_error: None,
            tool_call_id: None,
            reasoning_content: Some("先思考".to_string()),
        };
        let sm = StructuredMessage::from_message(&msg, "ses_1", None, None);
        assert_eq!(sm.role, MessageRole::Assistant);
        // 顺序必须与旧 message_to_structured 一致：reasoning → text → image → tool_call
        assert_eq!(sm.parts.len(), 4);
        match &sm.parts[0] {
            Part::Reasoning { text, .. } => assert_eq!(text, "先思考"),
            other => panic!("parts[0] 应为 Reasoning: {:?}", other),
        }
        match &sm.parts[1] {
            Part::Text { text, .. } => assert_eq!(text, "你好"),
            other => panic!("parts[1] 应为 Text: {:?}", other),
        }
        match &sm.parts[2] {
            Part::Image { url, .. } => assert_eq!(url, "data:image/png;base64,IMG1"),
            other => panic!("parts[2] 应为 Image: {:?}", other),
        }
        match &sm.parts[3] {
            Part::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                assert_eq!(id, "call_9");
                assert_eq!(name, "read_file");
                assert_eq!(arguments, r#"{"path":"a.rs"}"#);
            }
            other => panic!("parts[3] 应为 ToolCall: {:?}", other),
        }
    }

    #[test]
    fn test_from_message_skips_empty_text_and_empty_reasoning() {
        let empty_text = Message::assistant("");
        let sm = StructuredMessage::from_message(&empty_text, "ses_1", None, None);
        assert!(
            sm.parts.is_empty(),
            "空文本不应产生 Text part: {:?}",
            sm.parts
        );

        let empty_reasoning = Message {
            role: MessageRole::Assistant,
            content: "x".to_string(),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_duration_ms: None,
            tool_error: None,
            reasoning_content: Some("".to_string()),
        };
        let sm2 = StructuredMessage::from_message(&empty_reasoning, "ses_1", None, None);
        assert_eq!(
            sm2.parts.len(),
            1,
            "空 reasoning 不应产生 Reasoning part: {:?}",
            sm2.parts
        );
    }

    #[test]
    fn test_from_message_token_usage_mapping() {
        let msg = Message::assistant("hi");
        let sm =
            StructuredMessage::from_message(&msg, "ses_1", None, Some(TokenUsage::new(100, 50)));
        assert_eq!(sm.tokens.input, 100);
        assert_eq!(sm.tokens.output, 50);
        assert_eq!(sm.tokens.total, 150);

        let sm_default = StructuredMessage::from_message(&msg, "ses_1", None, None);
        assert_eq!(sm_default.tokens.input, 0);
        assert_eq!(sm_default.tokens.output, 0);
        assert_eq!(sm_default.tokens.total, 0);
        assert_eq!(sm_default.cost, 0.0);
        assert!(sm_default.model_id.is_none());
        assert!(!sm_default.compression_marker);
    }

    #[test]
    fn test_constructor_serde_roundtrip() {
        let sm = StructuredMessage::user_with_images(
            "ses_1",
            "看图",
            vec!["data:image/png;base64,AAAA".to_string()],
        );
        let json = serde_json::to_string(&sm).unwrap();
        let restored: StructuredMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, MessageRole::User);
        assert_eq!(restored.session_id, "ses_1");
        assert_eq!(restored.id, sm.id);
        assert_eq!(restored.parent_id, sm.parent_id);
        assert_eq!(restored.parts.len(), 2);
        assert!(matches!(
            &restored.parts[1],
            Part::Image { url, .. } if url == "data:image/png;base64,AAAA"
        ));
        assert_eq!(restored.time.created, restored.time.completed);
    }
}
