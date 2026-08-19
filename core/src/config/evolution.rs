//! 自演化模块配置（ADR-017：每日演化智能体任务）。

use serde::{Deserialize, Serialize};

/// 演化综述机制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvolutionMechanism {
    /// 委托到 evolution_reviewer 角色（默认）。
    #[default]
    Delegate,
    /// 唤醒主 agent（ADR-013 process_wake）。
    Wake,
}

fn default_true() -> bool {
    true
}

fn default_cron() -> String {
    "0 0 */24 * * *".to_string()
}

fn default_review_role() -> String {
    "evolution_reviewer".to_string()
}

fn default_max_items() -> usize {
    200
}

fn default_idle_episode_hours() -> u64 {
    24
}

/// 自演化任务配置（ADR-017：每日演化智能体任务 + 片段模型）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionConfig {
    /// 是否启用演化任务（无启用模型 Provider 时整体不装配，与此开关无关）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Cron 表达式（自研 cron 仅支持 */N 间隔；默认每天一次）。
    #[serde(default = "default_cron")]
    pub cron: String,
    /// 演化综述角色 ID（mechanism = delegate 时使用）。
    #[serde(default = "default_review_role")]
    pub review_role: String,
    /// 综述机制（delegate | wake，默认 delegate）。
    #[serde(default)]
    pub mechanism: EvolutionMechanism,
    /// 单次运行最大处理条目数（防 token 爆炸）。
    #[serde(default = "default_max_items")]
    pub max_items_per_run: usize,
    /// 片段划分空闲阈值（小时）：同一会话内相邻消息间隔超过该值切分新片段。
    #[serde(default = "default_idle_episode_hours")]
    pub idle_episode_hours: u64,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            cron: default_cron(),
            review_role: default_review_role(),
            mechanism: EvolutionMechanism::default(),
            max_items_per_run: default_max_items(),
            idle_episode_hours: default_idle_episode_hours(),
        }
    }
}

impl EvolutionConfig {
    /// 校验配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_items_per_run == 0 {
            return Err("max_items_per_run 必须大于 0".to_string());
        }
        if self.idle_episode_hours == 0 {
            return Err("idle_episode_hours 必须大于 0".to_string());
        }
        if self.review_role.trim().is_empty() {
            return Err("review_role 不能为空".to_string());
        }
        let parts: Vec<&str> = self.cron.split_whitespace().collect();
        if parts.len() != 6 {
            return Err(format!(
                "cron 表达式需为 6 字段格式（秒 分 时 日 月 周）：{}",
                self.cron
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evolution_config_defaults() {
        let cfg = EvolutionConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.cron, "0 0 */24 * * *");
        assert_eq!(cfg.review_role, "evolution_reviewer");
        assert_eq!(cfg.mechanism, EvolutionMechanism::Delegate);
        assert_eq!(cfg.max_items_per_run, 200);
        assert_eq!(cfg.idle_episode_hours, 24);
    }

    #[test]
    fn test_evolution_config_validate() {
        assert!(EvolutionConfig::default().validate().is_ok());
        let mut cfg = EvolutionConfig::default();
        cfg.max_items_per_run = 0;
        assert!(cfg.validate().is_err());
        let mut cfg = EvolutionConfig::default();
        cfg.idle_episode_hours = 0;
        assert!(cfg.validate().is_err());
        let mut cfg = EvolutionConfig::default();
        cfg.cron = "bad".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_evolution_mechanism_serde() {
        let json = serde_json::to_string(&EvolutionMechanism::Wake).unwrap();
        assert_eq!(json, "\"wake\"");
        let parsed: EvolutionMechanism = serde_json::from_str("\"delegate\"").unwrap();
        assert_eq!(parsed, EvolutionMechanism::Delegate);
    }
}
