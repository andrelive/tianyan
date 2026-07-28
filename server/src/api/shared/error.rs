//! API 错误处理
//!
//! 本模块定义了统一的 API 错误类型和错误处理机制。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use tianyan::TianyanError;

/// API 错误类型
///
/// 表示 API 层可能遇到的各种错误情况。
#[derive(Debug)]
pub enum ApiError {
    /// 资源不存在
    NotFound(String),
    /// 请求参数错误
    BadRequest(String),
    /// 内部服务器错误
    Internal(String),
    /// 配置错误
    Config(String),
    /// Agent 相关错误
    Agent(String),
    /// 认证失败
    Unauthorized(String),
    /// 权限不足或操作不允许
    Forbidden(String),
    /// 请求体过大
    PayloadTooLarge(String),
    /// 网关超时
    GatewayTimeout(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::NotFound(msg) => write!(f, "未找到：{}", msg),
            ApiError::BadRequest(msg) => write!(f, "请求错误：{}", msg),
            ApiError::Internal(msg) => write!(f, "内部错误：{}", msg),
            ApiError::Config(msg) => write!(f, "配置错误：{}", msg),
            ApiError::Agent(msg) => write!(f, "Agent 错误：{}", msg),
            ApiError::Unauthorized(msg) => write!(f, "认证失败：{}", msg),
            ApiError::Forbidden(msg) => write!(f, "禁止访问：{}", msg),
            ApiError::PayloadTooLarge(msg) => write!(f, "请求体过大：{}", msg),
            ApiError::GatewayTimeout(msg) => write!(f, "网关超时：{}", msg),
        }
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::Agent(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            ApiError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg.clone()),
            ApiError::GatewayTimeout(msg) => (StatusCode::GATEWAY_TIMEOUT, msg.clone()),
        };

        let body = Json(ErrorResponse {
            success: false,
            error: error_message,
        });

        (status, body).into_response()
    }
}

impl From<TianyanError> for ApiError {
    fn from(err: TianyanError) -> Self {
        match err {
            TianyanError::FileNotFound(path) => {
                ApiError::NotFound(format!("文件未找到：{}", path.display()))
            }
            TianyanError::DirectoryNotFound(path) => {
                ApiError::NotFound(format!("目录未找到：{}", path.display()))
            }
            TianyanError::ConfigNotFound(path) => {
                ApiError::NotFound(format!("配置文件未找到：{}", path.display()))
            }
            TianyanError::ModelNotFound(model) => {
                ApiError::NotFound(format!("模型未找到：{}", model))
            }
            TianyanError::SkillNotFound(skill) => {
                ApiError::NotFound(format!("技能未找到：{}", skill))
            }
            TianyanError::EntryNotFound(uri) => ApiError::NotFound(format!("条目未找到：{}", uri)),
            TianyanError::NoResultsFound(query) => {
                ApiError::NotFound(format!("未找到结果：{}", query))
            }
            TianyanError::InvalidPath(path) => ApiError::BadRequest(format!("无效路径：{}", path)),
            TianyanError::Config(msg)
            | TianyanError::InvalidConfig {
                key: _,
                message: msg,
            } => ApiError::Config(msg),
            TianyanError::InvalidArgument {
                arg: _,
                message: msg,
            }
            | TianyanError::InvalidSkillParameters {
                skill: _,
                message: msg,
            } => ApiError::BadRequest(msg),
            TianyanError::MemorySystem(msg) => ApiError::Internal(msg),
            TianyanError::ModelService(msg)
            | TianyanError::ModelRequestFailed(msg)
            | TianyanError::VlmService(msg) => ApiError::Internal(msg),
            TianyanError::StorageBackend(msg)
            | TianyanError::VirtualFileSystem(msg)
            | TianyanError::EntryAlreadyExists(msg) => ApiError::Internal(msg),
            TianyanError::VectorDatabase(msg) | TianyanError::VectorOperationFailed(msg) => {
                ApiError::Internal(msg)
            }
            TianyanError::DocumentProcessing(msg)
            | TianyanError::UnsupportedDocumentFormat(msg)
            | TianyanError::ImageProcessing(msg)
            | TianyanError::UnsupportedImageFormat(msg) => ApiError::Internal(msg),
            TianyanError::KnowledgeBase(msg) => ApiError::Internal(msg),
            TianyanError::Retrieval(msg) => ApiError::Internal(msg),
            TianyanError::SkillExecution(msg) => ApiError::Internal(msg),
            TianyanError::Internal(msg) | TianyanError::Other(msg) => ApiError::Internal(msg),
            TianyanError::Serialization(msg) => ApiError::Internal(msg),
            TianyanError::JsonError(_) => ApiError::BadRequest("JSON 解析错误".to_string()),
            TianyanError::TomlError(_) => ApiError::BadRequest("TOML 解析错误".to_string()),
            TianyanError::TomlSerializeError(_) => {
                ApiError::Internal("TOML 序列化错误".to_string())
            }
            TianyanError::Network(msg) | TianyanError::HttpRequest(msg) => ApiError::Internal(msg),
            TianyanError::Timeout(msg) => ApiError::GatewayTimeout(msg),
            TianyanError::TokenCounting(msg) => ApiError::Internal(msg),
            TianyanError::SummaryGeneration(msg) => ApiError::Internal(msg),
            TianyanError::Cli(msg) => ApiError::Internal(msg),
            TianyanError::NotImplemented(feature) => {
                ApiError::Internal(format!("功能未实现：{}", feature))
            }
            TianyanError::Io(e) => ApiError::Internal(format!("IO 错误：{}", e)),
            TianyanError::InvalidUri(msg) | TianyanError::UriParseError { uri: msg, .. } => {
                ApiError::BadRequest(format!("无效 URI: {}", msg))
            }
            TianyanError::UnsupportedUriScheme { scheme } => {
                ApiError::BadRequest(format!("不支持的 URI 方案：{}", scheme))
            }
            TianyanError::EmbeddingService(msg) => ApiError::Internal(msg),
            TianyanError::PermissionDenied(msg) => ApiError::Forbidden(msg.clone()),
            TianyanError::OperationNotAllowed(msg) => ApiError::Forbidden(msg.clone()),
            TianyanError::AuthenticationFailed(msg) => ApiError::Unauthorized(msg.clone()),
            TianyanError::Security(msg) => ApiError::Forbidden(msg.clone()),
            TianyanError::Planning(msg) | TianyanError::Execution(msg) => ApiError::Internal(msg),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::BadRequest(format!("JSON 解析错误：{}", err))
    }
}

/// 错误响应体
///
/// 用于统一错误响应的 JSON 格式。
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// 是否成功（始终为 false）
    pub success: bool,
    /// 错误信息
    pub error: String,
}

impl ErrorResponse {
    /// 创建新的错误响应
    ///
    /// # Arguments
    /// * `error` - 错误信息
    ///
    /// # Returns
    /// * `Self` - 新创建的错误响应
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_display() {
        let err = ApiError::NotFound("资源不存在".to_string());
        assert_eq!(format!("{}", err), "未找到：资源不存在");

        let err = ApiError::BadRequest("参数错误".to_string());
        assert_eq!(format!("{}", err), "请求错误：参数错误");
    }

    #[test]
    fn test_error_response_new() {
        let resp = ErrorResponse::new("测试错误");
        assert!(!resp.success);
        assert_eq!(resp.error, "测试错误");
    }

    #[test]
    fn test_from_tianyan_error() {
        let tianyan_err = TianyanError::FileNotFound(std::path::PathBuf::from("/test/path"));
        let api_err: ApiError = tianyan_err.into();
        match api_err {
            ApiError::NotFound(msg) => assert!(msg.contains("文件未找到")),
            _ => panic!("错误的错误类型"),
        }
    }
}
