//! 视觉模型专用类型。
//!
//! 这些类型与 [`ChatCompletionRequest`] 独立，原因是：
//! - Vision API 的消息结构（multipart content）与标准 chat 有差异
//! - 调用方（VlmService）需要更简洁的接口，而非暴露 chat 全部参数
//! - 未来若视觉模型 API 分化（如 Gemini），可独立演进

use serde::{Deserialize, Serialize};

use crate::common::types::ContentPart;
use crate::common::types::TokenUsage;

/// 视觉模型请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionRequest {
    /// 模型名称。
    pub model: String,
    /// 消息列表。
    pub messages: Vec<VisionMessage>,
    /// 最大生成 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// 采样温度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// 视觉模型消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionMessage {
    /// 消息角色。
    pub role: String,
    /// 消息内容。
    pub content: VisionContent,
}

/// 视觉内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VisionContent {
    /// 纯文本。
    Text(String),
    /// 多模态片段。
    MultiPart(Vec<ContentPart>),
}

impl VisionRequest {
    /// 创建新的视觉请求。
    pub fn new(model: impl Into<String>, messages: Vec<VisionMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            temperature: None,
        }
    }

    /// 设置最大 token 数。
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

/// 视觉模型响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionResponse {
    /// 响应 ID。
    pub id: String,
    /// 对象类型。
    pub object: String,
    /// 创建时间戳。
    pub created: i64,
    /// 使用的模型。
    pub model: String,
    /// 选择列表。
    pub choices: Vec<VisionChoice>,
    /// Token 使用量。
    pub usage: TokenUsage,
}

/// 单个视觉选择项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionChoice {
    /// 选择索引。
    pub index: usize,
    /// 消息内容。
    pub message: VisionMessage,
    /// 结束原因。
    pub finish_reason: Option<String>,
}
