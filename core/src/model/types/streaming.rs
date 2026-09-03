//! 流式聊天补全类型，与 [`ChatCompletionResponse`] 配套使用。

use serde::{Deserialize, Serialize};

use crate::common::types::{MessageRole, TokenUsage};

/// 流式 tool call 函数增量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunctionDelta {
    /// 函数名称。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 函数参数（增量）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// 流式 tool call 增量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    /// tool call 在本次响应中的索引。
    pub index: usize,
    /// tool call ID（首次出现时设置）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// tool call 类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    /// 函数调用增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<ToolCallFunctionDelta>,
}

/// 流式聊天补全响应块。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    /// 响应 ID。
    pub id: String,
    /// 对象类型。
    pub object: String,
    /// 创建时间戳。
    pub created: i64,
    /// 使用的模型。
    pub model: String,
    /// 选择列表。
    pub choices: Vec<ChunkChoice>,
    /// Token 用量（仅在最后一个 chunk 中提供）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

/// 单个流式选择项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// 选择索引。
    pub index: usize,
    /// 增量内容。
    pub delta: DeltaContent,
    /// 结束原因。
    pub finish_reason: Option<String>,
}

/// 增量消息内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaContent {
    /// 消息角色（可选）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<MessageRole>,
    /// 文本内容（可选）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 思考过程增量（DeepSeek 等思考模型的 `reasoning_content` 字段；
    /// ollama 网关用 `reasoning` 字段名，serde alias 统一解析；
    /// async-openai 0.34 不解析该字段，由手写流式解析填充）。
    #[serde(alias = "reasoning", skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// 流式 tool calls 增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ollama 网关用 `reasoning` 字段名，DeepSeek 用 `reasoning_content`——
    /// serde alias 必须统一解析两种形态（思考过程增量）。
    #[test]
    fn delta_content_parses_both_reasoning_field_names() {
        let ollama: DeltaContent = serde_json::from_value(serde_json::json!({
            "content": "好",
            "reasoning": "我们只需要回答一个字",
        }))
        .unwrap();
        assert_eq!(
            ollama.reasoning_content.as_deref(),
            Some("我们只需要回答一个字")
        );

        let deepseek: DeltaContent = serde_json::from_value(serde_json::json!({
            "content": "好",
            "reasoning_content": "思考中",
        }))
        .unwrap();
        assert_eq!(deepseek.reasoning_content.as_deref(), Some("思考中"));
    }
}
