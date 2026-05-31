use serde::{Deserialize, Serialize};

use super::message::MessageRole;

/// Token 使用详情，含缓存命中信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetailedTokenUsage {
    #[serde(default)]
    pub input: usize,
    #[serde(default)]
    pub output: usize,
    #[serde(default)]
    pub reasoning: usize,
    #[serde(default)]
    pub cache: CacheUsage,
    #[serde(default)]
    pub total: usize,
}

/// 缓存 Token 统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheUsage {
    #[serde(default)]
    pub read: usize,
    #[serde(default)]
    pub write: usize,
}

/// Part 级别的时间戳（毫秒）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartTime {
    #[serde(default)]
    pub start: i64,
    #[serde(default)]
    pub end: i64,
}

/// Message 级别的时间戳。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageTime {
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub completed: i64,
}

/// 结构化消息中的内容块。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Part {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default)]
        time: PartTime,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        text: String,
        #[serde(default)]
        time: PartTime,
    },
    #[serde(rename = "tool_call")]
    ToolCall {
        id: String,
        name: String,
        arguments: String,
        #[serde(default)]
        time: PartTime,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_call_id: String,
        content: String,
        #[serde(default)]
        time: PartTime,
    },
}

/// 面向持久化的结构化消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredMessage {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_id: Option<String>,
    pub role: MessageRole,
    pub parts: Vec<Part>,
    #[serde(default)]
    pub tokens: DetailedTokenUsage,
    #[serde(default)]
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub time: MessageTime,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish: Option<String>,
    #[serde(default)]
    pub compression_marker: bool,
}
