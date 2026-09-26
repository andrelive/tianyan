//! 配置验证模块。
//!
//! 提供配置验证功能，用于配置向导和配置状态检测。

use crate::common::error::TianyanError;
use crate::config::{AgentConfig, ModelsConfig, ProviderConfig, StorageConfig};

/// 配置验证结果。
pub type ValidationResult = Result<(), Vec<TianyanError>>;

/// 验证完整的模型配置。
pub fn validate_models_config(config: &ModelsConfig) -> ValidationResult {
    let mut errors = Vec::new();

    // 检查是否有模型提供商
    if config.providers.is_empty() {
        errors.push(TianyanError::Custom(
            "模型服务错误：至少需要配置一个模型提供商".to_string(),
        ));
    }

    // 验证每个提供商
    for provider in &config.providers {
        if let Err(e) = validate_provider(provider) {
            errors.push(TianyanError::Custom(format!(
                "模型服务错误：提供商 '{}': {}",
                provider.name, e
            )));
        }
    }

    // 检查 preferences 引用的 provider + model 是否有效
    for (label, pref) in [
        ("chat", &config.preferences.chat),
        ("embedding", &config.preferences.embedding),
        ("vision", &config.preferences.vision),
    ] {
        if let Some(r) = pref {
            if !config
                .providers
                .iter()
                .any(|p| p.name == r.provider && p.models.iter().any(|m| m.name == r.model))
            {
                errors.push(TianyanError::Custom(format!(
                    "模型服务错误：preferences.{} 引用的模型 '{}' (提供商 '{}') 不存在",
                    label, r.model, r.provider
                )));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 验证单个模型提供商配置（委托给 ProviderConfig::validate）。
pub fn validate_provider(provider: &ProviderConfig) -> Result<(), String> {
    provider.validate()
}

/// 验证存储配置。
pub fn validate_storage_config(config: &StorageConfig) -> ValidationResult {
    let mut errors = Vec::new();

    // 检查数据目录
    if config.data_dir.as_os_str().is_empty() {
        errors.push(TianyanError::Custom(format!(
            "'{}' 的配置值无效：{}",
            "storage.data_dir", "缺少必要字段"
        )));
    }

    // 检查向量存储配置
    if config.vector.collection_name.is_empty() {
        errors.push(TianyanError::Custom(format!(
            "'{}' 的配置值无效：{}",
            "storage.vector.collection_name", "缺少必要字段"
        )));
    }

    if config.vector.vector_dimension == 0 {
        errors.push(TianyanError::Custom(format!(
            "'{}' 的配置值无效：{}",
            "storage.vector.vector_dimension", "必须大于 0"
        )));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 验证智能体配置（委托给 AgentConfig::validate）。
pub fn validate_agent_config(config: &AgentConfig) -> ValidationResult {
    let mut errors = Vec::new();
    if let Err(e) = config.validate() {
        errors.push(TianyanError::config(format!("智能体配置错误：{}", e)));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 将验证错误转换为字符串列表。
pub fn validation_errors_to_strings(errors: Vec<TianyanError>) -> Vec<String> {
    errors.iter().map(|e| e.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelCapability, ModelEntry};

    fn make_model(name: &str, caps: Vec<ModelCapability>) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            capabilities: caps,
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_provider_valid() {
        let provider = ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            models: vec![make_model("gpt-4", vec![ModelCapability::Chat])],
            timeout: 60,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
        };
        assert!(validate_provider(&provider).is_ok());
    }

    #[test]
    fn test_validate_provider_empty_name() {
        let provider = ProviderConfig {
            name: "".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: None,
            models: vec![make_model("gpt-4", vec![ModelCapability::Chat])],
            timeout: 60,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
        };
        assert!(validate_provider(&provider).is_err());
    }

    #[test]
    fn test_validate_provider_no_models() {
        let provider = ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: None,
            models: vec![],
            timeout: 60,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
        };
        assert!(validate_provider(&provider).is_err());
    }

    #[test]
    fn test_validate_models_config_empty_providers() {
        let config = ModelsConfig {
            providers: vec![],
            preferences: Default::default(),
        };
        let result = validate_models_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_errors_to_strings() {
        let errors = vec![
            TianyanError::config("未找到配置文件"),
            TianyanError::Custom(format!("'{}' 的配置值无效：{}", "test", "缺少必要字段")),
        ];
        let strings = validation_errors_to_strings(errors);
        assert_eq!(strings.len(), 2);
        assert_eq!(strings[0], "配置错误：未找到配置文件");
        assert_eq!(strings[1], "'test' 的配置值无效：缺少必要字段");
    }
}
