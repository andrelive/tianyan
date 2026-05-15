//! 配置向导模块。
//!
//! 提供配置向导相关的状态管理和数据结构。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::{
    AgentConfig, LoggingConfig, MemoryConfig, ModelServiceConfig, ModelServiceType, ModelsConfig,
    RetrievalConfig, SecurityConfig, StorageConfig, SummaryServiceConfig,
    TianyanConfig, VectorStorageConfig,
};

/// 配置状态响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigStatus {
    /// 是否已配置。
    pub configured: bool,
    /// 配置文件路径（如果存在）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
    /// 配置错误列表。
    #[serde(default)]
    pub errors: Vec<String>,
    /// 核心默认系统提示词（未配置时返回，供向导使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_system_prompt: Option<String>,
}

impl ConfigStatus {
    /// 创建已配置状态。
    pub fn configured(path: PathBuf) -> Self {
        Self {
            configured: true,
            config_path: Some(path),
            errors: Vec::new(),
            default_system_prompt: None,
        }
    }

    /// 创建未配置状态。
    pub fn not_configured(errors: Vec<String>) -> Self {
        Self {
            configured: false,
            config_path: None,
            errors,
            default_system_prompt: Some(default_system_prompt()),
        }
    }

    /// 创建配置存在但无效的状态。
    pub fn invalid(path: PathBuf, errors: Vec<String>) -> Self {
        Self {
            configured: false,
            config_path: Some(path),
            errors,
            default_system_prompt: Some(default_system_prompt()),
        }
    }
}

/// 向导配置数据（从前端接收）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WizardConfig {
    /// 模型配置。
    pub models: WizardModelsConfig,
    /// 存储配置。
    pub storage: WizardStorageConfig,
    /// 智能体配置。
    pub agent: WizardAgentConfig,
}

/// 向导模型配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardModelsConfig {
    /// 模型服务列表。
    pub services: Vec<WizardModelService>,
    /// 默认聊天模型。
    pub default_chat_model: String,
    /// 默认嵌入模型。
    #[serde(default)]
    pub default_embedding_model: String,
    /// 默认视觉模型。
    #[serde(default)]
    pub default_vision_model: String,
}

/// 向导模型服务配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardModelService {
    /// 服务名称。
    pub name: String,
    /// 服务类型。
    #[serde(rename = "type")]
    pub service_type: ModelServiceType,
    /// API 端点 URL。
    pub endpoint: String,
    /// API 密钥。
    pub api_key: String,
    /// 默认模型。
    pub default_model: String,
    /// 可用模型列表。
    #[serde(default)]
    pub models: Vec<String>,
    /// 超时时间（秒）。
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// 是否启用。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 优先级。
    #[serde(default)]
    pub priority: u32,
}

/// 向导存储配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardStorageConfig {
    /// 数据目录。
    pub data_dir: PathBuf,
    /// 向量存储配置。
    #[serde(default)]
    pub vector: WizardVectorStorageConfig,
}

/// 向导向量存储配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardVectorStorageConfig {
    /// Qdrant 服务 URL。
    #[serde(default = "default_qdrant_url")]
    pub url: String,
    /// 集合名称。
    #[serde(default = "default_qdrant_collection")]
    pub collection_name: String,
    /// 向量维度。
    #[serde(default = "default_vector_dimension")]
    pub vector_dimension: usize,
}

/// 向导智能体配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardAgentConfig {
    /// 保留的最大对话历史消息数。
    #[serde(default = "default_max_history_messages")]
    pub max_history_messages: usize,
    /// 上下文的最大 Token 数。
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    /// 系统提示词。
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    /// 是否启用检索增强。
    #[serde(default = "default_true")]
    pub enable_retrieval: bool,
    /// 是否启用记忆持久化。
    #[serde(default = "default_true")]
    pub enable_memory: bool,
    /// 是否流式输出响应。
    #[serde(default = "default_true")]
    pub stream_responses: bool,
    /// 是否启用思考模式。
    #[serde(default = "default_false")]
    pub enable_thinking: bool,
    /// 默认检索 top-k 值。
    #[serde(default = "default_retrieval_top_k")]
    pub retrieval_top_k: usize,
}

// 默认值函。
fn default_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_qdrant_url() -> String {
    "http://localhost:6334".to_string()
}

fn default_qdrant_collection() -> String {
    "tianyan_contexts".to_string()
}

fn default_vector_dimension() -> usize {
    1536
}

fn default_max_history_messages() -> usize {
    10
}

fn default_max_context_tokens() -> usize {
    8000
}

fn default_system_prompt() -> String {
    include_str!("../agent/default_soul.md").to_string()
}

fn default_retrieval_top_k() -> usize {
    10
}

impl Default for WizardModelsConfig {
    fn default() -> Self {
        Self {
            services: vec![WizardModelService::default()],
            default_chat_model: String::new(),
            default_embedding_model: String::new(),
            default_vision_model: String::new(),
        }
    }
}

impl Default for WizardModelService {
    fn default() -> Self {
        Self {
            name: String::new(),
            service_type: ModelServiceType::OpenAI,
            endpoint: String::new(),
            api_key: String::new(),
            default_model: String::new(),
            models: Vec::new(),
            timeout: default_timeout(),
            enabled: true,
            priority: 0,
        }
    }
}

impl Default for WizardStorageConfig {
    fn default() -> Self {
        Self {
            data_dir: dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("tianyan"),
            vector: WizardVectorStorageConfig::default(),
        }
    }
}

impl Default for WizardVectorStorageConfig {
    fn default() -> Self {
        Self {
            url: default_qdrant_url(),
            collection_name: default_qdrant_collection(),
            vector_dimension: default_vector_dimension(),
        }
    }
}

impl Default for WizardAgentConfig {
    fn default() -> Self {
        Self {
            max_history_messages: default_max_history_messages(),
            max_context_tokens: default_max_context_tokens(),
            system_prompt: default_system_prompt(),
            enable_retrieval: true,
            enable_memory: true,
            stream_responses: true,
            enable_thinking: false,
            retrieval_top_k: default_retrieval_top_k(),
        }
    }
}

impl WizardConfig {
    /// 转换为 TianyanConfig。
    pub fn to_tianyan_config(&self) -> TianyanConfig {
        TianyanConfig {
            agent: self.to_agent_config(),
            storage: self.to_storage_config(),
            models: self.to_models_config(),
            logging: LoggingConfig::default(),
            security: SecurityConfig::default(),
            memory: MemoryConfig::default(),
            retrieval: RetrievalConfig::default(),
            summary_service: SummaryServiceConfig::default(),
        }
    }

    fn to_agent_config(&self) -> AgentConfig {
        AgentConfig {
            enable_skills: true,
            enable_memory: self.agent.enable_memory,
            stream_responses: self.agent.stream_responses,
            enable_thinking: self.agent.enable_thinking,
            default_top_k: self.agent.retrieval_top_k,
            enable_verification: false,
            learned_rules_top_k: 5,
            learned_rules_max_tokens: 800,
        }
    }

    fn to_storage_config(&self) -> StorageConfig {
        StorageConfig {
            data_dir: self.storage.data_dir.clone(),
            max_storage_size: 0,
            auto_cleanup: true,
            cleanup_days: 365,
            vector: VectorStorageConfig {
                url: self.storage.vector.url.clone(),
                collection_name: self.storage.vector.collection_name.clone(),
                vector_dimension: self.storage.vector.vector_dimension,
            },
        }
    }

    fn to_models_config(&self) -> ModelsConfig {
        ModelsConfig {
            default_chat_model: self.models.default_chat_model.clone(),
            default_embedding_model: self.models.default_embedding_model.clone(),
            default_vision_model: self.models.default_vision_model.clone(),
            services: self
                .models
                .services
                .iter()
                .map(|s| ModelServiceConfig {
                    name: s.name.clone(),
                    service_type: s.service_type,
                    endpoint: s.endpoint.clone(),
                    api_key: if s.api_key.is_empty() {
                        None
                    } else {
                        Some(s.api_key.clone())
                    },
                    default_model: s.default_model.clone(),
                    models: s.models.clone(),
                    timeout: s.timeout,
                    enabled: s.enabled,
                    priority: s.priority,
                    headers: std::collections::HashMap::new(),
                })
                .collect(),
        }
    }
}

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
    fn test_wizard_config_default() {
        let config = WizardConfig::default();
        assert!(!config.agent.system_prompt.is_empty());
        assert_eq!(config.agent.max_history_messages, 10);
        assert_eq!(config.agent.max_context_tokens, 8000);
    }

    #[test]
    fn test_wizard_to_tianyan_config() {
        let wizard = WizardConfig {
            models: WizardModelsConfig {
                services: vec![WizardModelService {
                    name: "test".to_string(),
                    service_type: ModelServiceType::OpenAI,
                    endpoint: "https://api.example.com".to_string(),
                    api_key: "sk-test".to_string(),
                    default_model: "gpt-4".to_string(),
                    models: vec!["gpt-4".to_string()],
                    timeout: 60,
                    enabled: true,
                    priority: 0,
                }],
                default_chat_model: "gpt-4".to_string(),
                default_embedding_model: "".to_string(),
                default_vision_model: "".to_string(),
            },
            storage: WizardStorageConfig::default(),
            agent: WizardAgentConfig::default(),
        };

        let tianyan = wizard.to_tianyan_config();
        assert_eq!(tianyan.models.default_chat_model, "gpt-4");
        assert_eq!(tianyan.models.services.len(), 1);
        assert_eq!(tianyan.models.services[0].name, "test");
    }

    #[test]
    fn test_config_status() {
        let status = ConfigStatus::configured(PathBuf::from("/test/config.toml"));
        assert!(status.configured);
        assert!(status.config_path.is_some());
        assert!(status.errors.is_empty());

        let status = ConfigStatus::not_configured(vec!["未找到配置文件".to_string()]);
        assert!(!status.configured);
        assert!(status.config_path.is_none());
        assert_eq!(status.errors.len(), 1);
    }

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
