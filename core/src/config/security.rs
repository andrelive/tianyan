//! 安全配置模块。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 安全配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// 启用安全功能。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 文件操作允许的目录。
    #[serde(default)]
    pub allowed_directories: Vec<PathBuf>,
    /// 文件操作禁止的目录。
    #[serde(default)]
    pub blocked_directories: Vec<PathBuf>,
    /// 允许执行的命令。
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    /// 禁止执行的命令。
    #[serde(default)]
    pub blocked_commands: Vec<String>,
    /// 启用命令确认。
    #[serde(default = "default_true")]
    pub confirm_commands: bool,
    /// 文件操作最大文件大小（字节）。
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    /// 启用审计日志。
    #[serde(default = "default_true")]
    pub audit_logging: bool,
}

fn default_true() -> bool {
    true
}

fn default_max_file_size() -> u64 {
    100 * 1024 * 1024 // 100 MB
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_directories: Vec::new(),
            blocked_directories: Vec::new(),
            allowed_commands: Vec::new(),
            blocked_commands: Vec::new(),
            confirm_commands: true,
            max_file_size: default_max_file_size(),
            audit_logging: true,
        }
    }
}

impl SecurityConfig {
    /// 验证安全配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_file_size == 0 {
            return Err("max_file_size 必须大于 0".to_string());
        }

        // 检查允许目录和禁止目录是否有冲。
        for allowed in &self.allowed_directories {
            for blocked in &self.blocked_directories {
                if allowed.starts_with(blocked) || blocked.starts_with(allowed) {
                    return Err(format!(
                        "允许目录 {:?} 和禁止目。
{:?} 存在冲突",
                        allowed, blocked
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_security_config() {
        let config = SecurityConfig::default();
        assert!(config.enabled);
        assert!(config.confirm_commands);
        assert!(config.audit_logging);
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
    }

    #[test]
    fn test_security_validation() {
        let config = SecurityConfig::default();
        assert!(config.validate().is_ok());

        let mut invalid_config = config.clone();
        invalid_config.max_file_size = 0;
        assert!(invalid_config.validate().is_err());
    }
}
