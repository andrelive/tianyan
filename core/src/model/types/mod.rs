mod api_error;
mod chat;
mod embedding;
mod model_info;
mod streaming;
mod tool;
mod vision;

pub use crate::common::types::{ContentPart, ImageUrl};
pub use api_error::{ApiError, ApiErrorResponse};
pub use chat::{ChatChoice, ChatCompletionRequest, ChatCompletionResponse};
pub use embedding::{
    embedding_dimension, EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse,
};
pub use model_info::{ModelCapability, ModelInfo, ModelProvider, ModelType};
pub use streaming::{
    ChatCompletionChunk, ChunkChoice, DeltaContent, ToolCallDelta, ToolCallFunctionDelta,
};
pub use tool::{
    FunctionCall, FunctionDefinition, ToolCall, ToolCallType, ToolChoice, ToolChoiceFunction,
    ToolDefinition, ToolType,
};
pub use vision::{VisionChoice, VisionContent, VisionMessage, VisionRequest, VisionResponse};
