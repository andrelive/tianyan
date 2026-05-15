use serde::{Deserialize, Serialize};

use crate::common::types::{Message, MessageRole};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: usize,
    pub delta: DeltaContent,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<MessageRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl ChatCompletionChunk {
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
