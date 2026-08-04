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

impl TianyanError {
    /// "条目未找到"类错误的统一消息前缀。
    ///
    /// 所有"目标不存在"错误通过 [`Self::not_found`] 构造，判断时用 [`Self::is_not_found`]，
    /// 避免散落的字符串字面量与脆弱的 `starts_with` 判断。
    pub const NOT_FOUND_PREFIX: &str = "条目未找到";

    /// "目录未找到"类错误的统一消息前缀（`is_not_found` 同样识别）。
    pub const DIRECTORY_NOT_FOUND_PREFIX: &str = "目录未找到";

    /// 构造"条目未找到"错误（与"不存在"语义统一的入口）。
    pub fn not_found<T: std::fmt::Display>(detail: T) -> Self {
        TianyanError::Custom(format!("{}：{}", Self::NOT_FOUND_PREFIX, detail))
    }

    /// 判断是否为"目标不存在"类错误（条目未找到 / 目录未找到 / IO NotFound）。
    pub fn is_not_found(&self) -> bool {
        match self {
            TianyanError::Io(e) => e.kind() == std::io::ErrorKind::NotFound,
            TianyanError::Custom(msg) => {
                msg.starts_with(Self::NOT_FOUND_PREFIX)
                    || msg.starts_with(Self::DIRECTORY_NOT_FOUND_PREFIX)
            }
            _ => false,
        }
    }
}

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

    #[test]
    fn test_not_found_constructor_and_predicate() {
        let err = TianyanError::not_found("tianyan://memory/abc");
        assert_eq!(err.to_string(), "条目未找到：tianyan://memory/abc");
        assert!(err.is_not_found());
    }

    #[test]
    fn test_is_not_found_variants() {
        // Io(NotFound) 识别
        let io_err = TianyanError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert!(io_err.is_not_found());
        // 目录未找到识别
        let dir_err = TianyanError::Custom("目录未找到：C:\\data".to_string());
        assert!(dir_err.is_not_found());
        // 其他错误不识别
        let other = TianyanError::Custom("存储后端错误：磁盘已满".to_string());
        assert!(!other.is_not_found());
        let io_other = TianyanError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        assert!(!io_other.is_not_found());
    }
}
