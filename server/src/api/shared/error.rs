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
    /// 认证失败
    Unauthorized(String),
    /// 权限不足或操作不允许
    Forbidden(String),
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
            ApiError::Unauthorized(msg) => write!(f, "认证失败：{}", msg),
            ApiError::Forbidden(msg) => write!(f, "禁止访问：{}", msg),
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
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
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
        // 语义谓词优先：统一"目标不存在"判定（Custom 前缀 + Io(NotFound)），
        // 与 core 的 TianyanError::is_not_found 单一来源对齐。
        if err.is_not_found() {
            return ApiError::NotFound(err.to_string());
        }
        // 冲突（语义编辑锚点/旧内容/补丁定位不匹配）→ 409
        if err.is_conflict() {
            return ApiError::Conflict(err.to_string());
        }
        // 无效输入 → 400
        if err.is_invalid_input() {
            return ApiError::BadRequest(err.to_string());
        }
        // 权限拒绝 → 403
        if err.is_permission() {
            return ApiError::Forbidden(err.to_string());
        }
        // 超时 → 504
        if err.is_timeout() {
            return ApiError::GatewayTimeout(err.to_string());
        }
        match err {
            TianyanError::Io(e) => ApiError::Internal(format!("IO 错误：{}", e)),
            TianyanError::Json(e) => ApiError::BadRequest(format!("JSON 解析错误：{}", e)),
            TianyanError::Toml(e) => ApiError::BadRequest(format!("TOML 解析错误：{}", e)),
            TianyanError::Custom(msg) => {
                // 配置错误（core 侧 From<config::ConfigError> 统一前缀）→ 400；
                // 其余语义在谓词链已分类，此处仅剩默认 Internal（ADR-014）
                if msg.starts_with("配置错误：") {
                    ApiError::Config(msg)
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

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::Internal(format!("IO 错误：{}", err))
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

    /// 回归测试：已删除的死前缀 fallback 分支——"未找到结果：" 此前经冗余
    /// 字符串匹配映射 404，但工作区内无任何生产者（语义化分类唯一入口为
    /// TianyanError 构造器 + 谓词，ADR-014），删除后归为默认 Internal。
    #[test]
    fn test_removed_prefix_falls_to_internal() {
        let api_err: ApiError =
            TianyanError::Custom("未找到结果：tianyan://memory/abc".to_string()).into();
        match api_err {
            ApiError::Internal(msg) => assert!(msg.contains("未找到结果")),
            other => panic!("预期 Internal，实际为：{other}"),
        }
    }

    /// 语义谓词（ADR-014）映射不因死前缀清理而改变：not_found → 404、
    /// conflict → 409、invalid_input → 400、permission → 403、timeout → 504。
    #[test]
    fn test_semantic_predicates_map_to_status_codes() {
        let cases: [(TianyanError, StatusCode); 5] = [
            (TianyanError::not_found("会话"), StatusCode::NOT_FOUND),
            (TianyanError::conflict("锚点未命中"), StatusCode::CONFLICT),
            (
                TianyanError::invalid_input("缺少参数"),
                StatusCode::BAD_REQUEST,
            ),
            (
                TianyanError::permission("路径被阻止"),
                StatusCode::FORBIDDEN,
            ),
            (
                TianyanError::timeout("LLM 调用超时"),
                StatusCode::GATEWAY_TIMEOUT,
            ),
        ];
        for (err, expected) in cases {
            let (status, _) = error_to_parts(ApiError::from(err));
            assert_eq!(status, expected, "语义错误应映射为 {expected}");
        }
    }

    fn error_to_parts(err: ApiError) -> (StatusCode, String) {
        let response = err.into_response();
        (response.status(), String::new())
    }
}
