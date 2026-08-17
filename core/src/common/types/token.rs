use serde::{Deserialize, Serialize};

/// Token 使用统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// 提示词中的 token 数量
    #[serde(default)]
    pub prompt_tokens: usize,
    /// 补全中的 token 数量（嵌入模型等非流式 API 可能不返回此字段）
    #[serde(default)]
    pub completion_tokens: usize,
    /// 总 token 数
    #[serde(default)]
    pub total_tokens: usize,
    /// 缓存命中（读取）token 数。提供商返回缓存明细时填充
    /// （DeepSeek `prompt_cache_hit_tokens`、OpenAI `prompt_tokens_details.cached_tokens` 等），
    /// 否则为 0。
    #[serde(default)]
    pub cache_read: usize,
    /// 缓存写入 token 数。提供商返回时填充，否则为 0。
    #[serde(default)]
    pub cache_write: usize,
}

impl TokenUsage {
    /// 创建新的 token 使用统计。
    pub fn new(prompt_tokens: usize, completion_tokens: usize) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            cache_read: 0,
            cache_write: 0,
        }
    }

    /// 累加另一份使用统计（多轮对话的轮次汇总）。
    pub fn accumulate(&mut self, other: &TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_usage_new() {
        let usage = TokenUsage::new(100, 50);
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn test_token_usage_accumulate() {
        let mut total = TokenUsage::new(10, 5);
        total.cache_read = 3;
        total.cache_write = 2;
        let mut other = TokenUsage::new(100, 50);
        other.cache_read = 30;
        other.cache_write = 20;
        total.accumulate(&other);
        assert_eq!(total.prompt_tokens, 110);
        assert_eq!(total.completion_tokens, 55);
        assert_eq!(total.total_tokens, 165);
        assert_eq!(total.cache_read, 33);
        assert_eq!(total.cache_write, 22);
    }

    #[test]
    fn test_token_usage_cache_defaults_zero() {
        let usage = TokenUsage::new(100, 50);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
        // 旧 JSON（无缓存字段）仍可反序列化
        let old_json = r#"{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}"#;
        let old: TokenUsage = serde_json::from_str(old_json).unwrap();
        assert_eq!(old.cache_read, 0);
        assert_eq!(old.cache_write, 0);
    }
}
