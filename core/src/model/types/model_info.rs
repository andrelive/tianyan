use serde::{Deserialize, Serialize};

/// 模型元信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// 模型 ID。
    pub id: String,
    /// 模型显示名称。
    pub name: String,
    /// 提供方标识。
    pub provider: String,
    /// 模型类型。
    pub model_type: ModelType,
    /// 最大上下文长度（token 数）。
    pub max_context_length: usize,
    /// 支持的能力列表。
    pub capabilities: Vec<ModelCapability>,
}

/// 模型类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelType {
    /// 聊天模型。
    Chat,
    /// 嵌入模型。
    Embedding,
    /// 视觉模型。
    Vision,
}

/// 模型能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    /// 支持聊天补全。
    Chat,
    /// 支持函数调用。
    FunctionCalling,
    /// 支持视觉输入。
    Vision,
    /// 支持流式输出。
    Streaming,
    /// 支持 JSON 模式。
    JsonMode,
}

/// 模型提供方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ModelProvider {
    /// OpenAI 官方。
    #[default]
    OpenAI,
    /// OpenAI 兼容 API。
    OpenAICompatible,
}

impl std::fmt::Display for ModelProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelProvider::OpenAI => write!(f, "openai"),
            ModelProvider::OpenAICompatible => write!(f, "openai-compatible"),
        }
    }
}
