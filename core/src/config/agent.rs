//! 智能体配置管理。

use serde::{Deserialize, Serialize};

/// 智能体配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// 默认检索结果数量。
    #[serde(default = "default_default_top_k")]
    pub default_top_k: usize,
    /// 上下文管线注入的 learned rules 默认 Top-K。
    #[serde(default = "default_learned_rules_top_k")]
    pub learned_rules_top_k: usize,
    /// Agent Loop 最大轮次。
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    /// 后台任务结果自审（G4，默认关闭 opt-in）：开启后后台任务完成
    /// 通知注入前经 LLM 自审，未通过则结果带 `[自审未通过]` 标记，
    /// 由主 agent 复核（每个后台任务多一次小调用）。
    #[serde(default = "default_false")]
    pub background_self_review: bool,
    /// 工作目录：Agent 执行命令/读写文件的基础目录，也是会话回退时
    /// 文件快照的根目录。缺省使用进程当前目录。
    #[serde(default)]
    pub working_directory: Option<String>,
    /// 委托任务（delegate_to_agent）同时运行上限（ADR-026）。
    #[serde(default = "default_max_background_concurrency")]
    pub max_background_concurrency: usize,
    /// 委托任务排队上限（超出拒绝；ADR-026 双信号量排队模型）。
    #[serde(default = "default_max_background_queue")]
    pub max_background_queue: usize,
    /// 终端命令（execute_command background）同时运行上限（ADR-026）。
    #[serde(default = "default_max_command_concurrency")]
    pub max_command_concurrency: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_top_k: default_default_top_k(),
            learned_rules_top_k: default_learned_rules_top_k(),
            max_turns: default_max_turns(),
            background_self_review: false,
            working_directory: None,
            max_background_concurrency: default_max_background_concurrency(),
            max_background_queue: default_max_background_queue(),
            max_command_concurrency: default_max_command_concurrency(),
        }
    }
}

impl AgentConfig {
    /// 创建新的智能体配置。
    pub fn new() -> Self {
        Self::default()
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
        if self.max_background_concurrency == 0 {
            return Err("max_background_concurrency 必须大于 0".to_string());
        }
        if self.max_background_queue < self.max_background_concurrency {
            return Err(format!(
                "max_background_queue（{}）不能小于 max_background_concurrency（{}）",
                self.max_background_queue, self.max_background_concurrency
            ));
        }
        if self.max_command_concurrency == 0 {
            return Err("max_command_concurrency 必须大于 0".to_string());
        }
        Ok(())
    }
}

fn default_default_top_k() -> usize {
    5
}

fn default_learned_rules_top_k() -> usize {
    5
}

fn default_max_turns() -> usize {
    200
}

fn default_false() -> bool {
    false
}

fn default_max_background_concurrency() -> usize {
    20
}

fn default_max_background_queue() -> usize {
    40
}

fn default_max_command_concurrency() -> usize {
    16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.default_top_k, 5);
        assert_eq!(config.learned_rules_top_k, 5);
        assert!(!config.background_self_review);
    }

    #[test]
    fn test_agent_config_builder() {
        let config = AgentConfig::new();

        assert_eq!(config.max_turns, 200);
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
        assert_eq!(config.max_turns, parsed.max_turns);
        assert_eq!(config.working_directory, parsed.working_directory);
    }
}
