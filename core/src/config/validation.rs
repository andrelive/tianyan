//! 配置验证模块。
//!
//! 提供配置验证功能，用于配置向导和配置状态检测。

use crate::config::{AgentConfig, ModelServiceConfig, ModelsConfig, StorageConfig};

/// 配置验证错误。
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValidationError {
    /// 配置文件不存在。
    ConfigNotFound,
    /// 缺少必要字段。
    MissingField(String),
    /// 字段值无效。
    InvalidValue(String, String),
    /// 模型服务配置错误。
    ModelServiceError(String),
    /// 存储配置错误。
    StorageError(String),
    /// 智能体配置错误。
    AgentError(String),
}

impl std::fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigValidationError::ConfigNotFound => write!(f, "未找到配置文件"),
            ConfigValidationError::MissingField(field) => write!(f, "缺少必要字段：{}", field),
            ConfigValidationError::InvalidValue(field, reason) => {
                write!(f, "字段 '{}' 的值无效：{}", field, reason)
            }
            ConfigValidationError::ModelServiceError(msg) => write!(f, "模型服务配置错误：{}", msg),
            ConfigValidationError::StorageError(msg) => write!(f, "存储配置错误：{}", msg),
            ConfigValidationError::AgentError(msg) => write!(f, "智能体配置错误：{}", msg),
        }
    }
}

impl std::error::Error for ConfigValidationError {}

/// 配置验证结果。
pub type ValidationResult = Result<(), Vec<ConfigValidationError>>;

/// 验证完整的模型配置。
pub fn validate_models_config(config: &ModelsConfig) -> ValidationResult {
    let mut errors = Vec::new();

    // 检查是否有模型服务
    if config.services.is_empty() {
        errors.push(ConfigValidationError::ModelServiceError(
            "至少需要配置一个模型服务".to_string(),
        ));
    }

    // 验证每个服务
    for (idx, service) in config.services.iter().enumerate() {
        if let Err(e) = validate_model_service(service) {
            errors.push(ConfigValidationError::ModelServiceError(format!(
                "服务 {} ({}): {}",
                idx + 1,
                service.name,
                e
            )));
        }
    }

    // 检查默认聊天模型是否设置
    if config.default_chat_model.is_empty() {
        errors.push(ConfigValidationError::MissingField(
            "models.default_chat_model".to_string(),
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 验证单个模型服务配置（委托给 ModelServiceConfig::validate）。
pub fn validate_model_service(service: &ModelServiceConfig) -> Result<(), String> {
    service.validate()
}

/// 验证存储配置。
pub fn validate_storage_config(config: &StorageConfig) -> ValidationResult {
    let mut errors = Vec::new();

    // 检查数据目录
    if config.data_dir.as_os_str().is_empty() {
        errors.push(ConfigValidationError::MissingField(
            "storage.data_dir".to_string(),
        ));
    }

    // 检查向量存储配置
    if config.vector.url.is_empty() {
        errors.push(ConfigValidationError::MissingField(
            "storage.vector.url".to_string(),
        ));
    }

    if config.vector.collection_name.is_empty() {
        errors.push(ConfigValidationError::MissingField(
            "storage.vector.collection_name".to_string(),
        ));
    }

    if config.vector.vector_dimension == 0 {
        errors.push(ConfigValidationError::InvalidValue(
            "storage.vector.vector_dimension".to_string(),
            "必须大于 0".to_string(),
        ));
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
        errors.push(ConfigValidationError::AgentError(e));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 将验证错误转换为字符串列表。
pub fn validation_errors_to_strings(errors: Vec<ConfigValidationError>) -> Vec<String> {
    errors.iter().map(|e| e.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelServiceConfig, ModelServiceType};
    use std::collections::HashMap;

    #[test]
    fn test_validate_model_service_valid() {
        let service = ModelServiceConfig {
            name: "test".to_string(),
            service_type: ModelServiceType::OpenAI,
            endpoint: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            default_model: "gpt-4".to_string(),
            models: vec!["gpt-4".to_string()],
            timeout: 60,
            enabled: true,
            priority: 0,
            headers: HashMap::new(),
        };

        assert!(validate_model_service(&service).is_ok());
    }

    #[test]
    fn test_validate_model_service_empty_name() {
        let service = ModelServiceConfig {
            name: "".to_string(),
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

        assert!(validate_model_service(&service).is_err());
    }

    #[test]
    fn test_validate_model_service_invalid_url() {
        let service = ModelServiceConfig {
            name: "test".to_string(),
            service_type: ModelServiceType::OpenAI,
            endpoint: "ftp://api.example.com".to_string(),
            api_key: None,
            default_model: "gpt-4".to_string(),
            models: vec![],
            timeout: 60,
            enabled: true,
            priority: 0,
            headers: HashMap::new(),
        };

        assert!(validate_model_service(&service).is_err());
    }

    #[test]
    fn test_validate_models_config_empty_services() {
        let config = ModelsConfig {
            default_chat_model: "gpt-4".to_string(),
            default_embedding_model: "".to_string(),
            default_vision_model: "".to_string(),
            services: vec![],
        };

        let result = validate_models_config(&config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ConfigValidationError::ModelServiceError(_))));
    }

    #[test]
    fn test_validation_errors_to_strings() {
        let errors = vec![
            ConfigValidationError::ConfigNotFound,
            ConfigValidationError::MissingField("test".to_string()),
        ];

        let strings = validation_errors_to_strings(errors);
        assert_eq!(strings.len(), 2);
        assert_eq!(strings[0], "未找到配置文件");
        assert_eq!(strings[1], "缺少必要字段：test");
    }
}
