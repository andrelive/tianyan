//! Tianyan 错误类型。
//!
//! **约束**：禁止引入 `TianyanError` 之外的错误类型。
//! 模块内部错误通过 `Custom(String)` 变体传递，调用方在消息中携带足够上下文。

use thiserror::Error;

/// Tianyan 系统的唯一错误类型。
///
/// 仅保留有 `#[from]` 自动转换的结构化变体（Io / Json / Toml）。
/// 其余所有错误均通过 `Custom(String)` 传递。
#[derive(Error, Debug)]
pub enum TianyanError {
    /// IO 错误。
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    /// JSON 序列化/反序列化错误。
    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),

    /// TOML 反序列化错误。
    #[error("TOML 错误：{0}")]
    Toml(#[from] toml::de::Error),

    /// 自定义错误。用于所有无法通过上述变体自动转换的错误。
    /// 调用方应在消息中添加模块/操作前缀以便定位问题。
    #[error("{0}")]
    Custom(String),
}

/// 使用 TianyanError 的 Result 类型别名。
pub type Result<T> = std::result::Result<T, TianyanError>;

// ── From impls ──────────────────────────────────────────────

impl From<url::ParseError> for TianyanError {
    fn from(err: url::ParseError) -> Self {
        TianyanError::Custom(format!("URI 解析错误：{err}"))
    }
}

impl From<config::ConfigError> for TianyanError {
    fn from(err: config::ConfigError) -> Self {
        TianyanError::Custom(format!("配置错误：{err}"))
    }
}



impl From<reqwest::Error> for TianyanError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            TianyanError::Custom(format!("请求超时：{err}"))
        } else if err.is_connect() {
            TianyanError::Custom(format!("网络连接失败：{err}"))
        } else {
            TianyanError::Custom(format!("HTTP 请求错误：{err}"))
        }
    }
}

impl From<toml::ser::Error> for TianyanError {
    fn from(err: toml::ser::Error) -> Self {
        TianyanError::Custom(format!("TOML 序列化错误：{err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_error_display() {
        let err = TianyanError::Custom("配置错误：test".to_string());
        assert_eq!(err.to_string(), "配置错误：test");
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: TianyanError = io_err.into();
        assert!(err.to_string().contains("IO 错误"));
    }

    #[test]
    fn test_json_error_from() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: TianyanError = json_err.into();
        assert!(err.to_string().contains("JSON 错误"));
    }

    #[test]
    fn test_toml_error_from() {
        let toml_err = toml::from_str::<toml::Value>("invalid = [").unwrap_err();
        let err: TianyanError = toml_err.into();
        assert!(err.to_string().contains("TOML 错误"));
    }

    #[test]
    fn test_custom_prefix() {
        let err = TianyanError::Custom("模型服务错误：connection refused".to_string());
        assert!(err.to_string().contains("模型服务错误"));
    }
}
