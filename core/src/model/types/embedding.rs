use serde::{Deserialize, Serialize};

use crate::common::types::TokenUsage;

/// 嵌入请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    /// 模型名称。
    pub model: String,
    /// 嵌入输入。
    pub input: EmbeddingInput,
    /// 编码格式。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
    /// 向量维度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<usize>,
}

/// 嵌入输入（单文本或多文本）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    /// 单文本输入。
    Single(String),
    /// 多文本输入。
    Multiple(Vec<String>),
}

impl EmbeddingRequest {
    /// 创建单文本嵌入请求。
    pub fn new(model: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            input: EmbeddingInput::Single(text.into()),
            encoding_format: None,
            dimensions: None,
        }
    }

    /// 创建批量文本嵌入请求。
    pub fn new_batch(model: impl Into<String>, texts: Vec<String>) -> Self {
        Self {
            model: model.into(),
            input: EmbeddingInput::Multiple(texts),
            encoding_format: None,
            dimensions: None,
        }
    }

    /// 设置向量维度。
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = Some(dimensions);
        self
    }
}

/// 嵌入响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    /// 对象类型。
    pub object: String,
    /// 嵌入数据列表。
    pub data: Vec<EmbeddingData>,
    /// 使用的模型。
    pub model: String,
    /// Token 用量。
    pub usage: TokenUsage,
}

/// 嵌入数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingData {
    /// 数据索引。
    pub index: usize,
    /// 嵌入向量。
    pub embedding: Vec<f32>,
    /// 对象类型。
    pub object: String,
}

/// 获取模型的嵌入维度。
pub fn embedding_dimension(model: &str) -> usize {
    match model {
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        "text-embedding-ada-002" => 1536,
        "text-embedding-v3" => 1024,
        "text-embedding-v4" => 1024,
        _ => 1536,
    }
}
