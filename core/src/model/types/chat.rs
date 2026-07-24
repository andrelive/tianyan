use serde::{Deserialize, Serialize};

use crate::common::types::{Message, TokenUsage};
use crate::model::types::{ToolChoice, ToolDefinition};

/// 聊天补全请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// 模型名称。
    pub model: String,
    /// 对话消息列表。
    pub messages: Vec<Message>,
    /// 最大输出 Token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// 采样温度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// 核采样参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 返回候选数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<usize>,
    /// 是否流式输出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 停止序列。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// 存在惩罚。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// 频率惩罚。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// 用户标识。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// 是否启用思维链。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    /// 可调用的工具列表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// 工具调用选择策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

impl ChatCompletionRequest {
    /// 创建聊天补全请求。
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            temperature: None,
            top_p: None,
            n: None,
            stream: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            enable_thinking: None,
            tools: None,
            tool_choice: None,
        }
    }

    /// 创建简单单轮请求。
    pub fn simple(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self::new(model, vec![Message::user(prompt)])
    }

    /// 设置最大输出 Token 数。
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// 设置采样温度。
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// 设置核采样参数。
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// 设置流式输出。
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    /// 设置停止序列。
    pub fn with_stop(mut self, stop: Vec<String>) -> Self {
        self.stop = Some(stop);
        self
    }

    /// 设置是否启用思维链。
    pub fn with_enable_thinking(mut self, enable_thinking: bool) -> Self {
        self.enable_thinking = Some(enable_thinking);
        self
    }

    /// 设置可调用工具。
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// 设置工具调用策略。
    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }
}

/// 聊天补全响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// 响应 ID。
    pub id: String,
    /// 对象类型。
    pub object: String,
    /// 创建时间戳。
    pub created: i64,
    /// 使用的模型。
    pub model: String,
    /// 候选回复列表。
    pub choices: Vec<ChatChoice>,
    /// Token 用量。
    pub usage: TokenUsage,
}

/// 聊天补全候选回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    /// 候选索引。
    pub index: usize,
    /// 回复消息。
    pub message: Message,
    /// 完成原因。
    pub finish_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::{FunctionDefinition, ToolChoice, ToolDefinition};

    #[test]
    fn test_request_with_tools() {
        let tool = ToolDefinition::function(FunctionDefinition::new(
            "search",
            "Search files",
            serde_json::json!({"type": "object"}),
        ));
        let request = ChatCompletionRequest::new("gpt-4", vec![Message::user("hello")])
            .with_tools(vec![tool])
            .with_tool_choice(ToolChoice::auto());

        assert!(request.tools.is_some());
        assert_eq!(request.tools.as_ref().unwrap()[0].function.name, "search");
        assert!(matches!(request.tool_choice, Some(ToolChoice::Auto)));
    }

    #[test]
    fn test_request_serialization_omits_none() {
        let request = ChatCompletionRequest::new("gpt-4", vec![Message::user("hello")]);
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("tools"));
        assert!(!json.contains("tool_choice"));
    }
}
