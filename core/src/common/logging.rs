//! Logging utilities for the Tianyan agent system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
};

use crate::common::error::Result;

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
    ///
    /// 返回 `std::result::Result` 而非 [`crate::common::error::Result`]，
    /// 与配置校验链（`TianyanConfig::validate` 的 `Result<(), String>`）保持一致。
    pub fn validate(&self) -> std::result::Result<(), String> {
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

/// Initialize the logging system.
pub fn init_logging(config: &LoggingConfig) -> Result<()> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    let subscriber = tracing_subscriber::registry().with(env_filter);

    match config.format.as_str() {
        "json" => {
            let json_layer = fmt::layer()
                .json()
                .with_span_events(FmtSpan::CLOSE)
                .with_target(config.include_location)
                .with_file(config.include_location)
                .with_line_number(config.include_location);

            tracing::subscriber::set_global_default(subscriber.with(json_layer)).map_err(|e| {
                crate::common::error::TianyanError::Custom(format!(
                    "配置错误：Failed to set logging subscriber: {}",
                    e
                ))
            })?;
        }
        _ => {
            let text_layer = fmt::layer()
                .with_target(config.include_location)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_thread_ids(false)
                .with_thread_names(false);

            tracing::subscriber::set_global_default(subscriber.with(text_layer)).map_err(|e| {
                crate::common::error::TianyanError::Custom(format!(
                    "配置错误：Failed to set logging subscriber: {}",
                    e
                ))
            })?;
        }
    }

    Ok(())
}

/// Log level type alias for convenience.
pub type Level = tracing::Level;

/// Create a span for tracing.
#[macro_export]
macro_rules! span {
    ($level:expr, $name:expr) => {
        tracing::span!($level, $name)
    };
    ($level:expr, $name:expr, $($field:expr),+) => {
        tracing::span!($level, $name, $($field),+)
    };
}

/// Log an info message.
#[macro_export]
macro_rules! log_info {
    ($($arg:expr),+) => {
        tracing::info!($($arg),+)
    };
}

/// Log a debug message.
#[macro_export]
macro_rules! log_debug {
    ($($arg:expr),+) => {
        tracing::debug!($($arg),+)
    };
}

/// Log a warning message.
#[macro_export]
macro_rules! log_warn {
    ($($arg:expr),+) => {
        tracing::warn!($($arg),+)
    };
}

/// Log an error message.
#[macro_export]
macro_rules! log_error {
    ($($arg:expr),+) => {
        tracing::error!($($arg),+)
    };
}

/// Log a trace message.
#[macro_export]
macro_rules! log_trace {
    ($($arg:expr),+) => {
        tracing::trace!($($arg),+)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_logging() {
        let config = LoggingConfig::default();
        // Note: Can only initialize once per process
        // This test just verifies the config is valid
        assert!(!config.level.is_empty());
    }

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
