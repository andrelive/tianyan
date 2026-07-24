//! 连接测试数据结构。
//!
//! 提供模型连接测试的请求和响应类型。

use serde::{Deserialize, Serialize};

/// 模型连接测试请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionRequest {
    /// API 端点 URL。
    pub endpoint: String,
    /// API 密钥。
    pub api_key: String,
    /// 要测试的模型。
    pub model: String,
}

/// 模型连接测试响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResponse {
    /// 是否成功。
    pub success: bool,
    /// 消息。
    pub message: String,
    /// 可用模型列表（如果成功）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<String>>,
}

impl TestConnectionResponse {
    /// 创建成功响应。
    pub fn success(message: impl Into<String>, models: Vec<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            available_models: Some(models),
        }
    }

    /// 创建失败响应。
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            available_models: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_test_connection_response() {
        let success = TestConnectionResponse::success("连接成功", vec!["model1".to_string()]);
        assert!(success.success);
        assert!(success.available_models.is_some());

        let error = TestConnectionResponse::error("连接失败");
        assert!(!error.success);
        assert!(error.available_models.is_none());
    }
}
