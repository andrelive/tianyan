//! Token 估算器。
//!
//! 提供快速的 token 数量估算，用于判断是否需要压缩上下文。
//! 使用基于字符数的启发式方法，比精确分词快得多。

use crate::common::types::Message;

/// 中文字符平均 token 比率。
const CN_CHARS_PER_TOKEN: f32 = 1.5;
/// 英文单词平均 token 比率。
const EN_WORDS_PER_TOKEN: f32 = 0.75;
/// 消息格式开销（role、分隔符等）。
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// Token 估算器。
///
/// 使用启发式方法快速估算文本的 token 数量。
#[derive(Debug, Clone)]
pub struct TokenEstimator {
    /// 中文字符比率。
    cn_ratio: f32,
    /// 英文单词比率。
    en_ratio: f32,
    /// 消息开销。
    message_overhead: usize,
}

impl TokenEstimator {
    /// 创建新的估算器。
    pub fn new() -> Self {
        Self {
            cn_ratio: CN_CHARS_PER_TOKEN,
            en_ratio: EN_WORDS_PER_TOKEN,
            message_overhead: MESSAGE_OVERHEAD_TOKENS,
        }
    }

    /// 估算单条消息的 token 数量。
    ///
    /// 覆盖消息发送给模型的实际内容：正文 + 推理内容（reasoning_content）+
    /// 工具调用参数（tool_calls arguments）。天演作为 agent 总是携带 tools，
    /// DeepSeek 思考模型要求携带 tools 的请求必须完整回传 reasoning_content
    /// （即使该轮未实际进行工具调用）——思考内容随请求回传，必须计入估算。
    pub fn estimate_message(&self, message: &Message) -> usize {
        let mut tokens = self.estimate_text(&message.content);
        if let Some(reasoning) = &message.reasoning_content {
            tokens += self.estimate_text(reasoning);
        }
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                tokens += self.estimate_text(&call.function.name);
                tokens += self.estimate_text(&call.function.arguments);
            }
        }
        tokens + self.message_overhead
    }

    /// 估算消息列表的 token 数量。
    pub fn estimate_messages(&self, messages: &[Message]) -> usize {
        messages.iter().map(|m| self.estimate_message(m)).sum()
    }

    /// 估算文本的 token 数量。
    ///
    /// 使用字符类型混合估算：
    /// - 中文字符：每 1.5 字符 ≈ 1 token
    /// - 英文单词：每 0.75 词 ≈ 1 token
    /// - 数字和标点：每 1 字符 ≈ 1 token
    pub fn estimate_text(&self, text: &str) -> usize {
        let mut cn_chars = 0;
        let mut en_chars = 0;
        let mut other_chars = 0;
        let mut in_word = false;

        for ch in text.chars() {
            if ch.is_ascii_alphabetic() {
                en_chars += 1;
                in_word = true;
            } else if ch.is_ascii_digit() || ch.is_ascii_punctuation() {
                other_chars += 1;
                in_word = false;
            } else if ch as u32 >= 0x4E00 && ch as u32 <= 0x9FFF {
                // CJK 统一表意文字
                cn_chars += 1;
                in_word = false;
            } else {
                // 其他字符（如 emoji、其他语言）
                other_chars += 1;
                in_word = false;
            }
        }

        // 估算英文单词数（粗略）
        let en_words = en_chars / 4 + if in_word { 1 } else { 0 };

        let cn_tokens = (cn_chars as f32 / self.cn_ratio) as usize;
        let en_tokens = (en_words as f32 / self.en_ratio) as usize;
        let other_tokens = other_chars;

        cn_tokens + en_tokens + other_tokens
    }

    /// 批量估算并返回详细统计。
    pub fn estimate_with_details(&self, messages: &[Message]) -> EstimationDetails {
        let mut total_content_tokens = 0;
        let mut total_overhead = 0;
        let mut message_breakdown = Vec::new();

        for (i, message) in messages.iter().enumerate() {
            let content_tokens = self.estimate_text(&message.content)
                + message
                    .reasoning_content
                    .as_deref()
                    .map(|r| self.estimate_text(r))
                    .unwrap_or(0)
                + message
                    .tool_calls
                    .as_ref()
                    .map(|calls| {
                        calls.iter().fold(0, |acc, call| {
                            acc + self.estimate_text(&call.function.name)
                                + self.estimate_text(&call.function.arguments)
                        })
                    })
                    .unwrap_or(0);
            let message_total = content_tokens + self.message_overhead;

            total_content_tokens += content_tokens;
            total_overhead += self.message_overhead;

            message_breakdown.push(MessageTokenEstimate {
                index: i,
                role: format!("{:?}", message.role),
                content_tokens,
                overhead: self.message_overhead,
                total: message_total,
            });
        }

        EstimationDetails {
            total_tokens: total_content_tokens + total_overhead,
            total_content_tokens,
            total_overhead,
            message_count: messages.len(),
            message_breakdown,
        }
    }
}

impl Default for TokenEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// 使用全局默认 TokenEstimator 快速估算文本的 Token 数量。
///
/// 这是 `TokenEstimator::new().estimate_text(text)` 的便捷函数。
/// 用于整个系统中所有需要快速 Token 估算的场景。
pub fn estimate_tokens(text: &str) -> usize {
    TokenEstimator::new().estimate_text(text)
}

/// 估算详细统计。
#[derive(Debug, Clone)]
pub struct EstimationDetails {
    /// 总 token 数。
    pub total_tokens: usize,
    /// 内容 token 数。
    pub total_content_tokens: usize,
    /// 开销 token 数。
    pub total_overhead: usize,
    /// 消息数量。
    pub message_count: usize,
    /// 每条消息的估算。
    pub message_breakdown: Vec<MessageTokenEstimate>,
}

/// 单条消息的 token 估算。
#[derive(Debug, Clone)]
pub struct MessageTokenEstimate {
    /// 消息索引。
    pub index: usize,
    /// 角色。
    pub role: String,
    /// 内容 token 数。
    pub content_tokens: usize,
    /// 开销 token 数。
    pub overhead: usize,
    /// 总计。
    pub total: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{FunctionCall, ToolCall, ToolCallType};

    #[test]
    fn test_estimate_text_chinese() {
        let estimator = TokenEstimator::new();
        // "你好世界" = 4 中文字符 ≈ 3 tokens
        let tokens = estimator.estimate_text("你好世界");
        assert!((2..=4).contains(&tokens));
    }

    #[test]
    fn test_estimate_text_english() {
        let estimator = TokenEstimator::new();
        // "Hello world" = 2 词 ≈ 2-3 tokens
        let tokens = estimator.estimate_text("Hello world");
        assert!((2..=5).contains(&tokens));
    }

    #[test]
    fn test_estimate_message() {
        let estimator = TokenEstimator::new();
        let msg = Message::user("Hello world");
        let tokens = estimator.estimate_message(&msg);
        assert!(tokens >= 6); // content + overhead
    }

    #[test]
    fn test_estimate_message_includes_tool_calls() {
        let estimator = TokenEstimator::new();
        let mut msg = Message::assistant("");
        msg.tool_calls = Some(vec![ToolCall {
            id: "call_1".to_string(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp/very/long/path/to/a/file"}"#.to_string(),
            },
        }]);
        let with_calls = estimator.estimate_message(&msg);
        msg.tool_calls = None;
        let without_calls = estimator.estimate_message(&msg);
        // 工具调用参数必须被算入（请求会发送）
        assert!(with_calls > without_calls);
        assert!(with_calls - without_calls >= 10);
    }

    #[test]
    fn test_estimate_message_includes_reasoning() {
        let estimator = TokenEstimator::new();
        let mut msg = Message::assistant("Hello");
        msg.reasoning_content = Some("思考过程内容".to_string());
        let with_reasoning = estimator.estimate_message(&msg);
        msg.reasoning_content = None;
        let without_reasoning = estimator.estimate_message(&msg);
        // 思考内容随请求回传（携带 tools 时必须完整回传），必须计入估算
        assert!(with_reasoning > without_reasoning);
        assert!(with_reasoning - without_reasoning >= 2);
    }

    #[test]
    fn test_estimate_messages() {
        let estimator = TokenEstimator::new();
        let messages = vec![Message::user("Hello"), Message::assistant("Hi there!")];
        let tokens = estimator.estimate_messages(&messages);
        assert!(tokens >= 8); // 2 messages * overhead + content
    }

    #[test]
    fn test_estimate_with_details() {
        let estimator = TokenEstimator::new();
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello"),
        ];
        let details = estimator.estimate_with_details(&messages);
        assert_eq!(details.message_count, 2);
        assert!(details.total_tokens > 0);
        assert_eq!(details.message_breakdown.len(), 2);
    }
}
