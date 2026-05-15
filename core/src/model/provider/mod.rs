//! OpenAI 兼容 API provider 实现。
//!
//! 本模块基于 async-openai 库，提供 OpenAI 兼容 API 的客户端实现。
//! 支持聊天补全、文本嵌入和视觉分析。

mod chat_service;
pub mod client;
mod discovery_service;
mod embedding_service;
pub(crate) mod middleware;
mod vision_service;

pub use client::AsyncOpenAIClient;

use async_openai::types::chat::FinishReason;

pub(crate) fn finish_reason_str(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::FunctionCall => "function_call",
    }
}
