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

    /// "冲突"类错误的统一消息前缀（语义编辑锚点不匹配、目标状态过期等）。
    pub const KIND_CONFLICT: &str = "冲突";

    /// "无效输入"类错误的统一消息前缀（参数缺失/格式错误/校验失败）。
    pub const KIND_INVALID_INPUT: &str = "无效输入";

    /// "操作不被允许"类错误的统一消息前缀（权限拒绝/安全策略阻止）。
    pub const KIND_PERMISSION: &str = "操作不被允许";

    /// "操作超时"类错误的统一消息前缀（请求超时/调用超时）。
    pub const KIND_TIMEOUT: &str = "操作超时";

    /// 构造"条目未找到"错误（与"不存在"语义统一的入口）。
    pub fn not_found<T: std::fmt::Display>(detail: T) -> Self {
        TianyanError::Custom(format!("{}：{}", Self::NOT_FOUND_PREFIX, detail))
    }

    /// 构造"冲突"错误（语义编辑锚点不匹配等；判断用 [`Self::is_conflict`]）。
    pub fn conflict<T: std::fmt::Display>(detail: T) -> Self {
        TianyanError::Custom(format!("{}：{}", Self::KIND_CONFLICT, detail))
    }

    /// 构造"无效输入"错误（参数缺失/格式错误；判断用 [`Self::is_invalid_input`]）。
    pub fn invalid_input<T: std::fmt::Display>(detail: T) -> Self {
        TianyanError::Custom(format!("{}：{}", Self::KIND_INVALID_INPUT, detail))
    }

    /// 构造"操作不被允许"错误（权限拒绝/安全策略；判断用 [`Self::is_permission`]）。
    pub fn permission<T: std::fmt::Display>(detail: T) -> Self {
        TianyanError::Custom(format!("{}：{}", Self::KIND_PERMISSION, detail))
    }

    /// 构造"操作超时"错误（请求/调用超时；判断用 [`Self::is_timeout`]）。
    pub fn timeout<T: std::fmt::Display>(detail: T) -> Self {
        TianyanError::Custom(format!("{}：{}", Self::KIND_TIMEOUT, detail))
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

    /// 判断是否为"冲突"类错误（语义编辑锚点不匹配等）。
    pub fn is_conflict(&self) -> bool {
        matches!(self, TianyanError::Custom(msg) if msg.starts_with(Self::KIND_CONFLICT))
    }

    /// 判断是否为"无效输入"类错误（参数缺失/格式错误）。
    pub fn is_invalid_input(&self) -> bool {
        matches!(self, TianyanError::Custom(msg) if msg.starts_with(Self::KIND_INVALID_INPUT))
    }

    /// 判断是否为"操作不被允许"类错误（权限拒绝 / IO PermissionDenied）。
    pub fn is_permission(&self) -> bool {
        match self {
            TianyanError::Io(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
            TianyanError::Custom(msg) => msg.starts_with(Self::KIND_PERMISSION),
            _ => false,
        }
    }

    /// 判断是否为"操作超时"类错误（请求/调用超时）。
    pub fn is_timeout(&self) -> bool {
        matches!(self, TianyanError::Custom(msg) if msg.starts_with(Self::KIND_TIMEOUT))
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
            TianyanError::timeout(err)
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

    #[test]
    fn test_conflict_constructor_and_predicate() {
        let err = TianyanError::conflict("锚点未命中");
        assert_eq!(err.to_string(), "冲突：锚点未命中");
        assert!(err.is_conflict());
        let other = TianyanError::not_found("x");
        assert!(!other.is_conflict());
    }

    #[test]
    fn test_invalid_input_constructor_and_predicate() {
        let err = TianyanError::invalid_input("缺少 file_path");
        assert_eq!(err.to_string(), "无效输入：缺少 file_path");
        assert!(err.is_invalid_input());
        assert!(!err.is_conflict());
    }

    #[test]
    fn test_permission_constructor_and_predicate() {
        let err = TianyanError::permission("路径被阻止");
        assert_eq!(err.to_string(), "操作不被允许：路径被阻止");
        assert!(err.is_permission());
        // Io(PermissionDenied) 同样识别
        let io_err = TianyanError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        assert!(io_err.is_permission());
        // 其他错误不识别
        assert!(!TianyanError::not_found("x").is_permission());
    }

    #[test]
    fn test_timeout_constructor_and_predicate() {
        let err = TianyanError::timeout("LLM 调用超时");
        assert_eq!(err.to_string(), "操作超时：LLM 调用超时");
        assert!(err.is_timeout());
        // 其他错误不识别
        assert!(!TianyanError::not_found("x").is_timeout());
        assert!(!TianyanError::conflict("x").is_timeout());
    }

    #[test]
    fn test_kind_prefixes_do_not_collide() {
        // 各 KIND 前缀互不为对方前缀（谓词 starts_with 精确性）
        let kinds = [
            TianyanError::NOT_FOUND_PREFIX,
            TianyanError::DIRECTORY_NOT_FOUND_PREFIX,
            TianyanError::KIND_CONFLICT,
            TianyanError::KIND_INVALID_INPUT,
            TianyanError::KIND_PERMISSION,
            TianyanError::KIND_TIMEOUT,
        ];
        for a in kinds {
            for b in kinds {
                if a != b {
                    assert!(!a.starts_with(b), "前缀碰撞：{a} 以 {b} 开头——谓词将误判");
                }
            }
        }
    }
}
