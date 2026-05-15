//! 日志配置模块。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 日志配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// 日志级别（trace、debug、info、warn、error）。
    #[serde(default = "default_log_level")]
    pub level: String,
    /// 日志格式（text、json）。
    #[serde(default = "default_log_format")]
    pub format: String,
    /// 日志文件路径（可选，未设置则输出到控制台）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// 最大日志文件大小（MB）。
    #[serde(default = "default_max_log_size")]
    pub max_file_size: u64,
    /// 保留的日志文件数量。
    #[serde(default = "default_log_files")]
    pub max_files: u32,
    /// 包含时间戳。
    #[serde(default = "default_true")]
    pub include_timestamp: bool,
    /// 包含文件和行号信息。
    #[serde(default)]
    pub include_location: bool,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "text".to_string()
}

fn default_max_log_size() -> u64 {
    10
}

fn default_log_files() -> u32 {
    5
}

fn default_true() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            file: None,
            max_file_size: default_max_log_size(),
            max_files: default_log_files(),
            include_timestamp: true,
            include_location: false,
        }
    }
}

impl LoggingConfig {
    /// 验证日志配置。
    pub fn validate(&self) -> Result<(), String> {
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.level.as_str()) {
            return Err(format!(
                "无效的日志级。
 {}，有效值为: {:?}",
                self.level, valid_levels
            ));
        }

        let valid_formats = ["text", "json"];
        if !valid_formats.contains(&self.format.as_str()) {
            return Err(format!(
                "无效的日志格。
 {}，有效值为: {:?}",
                self.format, valid_formats
            ));
        }

        if self.max_file_size == 0 {
            return Err("max_file_size 必须大于 0".to_string());
        }

        if self.max_files == 0 {
            return Err("max_files 必须大于 0".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_logging_config() {
        let config = LoggingConfig::default();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, "text");
        assert!(config.include_timestamp);
    }

    #[test]
    fn test_logging_validation() {
        let mut config = LoggingConfig::default();
        assert!(config.validate().is_ok());

        config.level = "invalid".to_string();
        assert!(config.validate().is_err());

        config.level = "info".to_string();
        config.format = "invalid".to_string();
        assert!(config.validate().is_err());
    }
}
