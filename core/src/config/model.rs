//! 模型配置模块。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 模型配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// 聊天补全的默认模型。
    #[serde(default = "default_chat_model")]
    pub default_chat_model: String,
    /// 嵌入的默认模型。
    #[serde(default = "default_embedding_model")]
    pub default_embedding_model: String,
    /// 视觉的默认模型。
    #[serde(default = "default_vision_model")]
    pub default_vision_model: String,
    /// 模型服务配置。
    #[serde(default)]
    pub services: Vec<ModelServiceConfig>,
}

/// 模型服务配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelServiceConfig {
    /// 服务名称（如 "openai"、"claude"、"deepseek"）。
    pub name: String,
    /// 服务类型。
    #[serde(rename = "type")]
    pub service_type: ModelServiceType,
    /// API 端点 URL。
    pub endpoint: String,
    /// API 密钥。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// 此服务的默认模型。
    pub default_model: String,
    /// 此服务可用的模型。
    #[serde(default)]
    pub models: Vec<String>,
    /// 请求超时时间（秒）。
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// 是否启用此服务。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 路由优先级（数值越大越优先）。
    #[serde(default)]
    pub priority: u32,
    /// 自定义请求头。
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// 模型服务类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ModelServiceType {
    /// OpenAI 兼容 API。
    #[default]
    OpenAI,
    /// 自定义 OpenAI 兼容 API。
    Custom,
}

fn default_chat_model() -> String {
    "gpt-4".to_string()
}

fn default_embedding_model() -> String {
    "text-embedding-3-small".to_string()
}

fn default_vision_model() -> String {
    "gpt-4-vision-preview".to_string()
}

fn default_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            default_chat_model: default_chat_model(),
            default_embedding_model: default_embedding_model(),
            default_vision_model: default_vision_model(),
            services: Vec::new(),
        }
    }
}

impl ModelServiceConfig {
    /// 验证模型服务配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("服务名称不能为空".to_string());
        }

        if self.endpoint.is_empty() {
            return Err("API 端点 URL 不能为空".to_string());
        }

        // 验证 URL 格式
        if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            return Err(format!(
                "API 端点 URL 格式无效：{}，必须以 http:// 或 https:// 开头",
                self.endpoint
            ));
        }

        if self.default_model.is_empty() {
            return Err("默认模型不能为空".to_string());
        }

        if self.timeout == 0 {
            return Err("超时时间必须大于 0".to_string());
        }
        if self.timeout > 3600 {
            return Err("超时时间不能超过 3600 秒".to_string());
        }
        Ok(())
    }
}

impl ModelsConfig {
    /// 验证模型配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.services.is_empty() {
            return Err("至少需要配置一个模型服务".to_string());
        }

        for service in &self.services {
            service.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_models_config() {
        let config = ModelsConfig::default();
        assert!(!config.default_chat_model.is_empty());
        assert!(!config.default_embedding_model.is_empty());
    }

    #[test]
    fn test_service_validation() {
        let service = ModelServiceConfig {
            name: "test".to_string(),
            service_type: ModelServiceType::OpenAI,
            endpoint: "https://api.example.com".to_string(),
            api_key: None,
            default_model: "gpt-4".to_string(),
            models: vec![],
            timeout: 60,
            enabled: true,
            priority: 0,
            headers: HashMap::new(),
        };
        assert!(service.validate().is_ok());

        let mut invalid_service = service.clone();
        invalid_service.name = "".to_string();
        assert!(invalid_service.validate().is_err());
    }

    #[test]
    fn test_model_service_type_serde() {
        // 测试序列。
        let openai = ModelServiceType::OpenAI;
        let serialized = serde_json::to_string(&openai).unwrap();
        assert_eq!(serialized, "\"openai\"");

        let custom = ModelServiceType::Custom;
        let serialized = serde_json::to_string(&custom).unwrap();
        assert_eq!(serialized, "\"custom\"");

        // 测试反序列化
        let deserialized: ModelServiceType = serde_json::from_str("\"openai\"").unwrap();
        assert_eq!(deserialized, ModelServiceType::OpenAI);

        let deserialized: ModelServiceType = serde_json::from_str("\"custom\"").unwrap();
        assert_eq!(deserialized, ModelServiceType::Custom);
    }

    #[test]
    fn test_model_service_config_toml_serde() {
        let toml_str = r#"
name = "test"
type = "openai"
endpoint = "https://api.example.com"
default_model = "gpt-4"
timeout = 60
enabled = true
priority = 0
"#;

        let config: ModelServiceConfig = toml::from_str(toml_str).expect("TOML解析失败");
        assert_eq!(config.service_type, ModelServiceType::OpenAI);
        assert_eq!(config.name, "test");
    }
}
