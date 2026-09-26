use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tianyan::config::TianyanConfig;
use tianyan::model::spec::ModelSpec;

// Re-export shared types from core so server API can use them directly
pub use tianyan::config::api_types::{
    ModelCatalogInfo, ModelInfo, ModelsResponse, PreferencesInfo, ProviderInfo, SwitchModelRequest,
    UpdateConfigResponse,
};

/// 配置响应
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    /// 天演配置
    pub config: TianyanConfig,
    /// 各模型解析后的完整规格（只读展示字段，key = "{provider}/{model}";
    /// 缺省时省略该字段以兼容旧客户端）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_specs: Option<HashMap<String, ModelSpec>>,
    /// 各模型的内置目录命中信息（advisory：显示名 + 默认档位；
    /// key = "{provider}/{model}"；缺省时省略该字段以兼容旧客户端）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalog: Option<HashMap<String, ModelCatalogInfo>>,
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
        let response = ConfigResponse {
            config,
            model_specs: None,
            model_catalog: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("agent"));
        assert!(json.contains("models"));
    }

    #[test]
    fn test_config_response_model_specs_field() {
        use std::collections::HashMap;
        use tianyan::model::spec::ModelSpec;

        let config = TianyanConfig::default();
        // 填充时序列化含 model_specs（key = "{provider}/{model}"）
        let mut specs = HashMap::new();
        specs.insert(
            "deepseek/deepseek-v4-flash".to_string(),
            ModelSpec::default(),
        );
        let response = ConfigResponse {
            config: config.clone(),
            model_specs: Some(specs),
            model_catalog: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"model_specs\""));
        assert!(json.contains("deepseek/deepseek-v4-flash"));
        // 缺省时省略该字段（旧客户端兼容）
        let response = ConfigResponse {
            config,
            model_specs: None,
            model_catalog: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("model_specs"));
    }

    #[test]
    fn test_switch_model_request_validation() {
        let req = SwitchModelRequest {
            model: "gpt-4".to_string(),
            capability: None,
            provider: None,
        };
        assert!(req.validate().is_ok());

        let req = SwitchModelRequest {
            model: "".to_string(),
            capability: None,
            provider: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_switch_model_request_provider_optional() {
        // 旧客户端载荷（无 provider）：serde default 保证继续可解析
        let req: SwitchModelRequest =
            serde_json::from_str(r#"{"model":"glm-5.3-flash","capability":"chat"}"#)
                .expect("无 provider 载荷必须可解析");
        assert_eq!(req.model, "glm-5.3-flash");
        assert_eq!(req.provider, None);

        // 新载荷带 provider：同名模型跨提供商消歧的输入
        let req: SwitchModelRequest = serde_json::from_str(
            r#"{"model":"glm-5.3-flash","capability":"chat","provider":"ollama"}"#,
        )
        .expect("带 provider 载荷必须可解析");
        assert_eq!(req.provider.as_deref(), Some("ollama"));
    }
}
