//! 记忆配置模块。

use serde::{Deserialize, Serialize};

/// 记忆配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 最大会话记忆大小（token 数）。
    #[serde(default = "default_session_memory")]
    pub max_session_memory: usize,
    /// 最大长期记忆条目数。
    #[serde(default = "default_long_term_memory")]
    pub max_long_term_memory: usize,
    /// 记忆重要性阈值（0.0 - 1.0）。
    #[serde(default = "default_importance_threshold")]
    pub importance_threshold: f32,
    /// 启用自动记忆整合。
    #[serde(default = "default_true")]
    pub auto_consolidation: bool,
    /// 整合间隔（秒）。
    #[serde(default = "default_consolidation_interval")]
    pub consolidation_interval: u64,
    /// 记忆每日衰减率（0.0 - 1.0）。
    #[serde(default = "default_decay_rate")]
    pub decay_rate: f32,
    /// 偏好类记忆写前校验（G3，默认关闭 opt-in）。
    ///
    /// 开启后 MemoryTask 对 `preferences` 类别记忆先经 LLM 校验
    /// （是否稳定长期偏好）再持久化；未通过/校验失败的记忆被丢弃。
    /// 每提取周期仅对偏好类记忆多一次小调用，成本可控。
    #[serde(default = "default_false")]
    pub verify_preferences: bool,
}

fn default_session_memory() -> usize {
    8000
}

fn default_long_term_memory() -> usize {
    10000
}

fn default_importance_threshold() -> f32 {
    0.5
}

fn default_true() -> bool {
    true
}

fn default_consolidation_interval() -> u64 {
    3600
}

fn default_decay_rate() -> f32 {
    0.01
}

fn default_false() -> bool {
    false
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_session_memory: default_session_memory(),
            max_long_term_memory: default_long_term_memory(),
            importance_threshold: default_importance_threshold(),
            auto_consolidation: true,
            consolidation_interval: default_consolidation_interval(),
            decay_rate: default_decay_rate(),
            verify_preferences: false,
        }
    }
}

impl MemoryConfig {
    /// 验证记忆配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_session_memory == 0 {
            return Err("max_session_memory 必须大于 0".to_string());
        }

        if self.max_long_term_memory == 0 {
            return Err("max_long_term_memory 必须大于 0".to_string());
        }

        if !(0.0..=1.0).contains(&self.importance_threshold) {
            return Err("importance_threshold 必须在 0.0 和 1.0 之间".to_string());
        }

        if self.consolidation_interval == 0 {
            return Err("consolidation_interval 必须大于 0".to_string());
        }

        if !(0.0..=1.0).contains(&self.decay_rate) {
            return Err("decay_rate 必须在 0.0 和 1.0 之间".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_memory_config() {
        let config = MemoryConfig::default();
        assert_eq!(config.max_session_memory, 8000);
        assert_eq!(config.max_long_term_memory, 10000);
        assert_eq!(config.importance_threshold, 0.5);
        assert!(config.auto_consolidation);
        assert_eq!(config.consolidation_interval, 3600);
        assert_eq!(config.decay_rate, 0.01);
        assert!(!config.verify_preferences, "偏好校验默认关闭 opt-in");
    }

    #[test]
    fn test_memory_validation() {
        let mut config = MemoryConfig::default();
        assert!(config.validate().is_ok());

        config.importance_threshold = 1.5;
        assert!(config.validate().is_err());

        config.importance_threshold = 0.5;
        config.max_session_memory = 0;
        assert!(config.validate().is_err());
    }
}
