//! 记忆配置模块。

use serde::{Deserialize, Serialize};

/// 记忆配置（记忆写入通道 = 演化任务，ADR-017；仅保留"巩固"开关）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 启用自动记忆巩固（演化综述的查重 / 合并归纳职责；ADR-034 兑现）。
    #[serde(default = "default_true")]
    pub auto_consolidation: bool,
}

fn default_true() -> bool {
    true
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            auto_consolidation: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_memory_config() {
        let config = MemoryConfig::default();
        assert!(config.auto_consolidation, "默认开启自动巩固");
    }
}
