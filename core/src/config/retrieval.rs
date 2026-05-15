//! 检索配置模块。

use serde::{Deserialize, Serialize};

/// 检索配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// 默认返回结果数量。
    #[serde(default = "default_top_k")]
    pub default_top_k: usize,
    /// 最小相关性评分阈值。
    #[serde(default = "default_min_score")]
    pub min_score: f32,
    /// 启用 L0/L1 两阶段检索。
    #[serde(default = "default_true")]
    pub two_stage_retrieval: bool,
    /// L0 结果倍数（用于重排序）。
    #[serde(default = "default_l0_multiplier")]
    pub l0_multiplier: usize,
    /// 上下文最大 token 数。
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    /// 启用检索缓存。
    #[serde(default = "default_true")]
    pub enable_cache: bool,
    /// 缓存 TTL（秒）。
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: u64,
}

fn default_top_k() -> usize {
    10
}

fn default_min_score() -> f32 {
    0.5
}

fn default_true() -> bool {
    true
}

fn default_l0_multiplier() -> usize {
    3
}

fn default_max_context_tokens() -> usize {
    4000
}

fn default_cache_ttl() -> u64 {
    300 // 5 分钟
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            default_top_k: default_top_k(),
            min_score: default_min_score(),
            two_stage_retrieval: true,
            l0_multiplier: default_l0_multiplier(),
            max_context_tokens: default_max_context_tokens(),
            enable_cache: true,
            cache_ttl: default_cache_ttl(),
        }
    }
}

impl RetrievalConfig {
    /// 验证检索配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.default_top_k == 0 {
            return Err("default_top_k 必须大于 0".to_string());
        }

        if !(0.0..=1.0).contains(&self.min_score) {
            return Err("min_score 必须在 0.0 到 1.0 之间".to_string());
        }

        if self.l0_multiplier == 0 {
            return Err("l0_multiplier 必须大于 0".to_string());
        }

        if self.max_context_tokens == 0 {
            return Err("max_context_tokens 必须大于 0".to_string());
        }

        if self.cache_ttl == 0 {
            return Err("cache_ttl 必须大于 0".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_retrieval_config() {
        let config = RetrievalConfig::default();
        assert_eq!(config.default_top_k, 10);
        assert_eq!(config.min_score, 0.5);
        assert!(config.two_stage_retrieval);
        assert!(config.enable_cache);
    }

    #[test]
    fn test_retrieval_validation() {
        let mut config = RetrievalConfig::default();
        assert!(config.validate().is_ok());

        config.min_score = 1.5;
        assert!(config.validate().is_err());

        config.min_score = 0.5;
        config.default_top_k = 0;
        assert!(config.validate().is_err());
    }
}
