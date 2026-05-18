//! OpenAI 兼容 API provider 实现。
//!
//! 本模块基于 async-openai 库，提供 OpenAI 兼容 API 的客户端实现。
//! 支持聊天补全、文本嵌入和视觉分析。
//!
//! 当前仅支持 OpenAI 兼容 API。
//! 若需新增非兼容 provider，应将本模块拆为 `openai/`、`anthropic/` 等子目录。

mod chat;
mod client;
pub(crate) mod middleware;
mod discovery;
mod embedding;
pub(crate) mod vision;

pub use client::AsyncOpenAIClient;
