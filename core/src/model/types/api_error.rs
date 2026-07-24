use serde::{Deserialize, Serialize};

/// API 错误响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    /// 错误详情。
    pub error: ApiError,
}

/// API 错误详情。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    /// 错误消息。
    pub message: String,
    /// 错误类型。
    #[serde(rename = "type")]
    pub error_type: String,
    /// 错误参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// 错误代码。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}
