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
use crate::common::types::TokenUsage;
use crate::model::{ChatCompletionRequest, ChatService};

pub use crate::common::token_estimator::{
    estimate_tokens, EstimationDetails, MessageTokenEstimate, TokenEstimator,
};

/// 压缩策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionStrategy {
    /// 摘要：将早期对话压缩为摘要。
    Summarize,
    /// 选择：保留重要消息，丢弃次要消息。
    Select,
    /// 混合：先摘要再选择。
    #[default]
    Hybrid,
}

/// 默认上下文窗口大小。
const DEFAULT_CONTEXT_WINDOW: usize = 128000;
/// 默认压缩阈值（窗口的百分比）。
const DEFAULT_COMPRESSION_THRESHOLD: f32 = 0.5;
/// 默认保留最近消息数。
const DEFAULT_PRESERVE_RECENT: usize = 10;
/// 默认最小压缩消息数。
const DEFAULT_MIN_MESSAGES_TO_COMPRESS: usize = 6;
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
            summary_model: String::new(),
            min_messages_to_compress: DEFAULT_MIN_MESSAGES_TO_COMPRESS,
            max_summary_tokens: 4000,
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
    /// 生成摘要的 LLM 请求真实 token 用量（输入/输出/缓存命中）。
    /// 压缩请求不走 AgentLoop，其消耗须随摘要消息持久化，否则会话统计丢失。
    /// `Select` 策略不调用 LLM——恒为默认值。
    pub summary_usage: TokenUsage,
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
///
/// 要求输出固定四字段结构（意图/已执行步骤/当前状态/下一步），
/// 保证 resume 时 LLM 能恢复"任务指针"而不只是事实摘要。
const DEFAULT_SUMMARY_PROMPT: &str = r##"请将以下对话历史压缩为分级摘要。要求：

1. **用户意图与进度是最高优先级**：完整保留用户的目标、追加/修正的需求、当前进度、已完成与未完成事项
2. **时间分级**：近期（最后几轮）的关键事件详细记录（工具结果要点、重要输出、决策）；越早期越概略，只保留结论性信息
3. 保留重要的事实、决策、错误与解决方案
4. 删除重复信息、思考过程（reasoning）与闲聊
5. 使用第三人称客观描述

必须严格按以下四个字段输出（每字段以 ## 开头的标题行分隔，字段内容为简洁要点列表）：

## 用户意图
用户最初想达成的目标，以及后续追加或修正的需求（原话要点）

## 已执行步骤
已经完成的任务、采取的行动和取得的结果（近期详细，早期概略）

## 当前状态
当前进度：已完成部分、进行中的部分、尚未解决的问题

## 下一步
尚未完成的事项、下一步应执行的动作（若对话尚未结束，必须给出）

对话历史：
{conversation}

请输出结构化摘要（不超过 {max_tokens} 字）："##;
const INCREMENTAL_SUMMARY_PROMPT: &str = r##"请将以下新对话内容合并到已有摘要中，生成更新后的摘要。

已有摘要：
{existing_summary}

新对话内容：
{new_conversation}

要求：
1. 保留已有摘要中的所有重要信息
2. 将新对话中的关键信息合并进去
3. 删除重复内容
4. 保持简洁，不超过 {max_tokens} 字

必须保持 ## 用户意图 / ## 已执行步骤 / ## 当前状态 / ## 下一步 四个字段结构（每字段以 ## 标题行分隔），新对话中的最新意图、进度与下一步要合并进对应字段。

请输出更新后的摘要："##;

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
    ///
    /// 基于**真实 usage**（provider 返回的最近一次请求 prompt 侧 token 数），
    /// 而非字符估算：`prompt_tokens` 是本次请求的全部输入（含全部历史），
    /// 即为当前上下文真实占用。缓存命中部分（cache.read）也算占用（它仍
    /// 占据窗口）。
    ///
    pub fn should_compress(&self, recent_input_tokens: usize) -> bool {
        let threshold =
            (self.config.context_window as f32 * self.config.compression_threshold) as usize;
        recent_input_tokens > threshold
    }

    /// 压缩消息列表。
    ///
    /// - `messages` - 原始消息列表
    pub async fn compress(&mut self, messages: &[Message]) -> Result<CompressionResult> {
        if messages.is_empty() {
            let zero = self.estimator.estimate_messages(messages);
            return Ok(CompressionResult {
                messages: vec![],
                compressed_count: 0,
                summary: String::new(),
                summary_usage: TokenUsage::default(),
                original_tokens: zero,
                compressed_tokens: zero,
            });
        }

        let original_tokens = self.estimator.estimate_messages(messages);

        let (compressed_messages, summary, summary_usage) = match self.config.strategy {
            CompressionStrategy::Summarize => {
                let (msgs, s, u) = self.compress_by_summarization(messages).await?;
                (msgs, s, u)
            }
            CompressionStrategy::Select => {
                let (msgs, s) = self.compress_by_selection(messages).await?;
                (msgs, s, TokenUsage::default())
            }
            CompressionStrategy::Hybrid => {
                let (msgs, s, u) = self.compress_hybrid(messages).await?;
                (msgs, s, u)
            }
        };
        let compressed_tokens = self.estimator.estimate_messages(&compressed_messages);
        Ok(CompressionResult {
            messages: compressed_messages,
            summary_usage,
            compressed_count: messages.len(),
            summary,
            original_tokens,
            compressed_tokens,
        })
    }

    /// 通过摘要压缩。
    async fn compress_by_summarization(
        &mut self,
        messages: &[Message],
    ) -> Result<(Vec<Message>, String, TokenUsage)> {
        // 如果有缓存摘要，使用增量摘要；否则全量摘要
        let (summary, usage) = if let Some(ref cached) = self.cached_summary {
            self.incremental_summarize(cached, messages).await?
        } else {
            self.summarize(messages).await?
        };

        self.cached_summary = Some(summary.clone());

        let summary_message = Message::system(format!(
            "[对话摘要] 以下是对早期对话的摘要：\n{}\n[摘要结束]",
            summary
        ));

        Ok((vec![summary_message], summary, usage))
    }

    async fn summarize(&self, messages: &[Message]) -> Result<(String, TokenUsage)> {
        if messages.is_empty() {
            return Ok((String::new(), TokenUsage::default()));
        }

        let conversation = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = DEFAULT_SUMMARY_PROMPT.replace("{conversation}", &conversation);

        let response = self
            .model_service
            .chat_completion(ChatCompletionRequest::new(
                &self.config.summary_model,
                vec![Message::user(prompt)],
            ))
            .await?;
        // 取第一个候选的文本（`chat()` 便捷方法丢弃 usage——此处需要真实
        // token 统计供压缩消耗入账，故走完整响应）。
        let text = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        Ok((text.trim().to_string(), response.usage))
    }

    /// LLM 增量摘要。
    async fn incremental_summarize(
        &self,
        existing_summary: &str,
        new_messages: &[Message],
    ) -> Result<(String, TokenUsage)> {
        if new_messages.is_empty() {
            return Ok((existing_summary.to_string(), TokenUsage::default()));
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
            .chat_completion(ChatCompletionRequest::new(
                &self.config.summary_model,
                vec![Message::user(prompt)],
            ))
            .await?;
        let text = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        Ok((text.trim().to_string(), response.usage))
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
    async fn compress_hybrid(
        &mut self,
        messages: &[Message],
    ) -> Result<(Vec<Message>, String, TokenUsage)> {
        // 全量摘要：所有消息进一条摘要，不保留原样。
        // 前缀缓存考量：保留原样会让请求前缀随轮次增长且压缩后缓存失效；
        // 全量摘要 + 稳定前缀（soul/rules/摘要）跨请求不变，命中率最高。
        // 用户意图与进度由摘要提示词专门承载（见 DEFAULT_SUMMARY_PROMPT）。
        let (summary, usage) = if let Some(ref cached) = self.cached_summary {
            self.incremental_summarize(cached, messages).await?
        } else {
            self.summarize(messages).await?
        };

        self.cached_summary = Some(summary.clone());
        let summary_message = Message::system(format!(
            "[对话摘要] 以下是对早期对话的摘要：
{}
[摘要结束]",
            summary
        ));

        Ok((vec![summary_message], summary, usage))
    }

    /// 获取压缩状态信息。
    pub fn get_status(&self, recent_input_tokens: usize) -> CompressionStatus {
        let threshold =
            (self.config.context_window as f32 * self.config.compression_threshold) as usize;
        let critical_threshold =
            (self.config.context_window as f32 * CRITICAL_THRESHOLD_RATIO) as usize;

        CompressionStatus {
            estimated_tokens: recent_input_tokens,
            context_window: self.config.context_window,
            threshold,
            critical_threshold,
            should_compress: recent_input_tokens > threshold,
            is_critical: recent_input_tokens > critical_threshold,
            message_count: 0,
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
    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::ChatService;
    use crate::test_utils::MockChatService;

    /// 创建返回指定响应文本的 mock ChatService。
    fn mock_chat(response: &str) -> Arc<dyn ChatService> {
        let response = response.to_string();
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(move |_| {
            Ok(ChatCompletionResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: Message::assistant(response.clone()),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Default::default(),
            })
        });
        Arc::new(mock)
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
            summary_usage: TokenUsage::default(),
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
        let mock = mock_chat("ok");
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
        let mock = mock_chat("ok");
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
        assert!(!compressor.should_compress(0));
    }

    #[test]
    fn test_should_compress_below_threshold() {
        let compressor = make_compressor(CompressionStrategy::Select);
        assert!(!compressor.should_compress(0));
    }

    #[test]
    fn test_should_compress_above_threshold() {
        // Use very low threshold so even a few messages trigger compression
        let compressor = make_compressor_with_threshold(CompressionStrategy::Select, 100, 0.05, 3);
        assert!(compressor.should_compress(10000));
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
    async fn test_compress_with_few_messages() {
        let config = CompressionConfig {
            strategy: CompressionStrategy::Select,
            preserve_recent_messages: 5,
            min_messages_to_compress: 3,
            ..CompressionConfig::default()
        };
        let mock = mock_chat("ok");
        let mut compressor = ContextCompressor::new(mock, config);
        let msgs = vec![msg_user("a"), msg_assistant("b")];
        let result = compressor.compress(&msgs).await.unwrap();
        assert_eq!(result.compressed_count, 2);
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
        let mock = mock_chat("ok");
        let mut compressor = ContextCompressor::new(mock, config);
        let msgs = vec![
            msg_user("first"),
            msg_assistant("middle"),
            msg_user("recent"),
        ];
        let result = compressor.compress(&msgs).await.unwrap();
        assert_eq!(result.compressed_count, 3);
        assert_eq!(result.messages.len(), 2);
    }

    // ── get_status ──────────────────────────────────────────────────

    #[test]
    fn test_get_status_reflects_real_usage() {
        // 窗口 100 × 阈值 0.5 → 触发线 50；临界 100 × 0.8 = 80
        let compressor = make_compressor_with_threshold(CompressionStrategy::Select, 100, 0.5, 3);
        let status = compressor.get_status(30);
        assert_eq!(status.estimated_tokens, 30);
        assert!(!status.should_compress);
        let status = compressor.get_status(60);
        assert!(status.should_compress);
        let status = compressor.get_status(90);
        assert!(status.is_critical);
    }
}
