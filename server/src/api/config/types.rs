use serde::{Deserialize, Serialize};
use tianyan::config::TianyanConfig;

/// 配置响应
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub config: TianyanConfig,
}

/// 更新配置请求 — 接受完整的 TianyanConfig
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
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

/// 更新配置响应
#[derive(Debug, Serialize)]
pub struct UpdateConfigResponse {
    pub success: bool,
    pub message: String,
}

/// 模型服务摘要（供前端列表展示）
#[derive(Debug, Serialize)]
pub struct ModelServiceInfo {
    pub name: String,
    pub endpoint: String,
    pub default_model: String,
    pub enabled: bool,
    pub priority: u32,
}

/// 模型服务列表响应
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub services: Vec<ModelServiceInfo>,
    pub default_chat_model: String,
    pub default_embedding_model: String,
    pub default_vision_model: String,
}

/// 切换默认聊天模型请求
#[derive(Debug, Deserialize)]
pub struct SwitchModelRequest {
    pub model: String,
}

impl SwitchModelRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("模型名不能为空".to_string());
        }
        Ok(())
    }
}

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
        };
        assert!(req.validate().is_ok());

        let req = SwitchModelRequest {
            model: "".to_string(),
        };
        assert!(req.validate().is_err());
    }
}
