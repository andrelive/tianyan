//! 上下文压缩模块。
//!
//! 提供智能的上下文压缩能力，对标 Hermes Agent 的上下文压缩机制：
//! - 当对话历史超过上下文窗口的 50% 时自动触发压缩
//! - 使用 LLM 生成对话摘要，保留关键信息
//! - 支持分层压缩（最近对话保留详情，早期对话压缩为摘要）
//! - 保留重要的执行结果和决策记录
//!
//! # 架构设计
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 ContextCompressor                            │
//! │  (上下文压缩入口，协调压缩策略和触发条件)                       │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │  Summarizer │  │   Selector  │  │     Estimator       │  │
//! │  │ (LLM 摘要)   │  │ (重要性选择) │  │ (Token 估算)         │  │
//! │  └─────────────┘  └─────────────┘  └─────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::Message;
use crate::model::ChatService;

pub mod estimator;

pub use estimator::{estimate_tokens, TokenEstimator};

/// 压缩策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionStrategy {
    /// 摘要：将早期对话压缩为摘要。
    Summarize,
    /// 选择：保留重要消息，丢弃次要消息。
    Select,
    /// 混合：先摘要再选择。
    Hybrid,
}

impl Default for CompressionStrategy {
    fn default() -> Self {
        Self::Hybrid
    }
}

/// 默认上下文窗口大小。
const DEFAULT_CONTEXT_WINDOW: usize = 128000;
/// 默认压缩阈值（窗口的百分比）。
const DEFAULT_COMPRESSION_THRESHOLD: f32 = 0.5;
/// 默认保留最近消息数。
const DEFAULT_PRESERVE_RECENT: usize = 10;
/// 默认最小压缩消息数。
const DEFAULT_MIN_MESSAGES_TO_COMPRESS: usize = 6;
/// Hybrid 压缩的消息数分割点。
const HYBRID_SPLIT_THRESHOLD: usize = 20;
/// Hybrid 中保留的最近消息数。
const HYBRID_PRESERVE_RECENT: usize = 10;
/// 临界阈值（窗口的百分比）。
const CRITICAL_THRESHOLD_RATIO: f32 = 0.8;

/// 上下文压缩配置。
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// 上下文窗口大小（token 数）。
    pub context_window: usize,
    /// 触发压缩的阈值（窗口的百分比）。
    pub compression_threshold: f32,
    /// 压缩策略。
    pub strategy: CompressionStrategy,
    /// 保留的最近消息数量（不压缩）。
    pub preserve_recent_messages: usize,
    /// 摘要模型。
    pub summary_model: String,
    /// 最小压缩消息数。
    pub min_messages_to_compress: usize,
    /// 最大摘要长度（token 数）。
    pub max_summary_tokens: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            context_window: DEFAULT_CONTEXT_WINDOW,
            compression_threshold: DEFAULT_COMPRESSION_THRESHOLD,
            strategy: CompressionStrategy::Hybrid,
            preserve_recent_messages: DEFAULT_PRESERVE_RECENT,
            summary_model: "gpt-4o-mini".to_string(),
            min_messages_to_compress: DEFAULT_MIN_MESSAGES_TO_COMPRESS,
            max_summary_tokens: 500,
        }
    }
}

/// 压缩结果。
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// 压缩后的消息列表。
    pub messages: Vec<Message>,
    /// 被压缩的消息数量。
    pub compressed_count: usize,
    /// 生成的摘要文本。
    pub summary: String,
    /// 压缩前的 token 估算。
    pub original_tokens: usize,
    /// 压缩后的 token 估算。
    pub compressed_tokens: usize,
}

impl CompressionResult {
    /// 计算压缩率。
    pub fn compression_ratio(&self) -> f32 {
        if self.original_tokens == 0 {
            0.0
        } else {
            1.0 - (self.compressed_tokens as f32 / self.original_tokens as f32)
        }
    }
}

/// 默认摘要提示词模板。
const DEFAULT_SUMMARY_PROMPT: &str = r#"请将以下对话历史压缩为简洁的摘要。要求：

1. 保留所有重要的事实、决策和结论
2. 保留用户明确表达的需求和偏好
3. 保留已完成的任务和取得的结果
4. 保留遇到的错误和解决方案
5. 删除重复信息和闲聊内容
6. 使用第三人称客观描述

对话历史：
{conversation}

请输出简洁的结构化摘要（不超过 500 字）："#;

/// 增量摘要提示词模板。
const INCREMENTAL_SUMMARY_PROMPT: &str = r#"请将以下新对话内容合并到已有摘要中，生成更新后的摘要。

已有摘要：
{existing_summary}

新对话内容：
{new_conversation}

要求：
1. 保留已有摘要中的所有重要信息
2. 将新对话中的关键信息合并进去
3. 删除重复内容
4. 保持简洁，不超过 {max_tokens} 字

请输出更新后的摘要："#;

/// 上下文压缩器。
///
/// 负责监控上下文大小并在必要时执行智能压缩。
#[derive(Clone)]
pub struct ContextCompressor {
    model_service: Arc<dyn ChatService>,
    estimator: TokenEstimator,
    config: CompressionConfig,
    /// 缓存的上次摘要，用于增量压缩。
    cached_summary: Option<String>,
}

impl ContextCompressor {
    /// 创建新的上下文压缩器。
    pub fn new(model_service: Arc<dyn ChatService>, config: CompressionConfig) -> Self {
        let estimator = TokenEstimator::new();

        Self {
            model_service,
            estimator,
            config,
            cached_summary: None,
        }
    }

    /// 检查是否需要压缩。
    pub fn should_compress(&self, messages: &[Message]) -> bool {
        if messages.len() < self.config.min_messages_to_compress {
            return false;
        }

        let estimated_tokens = self.estimator.estimate_messages(messages);
        let threshold =
            (self.config.context_window as f32 * self.config.compression_threshold) as usize;

        estimated_tokens > threshold
    }

    /// 压缩消息列表。
    ///
    /// - `messages` - 原始消息列表
    pub async fn compress(&mut self, messages: &[Message]) -> Result<CompressionResult> {
        if messages.len() <= self.config.preserve_recent_messages {
            return Ok(CompressionResult {
                messages: messages.to_vec(),
                compressed_count: 0,
                summary: String::new(),
                original_tokens: self.estimator.estimate_messages(messages),
                compressed_tokens: self.estimator.estimate_messages(messages),
            });
        }

        let original_tokens = self.estimator.estimate_messages(messages);

        // 分割：保留的最近消息 + 需要压缩的早期消息
        let split_point = messages
            .len()
            .saturating_sub(self.config.preserve_recent_messages);
        let early_messages = &messages[..split_point];
        let recent_messages = &messages[split_point..];

        // 根据策略执行压缩
        let (compressed_early, summary) = match self.config.strategy {
            CompressionStrategy::Summarize => {
                self.compress_by_summarization(early_messages).await?
            }
            CompressionStrategy::Select => self.compress_by_selection(early_messages).await?,
            CompressionStrategy::Hybrid => self.compress_hybrid(early_messages).await?,
        };

        // 合并：压缩后的早期消息 + 保留的最近消息
        let mut result_messages = compressed_early;
        result_messages.extend_from_slice(recent_messages);

        let compressed_tokens = self.estimator.estimate_messages(&result_messages);

        Ok(CompressionResult {
            messages: result_messages,
            compressed_count: early_messages.len(),
            summary,
            original_tokens,
            compressed_tokens,
        })
    }

    /// 通过摘要压缩。
    async fn compress_by_summarization(
        &mut self,
        messages: &[Message],
    ) -> Result<(Vec<Message>, String)> {
        // 如果有缓存摘要，使用增量摘要；否则全量摘要
        let summary = if let Some(ref cached) = self.cached_summary {
            self.incremental_summarize(cached, messages).await?
        } else {
            self.summarize(messages).await?
        };

        self.cached_summary = Some(summary.clone());

        let summary_message = Message::system(format!(
            "[对话摘要] 以下是对早期对话的摘要：\n{}\n[摘要结束]",
            summary
        ));

        Ok((vec![summary_message], summary))
    }

    /// LLM 全量摘要。
    async fn summarize(&self, messages: &[Message]) -> Result<String> {
        if messages.is_empty() {
            return Ok(String::new());
        }

        let conversation = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = DEFAULT_SUMMARY_PROMPT.replace("{conversation}", &conversation);

        let response = self
            .model_service
            .chat(&self.config.summary_model, vec![Message::user(prompt)])
            .await?;

        Ok(response.trim().to_string())
    }

    /// LLM 增量摘要。
    async fn incremental_summarize(
        &self,
        existing_summary: &str,
        new_messages: &[Message],
    ) -> Result<String> {
        if new_messages.is_empty() {
            return Ok(existing_summary.to_string());
        }

        let new_conversation = new_messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = INCREMENTAL_SUMMARY_PROMPT
            .replace("{existing_summary}", existing_summary)
            .replace("{new_conversation}", &new_conversation)
            .replace("{max_tokens}", &self.config.max_summary_tokens.to_string());

        let response = self
            .model_service
            .chat(&self.config.summary_model, vec![Message::user(prompt)])
            .await?;

        Ok(response.trim().to_string())
    }

    /// 通过选择压缩（保留重要消息）。
    async fn compress_by_selection(&self, messages: &[Message]) -> Result<(Vec<Message>, String)> {
        // 简单策略：保留用户消息和包含关键信息的消息
        let selected: Vec<Message> = messages
            .iter()
            .enumerate()
            .filter(|(i, msg)| {
                // 保留第一条和最后一条
                if *i == 0 || *i == messages.len() - 1 {
                    return true;
                }
                // 保留用户消息
                if msg.role == crate::common::types::MessageRole::User {
                    return true;
                }
                // 保留包含重要关键词的消息
                let important_keywords = [
                    "决定", "决策", "确认", "同意", "错误", "失败", "成功", "重要",
                ];
                important_keywords.iter().any(|kw| msg.content.contains(kw))
            })
            .map(|(_, msg)| msg.clone())
            .collect();

        let summary = format!(
            "从 {} 条消息中选择保留了 {} 条重要消息",
            messages.len(),
            selected.len()
        );
        Ok((selected, summary))
    }

    /// 混合压缩：先摘要，再保留关键消息。
    async fn compress_hybrid(&mut self, messages: &[Message]) -> Result<(Vec<Message>, String)> {
        if messages.len() > HYBRID_SPLIT_THRESHOLD {
            let summary_point = messages.len() - HYBRID_PRESERVE_RECENT;
            let to_summarize = &messages[..summary_point];
            let to_preserve = &messages[summary_point..];

            let summary = if let Some(ref cached) = self.cached_summary {
                self.incremental_summarize(cached, to_summarize)
                    .await?
            } else {
                self.summarize(to_summarize).await?
            };

            self.cached_summary = Some(summary.clone());
            let summary_message = Message::system(format!(
                "[对话摘要] 以下是对早期对话的摘要：\n{}\n[摘要结束]",
                summary
            ));

            let mut result = vec![summary_message];
            result.extend_from_slice(to_preserve);

            return Ok((result, summary));
        }

        // 消息不多时，使用选择策略
        self.compress_by_selection(messages).await
    }

    /// 获取压缩状态信息。
    pub fn get_status(&self, messages: &[Message]) -> CompressionStatus {
        let estimated_tokens = self.estimator.estimate_messages(messages);
        let threshold =
            (self.config.context_window as f32 * self.config.compression_threshold) as usize;
        let critical_threshold =
            (self.config.context_window as f32 * CRITICAL_THRESHOLD_RATIO) as usize;

        CompressionStatus {
            estimated_tokens,
            context_window: self.config.context_window,
            threshold,
            critical_threshold,
            should_compress: estimated_tokens > threshold,
            is_critical: estimated_tokens > critical_threshold,
            message_count: messages.len(),
        }
    }
}

/// 压缩状态信息。
#[derive(Debug, Clone)]
pub struct CompressionStatus {
    /// 估算的 token 数量。
    pub estimated_tokens: usize,
    /// 上下文窗口大小。
    pub context_window: usize,
    /// 压缩阈值。
    pub threshold: usize,
    /// 临界阈值（80%）。
    pub critical_threshold: usize,
    /// 是否需要压缩。
    pub should_compress: bool,
    /// 是否处于临界状态。
    pub is_critical: bool,
    /// 消息数量。
    pub message_count: usize,
}

impl std::fmt::Display for CompressionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let usage_pct = (self.estimated_tokens as f32 / self.context_window as f32 * 100.0) as u32;
        write!(
            f,
            "上下文使用: {}/{} tokens ({}%), 消息数: {}, 需要压缩: {}, 临界: {}",
            self.estimated_tokens,
            self.context_window,
            usage_pct,
            self.message_count,
            if self.should_compress { "是" } else { "否" },
            if self.is_critical { "是" } else { "否" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::common::error::Result;
    use crate::model::types::{ChatCompletionRequest, ChatCompletionResponse, ChatChoice, ChatCompletionChunk};
    use crate::model::ChatService;

    struct MockChatService {
        response: String,
    }

    impl MockChatService {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[async_trait]
    impl ChatService for MockChatService {
        async fn chat_completion(&self, _request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: Message::assistant(self.response.clone()),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Default::default(),
            })
        }

        async fn chat_completion_stream(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<Result<ChatCompletionChunk>>> {
            unimplemented!("stream not used in compression tests")
        }
    }

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert_eq!(config.context_window, 128000);
        assert_eq!(config.compression_threshold, 0.5);
        assert_eq!(config.preserve_recent_messages, 10);
    }

    #[test]
    fn test_compression_result_ratio() {
        let result = CompressionResult {
            messages: vec![],
            compressed_count: 5,
            summary: "test".to_string(),
            original_tokens: 1000,
            compressed_tokens: 500,
        };
        assert_eq!(result.compression_ratio(), 0.5);
    }

    #[test]
    fn test_compression_status_display() {
        let status = CompressionStatus {
            estimated_tokens: 50000,
            context_window: 128000,
            threshold: 64000,
            critical_threshold: 102400,
            should_compress: false,
            is_critical: false,
            message_count: 20,
        };
        let display = format!("{}", status);
        assert!(display.contains("50000"));
        assert!(display.contains("128000"));
    }

    fn make_compressor(strategy: CompressionStrategy) -> ContextCompressor {
        let config = CompressionConfig {
            strategy,
            preserve_recent_messages: 3,
            min_messages_to_compress: 3,
            ..CompressionConfig::default()
        };
        let mock: Arc<dyn ChatService> = Arc::new(MockChatService::new("ok"));
        ContextCompressor::new(mock, config)
    }

    fn make_compressor_with_threshold(
        strategy: CompressionStrategy,
        context_window: usize,
        compression_threshold: f32,
        min_msgs: usize,
    ) -> ContextCompressor {
        let config = CompressionConfig {
            strategy,
            preserve_recent_messages: 3,
            min_messages_to_compress: min_msgs,
            context_window,
            compression_threshold,
            ..CompressionConfig::default()
        };
        let mock: Arc<dyn ChatService> = Arc::new(MockChatService::new("ok"));
        ContextCompressor::new(mock, config)
    }

    fn msg_user(text: &str) -> Message {
        Message::user(text.to_string())
    }

    fn msg_assistant(text: &str) -> Message {
        Message::assistant(text.to_string())
    }

    // ── should_compress ─────────────────────────────────────────────

    #[test]
    fn test_should_compress_below_min_messages() {
        let compressor = make_compressor(CompressionStrategy::Select);
        let msgs = vec![msg_user("hi"), msg_assistant("hello")];
        assert!(!compressor.should_compress(&msgs));
    }

    #[test]
    fn test_should_compress_below_threshold() {
        let compressor = make_compressor(CompressionStrategy::Select);
        let msgs = vec![msg_user("a"), msg_assistant("b"), msg_user("c")];
        assert!(!compressor.should_compress(&msgs));
    }

    #[test]
    fn test_should_compress_above_threshold() {
        // Use very low threshold so even a few messages trigger compression
        let compressor = make_compressor_with_threshold(
            CompressionStrategy::Select,
            100,
            0.05,
            3,
        );
        let msgs = vec![
            msg_user("trigger compression"),
            msg_assistant("response text"),
            msg_user("more content"),
        ];
        assert!(compressor.should_compress(&msgs));
    }

    // ── compress_by_selection ───────────────────────────────────────

    #[tokio::test]
    async fn test_selection_keeps_first_and_last() {
        let compressor = make_compressor(CompressionStrategy::Select);
        let msgs = vec![
            msg_assistant("middle A"),
            msg_assistant("middle B"),
            msg_assistant("middle C"),
            msg_assistant("middle D"),
        ];
        let (selected, _summary) = compressor.compress_by_selection(&msgs).await.unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].content, "middle A");
        assert_eq!(selected[1].content, "middle D");
    }

    #[tokio::test]
    async fn test_selection_keeps_user_messages() {
        let compressor = make_compressor(CompressionStrategy::Select);
        let msgs = vec![
            msg_assistant("sys A"),
            msg_user("user question"),
            msg_assistant("sys B"),
        ];
        let (selected, _summary) = compressor.compress_by_selection(&msgs).await.unwrap();
        assert_eq!(selected.len(), 3);
    }

    #[tokio::test]
    async fn test_selection_keeps_keyword_messages() {
        let compressor = make_compressor(CompressionStrategy::Select);
        let msgs = vec![
            msg_user("start"),
            msg_assistant("中间无关内容"),
            msg_assistant("发现了一个重要错误需要修复"),
            msg_user("end"),
        ];
        let (selected, _summary) = compressor.compress_by_selection(&msgs).await.unwrap();
        assert!(selected.iter().any(|m| m.content.contains("错误")));
    }

    // ── compress (integration) ──────────────────────────────────────

    #[tokio::test]
    async fn test_compress_noop_when_below_preserve_recent() {
        let config = CompressionConfig {
            strategy: CompressionStrategy::Select,
            preserve_recent_messages: 5,
            min_messages_to_compress: 3,
            ..CompressionConfig::default()
        };
        let mock: Arc<dyn ChatService> = Arc::new(MockChatService::new("ok"));
        let mut compressor = ContextCompressor::new(mock, config);
        let msgs = vec![msg_user("a"), msg_assistant("b")];
        let result = compressor.compress(&msgs).await.unwrap();
        assert_eq!(result.compressed_count, 0);
        assert_eq!(result.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_compress_with_selection_strategy() {
        let config = CompressionConfig {
            strategy: CompressionStrategy::Select,
            preserve_recent_messages: 1,
            min_messages_to_compress: 2,
            ..CompressionConfig::default()
        };
        let mock: Arc<dyn ChatService> = Arc::new(MockChatService::new("ok"));
        let mut compressor = ContextCompressor::new(mock, config);
        let msgs = vec![
            msg_user("first"),
            msg_assistant("middle"),
            msg_user("recent"),
        ];
        let result = compressor.compress(&msgs).await.unwrap();
        assert_eq!(result.compressed_count, 2);
    }

    // ── get_status ──────────────────────────────────────────────────

    #[test]
    fn test_get_status_reflects_messages() {
        let compressor = make_compressor(CompressionStrategy::Select);
        let msgs = vec![msg_user("hello world"), msg_assistant("response text")];
        let status = compressor.get_status(&msgs);
        assert_eq!(status.message_count, 2);
        // 2 short messages < 64000 threshold — should not trigger compression
        assert!(!status.should_compress);
    }
}
