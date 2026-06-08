//! Tianyan 代理系统的错误类型。
//!
//! 本模块使用 `thiserror` crate 定义统一的错误类型，
//! 为所有组件提供全面的错误处理。

use std::path::PathBuf;
use thiserror::Error;

/// Tianyan 代理系统的主要错误类型。
#[derive(Error, Debug)]
pub enum TianyanError {
    /// 配置相关错误
    #[error("配置错误：{0}")]
    Config(String),

    /// 配置文件未找到
    #[error("配置文件未找到：{0}")]
    ConfigNotFound(PathBuf),

    /// 无效的配置
    #[error("'{key}' 的配置值无效：{message}")]
    InvalidConfig { key: String, message: String },

    /// IO 相关错误
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    /// 文件未找到错误
    #[error("文件未找到：{0}")]
    FileNotFound(PathBuf),

    /// 目录未找到错误
    #[error("目录未找到：{0}")]
    DirectoryNotFound(PathBuf),

    /// 权限拒绝错误
    #[error("权限被拒绝：{0}")]
    PermissionDenied(String),

    /// 序列化/反序列化错误
    #[error("序列化错误：{0}")]
    Serialization(String),

    /// JSON 解析错误
    #[error("JSON 解析错误：{0}")]
    JsonError(#[from] serde_json::Error),

    /// TOML 解析错误
    #[error("TOML 解析错误：{0}")]
    TomlError(#[from] toml::de::Error),

    /// TOML 序列化错误
    #[error("TOML 序列化错误：{0}")]
    TomlSerializeError(String),

    /// URI 相关错误
    #[error("无效的 URI：{0}")]
    InvalidUri(String),

    /// URI 解析错误
    #[error("URI 解析错误：{source}")]
    UriParseError {
        uri: String,
        source: url::ParseError,
    },

    /// 不支持的 URI 方案
    #[error("不支持的 URI 方案 '{scheme}'。预期使用 'tianyan://'")]
    UnsupportedUriScheme { scheme: String },

    /// 模型服务错误
    #[error("模型服务错误：{0}")]
    ModelService(String),

    /// 模型未找到
    #[error("模型未找到：{0}")]
    ModelNotFound(String),

    /// 模型请求失败
    #[error("模型请求失败：{0}")]
    ModelRequestFailed(String),

    /// 嵌入服务错误
    #[error("嵌入服务错误：{0}")]
    EmbeddingService(String),

    /// VLM（视觉语言模型）服务错误
    #[error("VLM 服务错误：{0}")]
    VlmService(String),

    /// 视觉编码器错误
    #[error("视觉编码器错误：{0}")]
    VisionEncoder(String),

    /// 存储后端错误
    #[error("存储后端错误：{0}")]
    StorageBackend(String),

    /// 虚拟文件系统错误
    #[error("虚拟文件系统错误：{0}")]
    VirtualFileSystem(String),

    /// 虚拟文件系统中未找到条目
    #[error("URI 处未找到条目：{0}")]
    EntryNotFound(String),

    /// 条目已存在
    #[error("URI 处条目已存在：{0}")]
    EntryAlreadyExists(String),

    /// 虚拟文件系统中的无效路径
    #[error("虚拟文件系统中的无效路径：{0}")]
    InvalidPath(String),

    /// 向量数据库错误
    #[error("向量数据库错误：{0}")]
    VectorDatabase(String),

    /// 向量操作失败
    #[error("向量操作失败：{0}")]
    VectorOperationFailed(String),

    /// 文档处理错误
    #[error("文档处理错误：{0}")]
    DocumentProcessing(String),

    /// 不支持的文档格式
    #[error("不支持的文档格式：{0}")]
    UnsupportedDocumentFormat(String),

    /// 图片处理错误
    #[error("图片处理错误：{0}")]
    ImageProcessing(String),

    /// 不支持的图片格式
    #[error("不支持的图片格式：{0}")]
    UnsupportedImageFormat(String),

    /// 知识库错误
    #[error("知识库错误：{0}")]
    KnowledgeBase(String),

    /// 记忆系统错误
    #[error("记忆系统错误：{0}")]
    MemorySystem(String),

    /// 技能执行错误
    #[error("技能执行错误：{0}")]
    SkillExecution(String),

    /// 技能未找到
    #[error("技能未找到：{0}")]
    SkillNotFound(String),

    /// 无效的技能参数
    #[error("'{skill}' 的技能参数无效：{message}")]
    InvalidSkillParameters { skill: String, message: String },

    /// 安全相关错误
    #[error("安全错误：{0}")]
    Security(String),

    /// 操作不被允许
    #[error("操作不被允许：{0}")]
    OperationNotAllowed(String),

    /// 认证失败
    #[error("认证失败：{0}")]
    AuthenticationFailed(String),

    /// HTTP 请求错误
    #[error("HTTP 请求错误：{0}")]
    HttpRequest(String),

    /// 网络错误
    #[error("网络错误：{0}")]
    Network(String),

    /// 超时错误
    #[error("操作超时：{0}")]
    Timeout(String),

    /// Token 计数错误
    #[error("Token 计数错误：{0}")]
    TokenCounting(String),

    /// 摘要生成错误
    #[error("摘要生成错误：{0}")]
    SummaryGeneration(String),

    /// 检索错误
    #[error("检索错误：{0}")]
    Retrieval(String),

    /// 规划错误
    #[error("规划错误：{0}")]
    Planning(String),

    /// 执行错误
    #[error("执行错误：{0}")]
    Execution(String),

    /// 未找到结果
    #[error("查询未找到结果：{0}")]
    NoResultsFound(String),

    /// CLI 错误
    #[error("CLI 错误：{0}")]
    Cli(String),

    /// 无效参数
    #[error("无效参数 '{arg}'：{message}")]
    InvalidArgument { arg: String, message: String },

    /// 内部错误（正常操作中不应发生）
    #[error("内部错误：{0}")]
    Internal(String),

    /// 功能未实现
    #[error("功能未实现：{0}")]
    NotImplemented(String),

    /// 带消息的通用错误
    #[error("{0}")]
    Other(String),
}

/// 错误类别，用于分类处理和可维护性策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 配置错误（不应重试，需人工修复）
    Config,
    /// IO 错误（可重试）
    Io,
    /// 模型服务错误（临时性，可重试）
    Model,
    /// 存储后端错误（部分可重试）
    Storage,
    /// 技能执行错误（视情况重试）
    Skill,
    /// 安全/权限错误（不应重试）
    Security,
    /// HTTP/网络请求错误（可重试）
    Http,
    /// Token 相关错误（不应重试，需调整策略）
    Token,
    /// 执行/规划错误（不应重试，需修正逻辑）
    Execution,
    /// 序列化/反序列化错误（不应重试，数据问题）
    Serialization,
    /// 知识库/文档处理错误（部分可重试）
    Knowledge,
    /// 内部/未实现错误（不应重试）
    Internal,
    /// 其他未分类错误
    Other,
}

impl TianyanError {
    /// 获取错误的类别。
    pub fn category(&self) -> ErrorCategory {
        match self {
            TianyanError::Config(_)
            | TianyanError::ConfigNotFound(_)
            | TianyanError::InvalidConfig { .. }
            | TianyanError::Cli(_)
            | TianyanError::InvalidArgument { .. } => ErrorCategory::Config,

            TianyanError::Io(_)
            | TianyanError::FileNotFound(_)
            | TianyanError::DirectoryNotFound(_) => ErrorCategory::Io,

            TianyanError::ModelService(_)
            | TianyanError::ModelNotFound(_)
            | TianyanError::ModelRequestFailed(_)
            | TianyanError::EmbeddingService(_)
            | TianyanError::VlmService(_)
            | TianyanError::VisionEncoder(_) => ErrorCategory::Model,

            TianyanError::StorageBackend(_)
            | TianyanError::VirtualFileSystem(_)
            | TianyanError::EntryNotFound(_)
            | TianyanError::EntryAlreadyExists(_)
            | TianyanError::InvalidPath(_)
            | TianyanError::VectorDatabase(_)
            | TianyanError::VectorOperationFailed(_)
            | TianyanError::InvalidUri(_)
            | TianyanError::UriParseError { .. }
            | TianyanError::UnsupportedUriScheme { .. } => ErrorCategory::Storage,

            TianyanError::SkillExecution(_)
            | TianyanError::SkillNotFound(_)
            | TianyanError::InvalidSkillParameters { .. } => ErrorCategory::Skill,

            TianyanError::Security(_)
            | TianyanError::OperationNotAllowed(_)
            | TianyanError::AuthenticationFailed(_)
            | TianyanError::PermissionDenied(_) => ErrorCategory::Security,

            TianyanError::HttpRequest(_) | TianyanError::Network(_) | TianyanError::Timeout(_) => {
                ErrorCategory::Http
            }

            TianyanError::TokenCounting(_) => {
                ErrorCategory::Token
            }

            TianyanError::Planning(_)
            | TianyanError::Execution(_)
            | TianyanError::Retrieval(_)
            | TianyanError::SummaryGeneration(_) => ErrorCategory::Execution,

            TianyanError::Serialization(_)
            | TianyanError::JsonError(_)
            | TianyanError::TomlError(_)
            | TianyanError::TomlSerializeError(_) => ErrorCategory::Serialization,

            TianyanError::DocumentProcessing(_)
            | TianyanError::UnsupportedDocumentFormat(_)
            | TianyanError::ImageProcessing(_)
            | TianyanError::UnsupportedImageFormat(_)
            | TianyanError::KnowledgeBase(_) => ErrorCategory::Knowledge,

            TianyanError::MemorySystem(_) | TianyanError::NoResultsFound(_) => {
                ErrorCategory::Execution
            }

            TianyanError::Internal(_) | TianyanError::NotImplemented(_) => ErrorCategory::Internal,

            TianyanError::Other(_) => ErrorCategory::Other,
        }
    }

    /// 是否可重试。
    ///
    /// 临时性错误（网络超时、模型服务临时不可用）可重试。
    /// 结构性错误（配置缺失、权限不足）不应重试。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.category(),
            ErrorCategory::Io | ErrorCategory::Model | ErrorCategory::Http
        )
    }

    /// 是否应向用户展示详细信息。
    ///
    /// 只有用户行为相关的错误才应展示详细信息。
    /// 内部错误和配置错误应简化为通用消息。
    pub fn is_user_facing(&self) -> bool {
        matches!(
            self.category(),
            ErrorCategory::Skill
                | ErrorCategory::Security
                | ErrorCategory::Token
                | ErrorCategory::Config
        )
    }

    /// 获取机器可读的错误码，用于监控和告警。
    pub fn error_code(&self) -> &'static str {
        match self.category() {
            ErrorCategory::Config => "CONFIG_ERROR",
            ErrorCategory::Io => "IO_ERROR",
            ErrorCategory::Model => "MODEL_ERROR",
            ErrorCategory::Storage => "STORAGE_ERROR",
            ErrorCategory::Skill => "SKILL_ERROR",
            ErrorCategory::Security => "SECURITY_ERROR",
            ErrorCategory::Http => "HTTP_ERROR",
            ErrorCategory::Token => "TOKEN_ERROR",
            ErrorCategory::Execution => "EXECUTION_ERROR",
            ErrorCategory::Serialization => "SERIALIZATION_ERROR",
            ErrorCategory::Knowledge => "KNOWLEDGE_ERROR",
            ErrorCategory::Internal => "INTERNAL_ERROR",
            ErrorCategory::Other => "UNKNOWN_ERROR",
        }
    }
}

/// 使用 TianyanError 的 Result 类型别名。
pub type Result<T> = std::result::Result<T, TianyanError>;

impl From<url::ParseError> for TianyanError {
    fn from(err: url::ParseError) -> Self {
        TianyanError::InvalidUri(err.to_string())
    }
}

impl From<config::ConfigError> for TianyanError {
    fn from(err: config::ConfigError) -> Self {
        TianyanError::Config(err.to_string())
    }
}

impl From<crate::executor::types::ExecutorError> for TianyanError {
    fn from(err: crate::executor::types::ExecutorError) -> Self {
        TianyanError::Execution(err.to_string())
    }
}

impl From<reqwest::Error> for TianyanError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            TianyanError::Timeout(err.to_string())
        } else if err.is_connect() {
            TianyanError::Network(err.to_string())
        } else {
            TianyanError::HttpRequest(err.to_string())
        }
    }
}

impl From<toml::ser::Error> for TianyanError {
    fn from(err: toml::ser::Error) -> Self {
        TianyanError::TomlSerializeError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error() {
        let err = TianyanError::Config("test".to_string());
        assert!(err.to_string().contains("配置错误"));
    }

    #[test]
    fn test_io_error() {
        let err = TianyanError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file"));
        assert!(err.to_string().contains("IO 错误"));
    }

    #[test]
    fn test_model_service_error() {
        let err = TianyanError::ModelService("connection failed".to_string());
        assert!(err.to_string().contains("模型服务错误"));
    }

    #[test]
    fn test_planning_error() {
        let err = TianyanError::Planning("depth exceeded".to_string());
        assert!(err.to_string().contains("规划错误"));
    }

    #[test]
    fn test_execution_error() {
        let err = TianyanError::Execution("step failed".to_string());
        assert!(err.to_string().contains("执行错误"));
    }

    #[test]
    fn test_error_category_config() {
        let err = TianyanError::ConfigNotFound(PathBuf::new());
        assert_eq!(err.category(), ErrorCategory::Config);
    }

    #[test]
    fn test_error_category_model() {
        let err = TianyanError::ModelService("fail".into());
        assert_eq!(err.category(), ErrorCategory::Model);
    }

    #[test]
    fn test_error_category_storage() {
        let err = TianyanError::EntryNotFound("tianyan://x".into());
        assert_eq!(err.category(), ErrorCategory::Storage);
    }

    #[test]
    fn test_error_category_http_timeout() {
        let err = TianyanError::Timeout("timeout".into());
        assert_eq!(err.category(), ErrorCategory::Http);
    }

    #[test]
    fn test_is_retryable() {
        assert!(TianyanError::Network("network".into()).is_retryable());
        assert!(TianyanError::Timeout("timeout".into()).is_retryable());
        assert!(!TianyanError::Config("config".into()).is_retryable());
        assert!(!TianyanError::Security("security".into()).is_retryable());
    }

    #[test]
    fn test_is_user_facing() {
        assert!(TianyanError::Config("配置".into()).is_user_facing());
        assert!(TianyanError::SkillNotFound("技能".into()).is_user_facing());
        assert!(!TianyanError::Internal("内部".into()).is_user_facing());
        assert!(!TianyanError::Network("网络".into()).is_user_facing());
    }

    #[test]
    fn test_error_code() {
        assert_eq!(
            TianyanError::Config("x".into()).error_code(),
            "CONFIG_ERROR"
        );
        assert_eq!(
            TianyanError::ModelService("x".into()).error_code(),
            "MODEL_ERROR"
        );
        assert_eq!(
            TianyanError::Internal("x".into()).error_code(),
            "INTERNAL_ERROR"
        );
    }
}
