//! 智能体配置管理。
//!
//! 本模块提供智能体的配置结构，包括技能、记忆等配置项。

use serde::{Deserialize, Serialize};

/// 智能体配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// 是否启用技能执行。
    #[serde(default = "default_enable_skills")]
    pub enable_skills: bool,
    /// 是否启用记忆持久化。
    #[serde(default = "default_enable_memory")]
    pub enable_memory: bool,
    /// 是否流式输出响应。
    #[serde(default = "default_stream_responses")]
    pub stream_responses: bool,
    /// 是否启用模型的思考模式（Qwen3.5+ 支持）。
    /// 启用后模型会进行更深入的推理，但首字符响应会变慢。
    /// 默认禁用以获得更快的响应速度。
    #[serde(default = "default_enable_thinking")]
    pub enable_thinking: bool,
    /// 默认检索结果数量。
    #[serde(default = "default_default_top_k")]
    pub default_top_k: usize,
    /// 是否启用验证门控（Agent 产出后自动 cargo check）。
    #[serde(default = "default_enable_verification")]
    pub enable_verification: bool,
    /// 上下文管线注入的 learned rules 默认 Top-K。
    #[serde(default = "default_learned_rules_top_k")]
    pub learned_rules_top_k: usize,
    /// system prompt 中 learned rules 段的最大 token 估算上限。
    #[serde(default = "default_learned_rules_max_tokens")]
    pub learned_rules_max_tokens: usize,
    /// Agent Loop 最大轮次。
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enable_skills: default_enable_skills(),
            enable_memory: default_enable_memory(),
            stream_responses: default_stream_responses(),
            enable_thinking: default_enable_thinking(),
            default_top_k: default_default_top_k(),
            enable_verification: default_enable_verification(),
            learned_rules_top_k: default_learned_rules_top_k(),
            learned_rules_max_tokens: default_learned_rules_max_tokens(),
            max_turns: default_max_turns(),
        }
    }
}

impl AgentConfig {
    /// 创建新的智能体配置。
    pub fn new() -> Self {
        Self::default()
    }

    /// 启用或禁用技能。
    pub fn with_skills(mut self, enable: bool) -> Self {
        self.enable_skills = enable;
        self
    }

    /// 启用或禁用记忆。
    pub fn with_memory(mut self, enable: bool) -> Self {
        self.enable_memory = enable;
        self
    }

    /// 验证配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.default_top_k == 0 {
            return Err("default_top_k 必须大于 0".to_string());
        }
        if self.default_top_k > 100 {
            return Err(format!(
                "default_top_k 不能超过 100，当前值: {}",
                self.default_top_k
            ));
        }
        Ok(())
    }
}

fn default_enable_skills() -> bool {
    true
}

fn default_enable_memory() -> bool {
    true
}

fn default_stream_responses() -> bool {
    true
}

fn default_enable_thinking() -> bool {
    false
}

fn default_default_top_k() -> usize {
    5
}

fn default_enable_verification() -> bool {
    false
}

fn default_learned_rules_top_k() -> usize {
    5
}

fn default_learned_rules_max_tokens() -> usize {
    800
}

fn default_max_turns() -> usize {
    20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert!(config.enable_skills);
        assert!(config.enable_memory);
        assert!(config.stream_responses);
        assert!(!config.enable_thinking);
    }

    #[test]
    fn test_agent_config_builder() {
        let config = AgentConfig::new().with_skills(false).with_memory(false);

        assert!(!config.enable_skills);
        assert!(!config.enable_memory);
    }

    #[test]
    fn test_agent_config_validation() {
        let valid_config = AgentConfig::default();
        assert!(valid_config.validate().is_ok());
    }

    #[test]
    fn test_agent_config_serialization() {
        let config = AgentConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AgentConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.enable_skills, parsed.enable_skills);
        assert_eq!(config.enable_memory, parsed.enable_memory);
    }
}
