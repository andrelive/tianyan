//! 配置向导类型定义

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// 向导步骤
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WizardStep {
    Welcome,
    ModelConfig,
    DataConfig,
    AgentConfig,
    SystemPrompt,
    Confirm,
}

impl WizardStep {
    /// 获取步骤标题
    pub fn title(&self) -> &'static str {
        match self {
            WizardStep::Welcome => "欢迎",
            WizardStep::ModelConfig => "模型服务配置",
            WizardStep::DataConfig => "数据目录配置",
            WizardStep::AgentConfig => "智能体参数配置",
            WizardStep::SystemPrompt => "系统提示词",
            WizardStep::Confirm => "确认和保存",
        }
    }

    /// 获取步骤序号（从1开始）
    pub fn number(&self) -> usize {
        match self {
            WizardStep::Welcome => 1,
            WizardStep::ModelConfig => 2,
            WizardStep::DataConfig => 3,
            WizardStep::AgentConfig => 4,
            WizardStep::SystemPrompt => 5,
            WizardStep::Confirm => 6,
        }
    }

    /// 获取总步骤数
    pub fn total() -> usize {
        6
    }

    /// 获取下一步
    pub fn next(&self) -> Option<WizardStep> {
        match self {
            WizardStep::Welcome => Some(WizardStep::ModelConfig),
            WizardStep::ModelConfig => Some(WizardStep::DataConfig),
            WizardStep::DataConfig => Some(WizardStep::AgentConfig),
            WizardStep::AgentConfig => Some(WizardStep::SystemPrompt),
            WizardStep::SystemPrompt => Some(WizardStep::Confirm),
            WizardStep::Confirm => None,
        }
    }

    /// 获取上一步
    pub fn previous(&self) -> Option<WizardStep> {
        match self {
            WizardStep::Welcome => None,
            WizardStep::ModelConfig => Some(WizardStep::Welcome),
            WizardStep::DataConfig => Some(WizardStep::ModelConfig),
            WizardStep::AgentConfig => Some(WizardStep::DataConfig),
            WizardStep::SystemPrompt => Some(WizardStep::AgentConfig),
            WizardStep::Confirm => Some(WizardStep::SystemPrompt),
        }
    }
}

/// 模型服务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ModelServiceType {
    #[serde(rename = "openai")]
    #[default]
    OpenAI,
    #[serde(rename = "custom")]
    Custom,
}

/// 向导配置状态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WizardState {
    /// 当前步骤
    pub current_step: WizardStep,
    /// 模型服务配置
    pub model_services: Vec<ModelServiceConfig>,
    /// 默认聊天模型
    pub default_chat_model: String,
    /// 默认嵌入模型
    pub default_embedding_model: String,
    /// 默认视觉模型
    pub default_vision_model: String,
    /// 数据目录
    pub data_dir: String,
    /// 向量数据库 URL
    pub vector_url: String,
    /// 集合名称
    pub collection_name: String,
    /// 向量维度
    pub vector_dimension: usize,
    /// 最大历史消息数
    pub max_history_messages: usize,
    /// 最大上下文 Token 数
    pub max_context_tokens: usize,
    /// 系统提示词
    pub system_prompt: String,
    /// 是否启用检索
    pub enable_retrieval: bool,
    /// 是否启用记忆
    pub enable_memory: bool,
    /// 是否流式输出
    pub stream_responses: bool,
    /// 是否启用思考模式
    pub enable_thinking: bool,
    /// 检索 top-k
    pub retrieval_top_k: usize,
    /// 是否正在保存
    pub is_saving: bool,
    /// 保存错误信息
    pub save_error: Option<String>,
    /// 是否保存成功
    pub save_success: bool,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            current_step: WizardStep::Welcome,
            model_services: vec![ModelServiceConfig::default()],
            default_chat_model: String::new(),
            default_embedding_model: String::new(),
            default_vision_model: String::new(),
            data_dir: String::new(),
            vector_url: "http://localhost:6334".to_string(),
            collection_name: "tianyan_contexts".to_string(),
            vector_dimension: 1536,
            max_history_messages: 10,
            max_context_tokens: 8000,
            system_prompt: default_system_prompt(),
            enable_retrieval: true,
            enable_memory: true,
            stream_responses: true,
            enable_thinking: false,
            retrieval_top_k: 10,
            is_saving: false,
            save_error: None,
            save_success: false,
        }
    }
}

/// 模型服务配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelServiceConfig {
    /// 服务名称
    pub name: String,
    /// 服务类型
    pub service_type: ModelServiceType,
    /// API 端点 URL
    pub endpoint: String,
    /// API 密钥
    pub api_key: String,
    /// 默认模型
    pub default_model: String,
    /// 可用模型列表
    pub models: Vec<String>,
    /// 超时时间（秒）
    pub timeout: u64,
    /// 是否启用
    pub enabled: bool,
    /// 优先级
    pub priority: u32,
    /// 是否显示高级选项
    pub show_advanced: bool,
    /// 连接测试状态
    pub test_status: TestStatus,
}

impl Default for ModelServiceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            service_type: ModelServiceType::OpenAI,
            endpoint: String::new(),
            api_key: String::new(),
            default_model: String::new(),
            models: Vec::new(),
            timeout: 60,
            enabled: true,
            priority: 0,
            show_advanced: false,
            test_status: TestStatus::NotTested,
        }
    }
}

/// 连接测试状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    NotTested,
    Testing,
    Success,
    Failed,
}

impl TestStatus {
    /// 获取状态文本
    pub fn text(&self) -> &'static str {
        match self {
            TestStatus::NotTested => "未测试",
            TestStatus::Testing => "测试中...",
            TestStatus::Success => "连接成功",
            TestStatus::Failed => "连接失败",
        }
    }
}

/// 配置状态响应
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigStatusResponse {
    pub configured: bool,
    pub config_path: Option<String>,
    pub errors: Vec<String>,
    pub default_system_prompt: Option<String>,
}

/// 配置保存请求
#[derive(Debug, Clone, Serialize)]
pub struct SaveConfigRequest {
    pub config: WizardConfigData,
}

/// 向导配置数据
#[derive(Debug, Clone, Serialize)]
pub struct WizardConfigData {
    pub models: ModelsConfigData,
    pub storage: StorageConfigData,
    pub agent: AgentConfigData,
}

/// 模型配置数据
#[derive(Debug, Clone, Serialize)]
pub struct ModelsConfigData {
    pub services: Vec<ModelServiceData>,
    pub default_chat_model: String,
    pub default_embedding_model: String,
    pub default_vision_model: String,
}

/// 模型服务数据
#[derive(Debug, Clone, Serialize)]
pub struct ModelServiceData {
    pub name: String,
    #[serde(rename = "type")]
    pub service_type: ModelServiceType,
    pub endpoint: String,
    pub api_key: String,
    pub default_model: String,
    pub models: Vec<String>,
    pub timeout: u64,
    pub enabled: bool,
    pub priority: u32,
}

/// 存储配置数据
#[derive(Debug, Clone, Serialize)]
pub struct StorageConfigData {
    pub data_dir: String,
    pub vector: VectorStorageConfigData,
}

/// 向量存储配置数据
#[derive(Debug, Clone, Serialize)]
pub struct VectorStorageConfigData {
    pub url: String,
    pub collection_name: String,
    pub vector_dimension: usize,
}

/// 智能体配置数据
#[derive(Debug, Clone, Serialize)]
pub struct AgentConfigData {
    pub max_history_messages: usize,
    pub max_context_tokens: usize,
    pub system_prompt: String,
    pub enable_retrieval: bool,
    pub enable_memory: bool,
    pub stream_responses: bool,
    pub enable_thinking: bool,
    pub retrieval_top_k: usize,
}

/// 配置保存响应
#[derive(Debug, Clone, Deserialize)]
pub struct SaveConfigResponse {
    pub success: bool,
    pub message: String,
    pub config_path: Option<String>,
}

/// 连接测试请求
#[derive(Debug, Clone, Serialize)]
pub struct TestConnectionRequest {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

/// 连接测试响应
#[derive(Debug, Clone, Deserialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub message: String,
    pub available_models: Option<Vec<String>>,
}

/// 默认系统提示词
fn default_system_prompt() -> String {
    "你是一个智能助手，名为天演。你可以帮助用户回答各种问题，进行对话，并提供有用的信息。"
        .to_string()
}

impl WizardState {
    /// 转换为 API 请求格式
    pub fn to_save_request(&self) -> SaveConfigRequest {
        SaveConfigRequest {
            config: WizardConfigData {
                models: ModelsConfigData {
                    services: self
                        .model_services
                        .iter()
                        .map(|s| ModelServiceData {
                            name: s.name.clone(),
                            service_type: s.service_type,
                            endpoint: s.endpoint.clone(),
                            api_key: s.api_key.clone(),
                            default_model: s.default_model.clone(),
                            models: s.models.clone(),
                            timeout: s.timeout,
                            enabled: s.enabled,
                            priority: s.priority,
                        })
                        .collect(),
                    default_chat_model: self.default_chat_model.clone(),
                    default_embedding_model: self.default_embedding_model.clone(),
                    default_vision_model: self.default_vision_model.clone(),
                },
                storage: StorageConfigData {
                    data_dir: self.data_dir.clone(),
                    vector: VectorStorageConfigData {
                        url: self.vector_url.clone(),
                        collection_name: self.collection_name.clone(),
                        vector_dimension: self.vector_dimension,
                    },
                },
                agent: AgentConfigData {
                    max_history_messages: self.max_history_messages,
                    max_context_tokens: self.max_context_tokens,
                    system_prompt: self.system_prompt.clone(),
                    enable_retrieval: self.enable_retrieval,
                    enable_memory: self.enable_memory,
                    stream_responses: self.stream_responses,
                    enable_thinking: self.enable_thinking,
                    retrieval_top_k: self.retrieval_top_k,
                },
            },
        }
    }

    /// 验证当前步骤
    pub fn validate_step(&self) -> Result<(), Vec<String>> {
        match self.current_step {
            WizardStep::Welcome => Ok(()),
            WizardStep::ModelConfig => self.validate_model_config(),
            WizardStep::DataConfig => self.validate_data_config(),
            WizardStep::AgentConfig => self.validate_agent_config(),
            WizardStep::SystemPrompt => self.validate_system_prompt(),
            WizardStep::Confirm => self.validate_all(),
        }
    }

    fn validate_model_config(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.model_services.is_empty() {
            errors.push("至少需要配置一个模型服务".to_string());
        }

        for (idx, service) in self.model_services.iter().enumerate() {
            if service.name.is_empty() {
                errors.push(format!("服务 {}: 名称不能为空", idx + 1));
            }
            if service.endpoint.is_empty() {
                errors.push(format!("服务 {}: API 端点 URL 不能为空", idx + 1));
            } else if !service.endpoint.starts_with("http://")
                && !service.endpoint.starts_with("https://")
            {
                errors.push(format!("服务 {}: API 端点 URL 格式无效", idx + 1));
            }
            if service.api_key.is_empty() {
                errors.push(format!("服务 {}: API 密钥不能为空", idx + 1));
            }
            if service.default_model.is_empty() {
                errors.push(format!("服务 {}: 默认模型不能为空", idx + 1));
            }
        }

        if self.default_chat_model.is_empty() {
            errors.push("默认聊天模型不能为空".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn validate_data_config(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.data_dir.is_empty() {
            errors.push("数据目录不能为空".to_string());
        }

        if self.vector_url.is_empty() {
            errors.push("向量数据库 URL 不能为空".to_string());
        }

        if self.collection_name.is_empty() {
            errors.push("集合名称不能为空".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn validate_agent_config(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.max_history_messages == 0 {
            errors.push("最大历史消息数必须大于 0".to_string());
        }

        if self.max_context_tokens == 0 {
            errors.push("最大上下文 Token 数必须大于 0".to_string());
        }

        if self.retrieval_top_k == 0 {
            errors.push("检索 top-k 必须大于 0".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn validate_system_prompt(&self) -> Result<(), Vec<String>> {
        if self.system_prompt.is_empty() {
            Err(vec!["系统提示词不能为空".to_string()])
        } else {
            Ok(())
        }
    }

    fn validate_all(&self) -> Result<(), Vec<String>> {
        let mut all_errors = Vec::new();

        if let Err(errors) = self.validate_model_config() {
            all_errors.extend(errors);
        }

        if let Err(errors) = self.validate_data_config() {
            all_errors.extend(errors);
        }

        if let Err(errors) = self.validate_agent_config() {
            all_errors.extend(errors);
        }

        if let Err(errors) = self.validate_system_prompt() {
            all_errors.extend(errors);
        }

        if all_errors.is_empty() {
            Ok(())
        } else {
            Err(all_errors)
        }
    }
}
