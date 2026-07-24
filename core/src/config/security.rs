//! 安全配置模块。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 安全模式 — 决定如何处理危险操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyMode {
    /// 严格模式（默认）：对危险操作硬阻断，必须通过审批。
    #[serde(alias = "deny")]
    Strict,
    /// 转换模式：将危险命令自动重写为安全等价操作
    /// （例如 rm → mv 到回收站目录，del → move 到回收站）。
    Transform,
    /// 宽松模式：允许所有操作（用户自行承担风险）。
    Permissive,
}

impl Default for SafetyMode {
    fn default() -> Self {
        Self::Strict
    }
}

/// 安全配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// 启用安全功能。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 安全模式。
    #[serde(default)]
    pub safety_mode: SafetyMode,
    /// 回收站目录（SafetyMode::Transform 时使用）。
    /// 默认为 `~/.tianyan/trash`。
    #[serde(default = "default_trash_dir")]
    pub trash_directory: PathBuf,
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
    /// 文件读取技能最大大小（字节），默认 50MB。
    #[serde(default = "default_skill_read_max_size")]
    pub skill_file_read_max_size: u64,
    /// 文件读取技能超时（秒），默认 30。
    #[serde(default = "default_skill_read_timeout")]
    pub skill_file_read_timeout_secs: u64,
    /// 文件写入技能最大大小（字节），默认 10MB。
    #[serde(default = "default_skill_write_max_size")]
    pub skill_file_write_max_size: u64,
    /// 文件写入技能超时（秒），默认 30。
    #[serde(default = "default_skill_write_timeout")]
    pub skill_file_write_timeout_secs: u64,
    /// 文件列表技能最大条目数，默认 10000。
    #[serde(default = "default_skill_list_max")]
    pub skill_file_list_max_entries: usize,
    /// HTTP 请求技能超时（秒），默认 60。
    #[serde(default = "default_skill_http_timeout")]
    pub skill_http_timeout_secs: u64,
    /// 系统命令技能超时（秒），默认 300。
    #[serde(default = "default_skill_command_timeout")]
    pub skill_command_timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_max_file_size() -> u64 {
    100 * 1024 * 1024 // 100 MB
}

fn default_trash_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".tianyan")
        .join("trash")
}

fn default_skill_read_max_size() -> u64 {
    50 * 1024 * 1024 // 50 MB
}

fn default_skill_read_timeout() -> u64 {
    30
}

fn default_skill_write_max_size() -> u64 {
    10 * 1024 * 1024 // 10 MB
}

fn default_skill_write_timeout() -> u64 {
    30
}

fn default_skill_list_max() -> usize {
    10000
}

fn default_skill_http_timeout() -> u64 {
    60
}

fn default_skill_command_timeout() -> u64 {
    300
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            safety_mode: SafetyMode::default(),
            trash_directory: default_trash_dir(),
            allowed_directories: Vec::new(),
            blocked_directories: Vec::new(),
            allowed_commands: Vec::new(),
            blocked_commands: Vec::new(),
            confirm_commands: true,
            max_file_size: default_max_file_size(),
            audit_logging: true,
            skill_file_read_max_size: default_skill_read_max_size(),
            skill_file_read_timeout_secs: default_skill_read_timeout(),
            skill_file_write_max_size: default_skill_write_max_size(),
            skill_file_write_timeout_secs: default_skill_write_timeout(),
            skill_file_list_max_entries: default_skill_list_max(),
            skill_http_timeout_secs: default_skill_http_timeout(),
            skill_command_timeout_secs: default_skill_command_timeout(),
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
