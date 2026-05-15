use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::common::error::{Result, TianyanError};

use super::model_info::ModelProvider;

fn default_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// 服务名称（如  "openai"、"deepseek"）。
    pub name: String,
    /// 服务提供商类型。
    pub provider: ModelProvider,
    /// API 密钥。
    pub api_key: String,
    /// API 基础 URL（None 时根据 provider 推断默认值）。
    #[serde(default)]
    pub base_url: Option<String>,
    /// 聊天模型（必填）。
    pub chat_model: String,
    /// 嵌入模型。
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// 视觉模型。
    #[serde(default)]
    pub vision_model: Option<String>,
    /// 请求超时（秒）。
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// 自定义请求头。
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// 是否启用此服务。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 路由优先级（数值越大越优先）。
    #[serde(default)]
    pub priority: u32,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            provider: ModelProvider::OpenAI,
            api_key: String::new(),
            base_url: None,
            chat_model: String::new(),
            embedding_model: None,
            vision_model: None,
            timeout: default_timeout(),
            headers: HashMap::new(),
            enabled: true,
            priority: 0,
        }
    }
}

impl ModelConfig {
    pub fn new(provider: ModelProvider, api_key: impl Into<String>) -> Self {
        Self {
            provider,
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn with_chat_model(mut self, model: impl Into<String>) -> Self {
        self.chat_model = model.into();
        self
    }

    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = Some(model.into());
        self
    }

    pub fn with_vision_model(mut self, model: impl Into<String>) -> Self {
        self.vision_model = Some(model.into());
        self
    }

    pub fn with_timeout(mut self, timeout: u64) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub fn resolve_api_key(&self) -> String {
        if self.api_key.starts_with("${") && self.api_key.ends_with('}') {
            let var_name = &self.api_key[2..self.api_key.len() - 1];
            std::env::var(var_name).unwrap_or_else(|_| self.api_key.clone())
        } else {
            self.api_key.clone()
        }
    }

    pub fn get_base_url(&self) -> Result<String> {
        if let Some(ref url) = self.base_url {
            Ok(url.clone())
        } else {
            match self.provider {
                ModelProvider::OpenAI => Ok("https://api.openai.com/v1".to_string()),
                ModelProvider::OpenAICompatible => Err(TianyanError::Config(
                    "OpenAI-compatible 提供商需要设置 base_url".to_string(),
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_resolve_api_key() {
        std::env::set_var("TEST_API_KEY", "test_key_value");
        let config = ModelConfig::new(ModelProvider::OpenAI, "${TEST_API_KEY}");
        assert_eq!(config.resolve_api_key(), "test_key_value");
        std::env::remove_var("TEST_API_KEY");
    }

    #[test]
    fn test_model_config_default_base_url() {
        let config = ModelConfig::new(ModelProvider::OpenAI, "test_key");
        assert_eq!(config.get_base_url().unwrap(), "https://api.openai.com/v1");
    }
}
