use serde::{Deserialize, Serialize};

use super::message::MessageRole;

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

#[cfg(test)]
mod tests {
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
}
