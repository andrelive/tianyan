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
    /// 内容冲突（文件已被外部修改、锚点/补丁定位不匹配）
    Conflict(String),
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
            ApiError::Conflict(msg) => write!(f, "冲突：{}", msg),
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
            ApiError::Config(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Agent(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            ApiError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg.clone()),
            ApiError::GatewayTimeout(msg) => (StatusCode::GATEWAY_TIMEOUT, msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
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
            TianyanError::Io(e) => ApiError::Internal(format!("IO 错误：{}", e)),
            TianyanError::Json(e) => ApiError::BadRequest(format!("JSON 解析错误：{}", e)),
            TianyanError::Toml(e) => ApiError::BadRequest(format!("TOML 解析错误：{}", e)),
            TianyanError::Custom(msg) => {
                if msg.starts_with("配置错误：") {
                    ApiError::Config(msg)
                } else if msg.starts_with("未找到结果：")
                    || msg.starts_with("条目未找到：")
                    || msg.starts_with("模型未找到：")
                {
                    ApiError::NotFound(msg)
                } else if msg.starts_with("认证失败：") {
                    ApiError::Unauthorized(msg)
                } else if msg.starts_with("操作不被允许：") || msg.starts_with("安全错误：")
                {
                    ApiError::Forbidden(msg)
                } else if msg.starts_with("操作超时：") {
                    ApiError::GatewayTimeout(msg)
                } else if msg.starts_with("无效路径：") || msg.starts_with("无效 URI:") {
                    ApiError::BadRequest(msg)
                } else {
                    ApiError::Internal(msg)
                }
            }
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
        let tianyan_err = TianyanError::Custom("配置错误：无效的配置".to_string());
        let api_err: ApiError = tianyan_err.into();
        match api_err {
            ApiError::Config(msg) => assert!(msg.contains("无效的配置")),
            _ => panic!("错误的错误类型"),
        }
    }

    /// 回归测试：HTTP 状态码映射 —— 配置校验错误必须是 400（客户端输入），
    /// 而不是 500；NotFound 必须保持 404。
    #[test]
    fn test_http_status_mapping() {
        use axum::http::StatusCode;

        let config_err = ApiError::Config("无效配置".to_string());
        let (status, _) = error_to_parts(config_err);
        assert_eq!(status, StatusCode::BAD_REQUEST, "Config 错误应为 400");

        let not_found = ApiError::NotFound("会话未找到".to_string());
        let (status, _) = error_to_parts(not_found);
        assert_eq!(status, StatusCode::NOT_FOUND, "NotFound 错误应为 404");

        let internal = ApiError::Internal("内部错误".to_string());
        let (status, _) = error_to_parts(internal);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    fn error_to_parts(err: ApiError) -> (StatusCode, String) {
        let response = err.into_response();
        (response.status(), String::new())
    }
}
