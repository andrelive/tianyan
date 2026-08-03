use serde::{Deserialize, Serialize};
use tianyan::config::TianyanConfig;

// Re-export shared types from core so server API can use them directly
pub use tianyan::config::api_types::{
    ModelInfo, ModelsResponse, PreferencesInfo, ProviderInfo, SwitchModelRequest,
    UpdateConfigResponse,
};

/// 配置响应
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    /// 天演配置
    pub config: TianyanConfig,
}

/// 更新配置请求 — 接受完整的 TianyanConfig
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    /// 天演配置
    pub config: TianyanConfig,
}

impl UpdateConfigRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        self.config
            .validate()
            .map_err(|e| format!("配置验证失败: {}", e))
    }
}

// Shared types (ModelServiceInfo, ModelsResponse, SwitchModelRequest, UpdateConfigResponse)
// are re-exported from tianyan::config::api_types above.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_response_serialization() {
        let config = TianyanConfig::default();
        let response = ConfigResponse { config };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("agent"));
        assert!(json.contains("models"));
    }

    #[test]
    fn test_switch_model_request_validation() {
        let req = SwitchModelRequest {
            model: "gpt-4".to_string(),
            capability: None,
        };
        assert!(req.validate().is_ok());

        let req = SwitchModelRequest {
            model: "".to_string(),
            capability: None,
        };
        assert!(req.validate().is_err());
    }
}
