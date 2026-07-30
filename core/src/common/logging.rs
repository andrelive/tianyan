//! Logging utilities for the Tianyan agent system.

use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
};

use crate::common::error::Result;
use crate::config::LoggingConfig;

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
}
