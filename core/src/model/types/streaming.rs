//! 流式聊天补全类型，与 [`ChatCompletionResponse`] 配套使用。

use serde::{Deserialize, Serialize};

use crate::common::types::{Message, MessageRole};

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
}

impl ChatCompletionChunk {
    /// 将增量内容追加到消息中。
    pub fn add_to_message(&self, message: &mut Message) {
        for choice in &self.choices {
            if let Some(ref role) = choice.delta.role {
                message.role = role.clone();
            }
            if let Some(ref content) = choice.delta.content {
                message.content.push_str(content);
            }
        }
    }
}
