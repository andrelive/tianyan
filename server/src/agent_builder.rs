//! Agent Builder Module
//!
//! 提供 Agent 构建工厂，负责 Agent 实例的创建和配置验证。
//! 将构建逻辑与 AppState 的状态管理职责分离。

// 标准库
use std::sync::Arc;

// 外部 crate
use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};

// 内部 crate
use tianyan::agent::{
    Agent, AgentBuilder, AgentCoordinator, AgentResponse, AgentState, AgentStreamChunk,
};
use tianyan::config::{ModelServiceType, TianyanConfig};
use tianyan::context::DualLayerRetriever;
use tianyan::model::types::ModelProvider;
use tianyan::model::{ModelConfig, ModelServices};
use tianyan::session::PersistentSessionManager;
use tianyan::skills::{SkillExecutor, SkillRegistry};
use tianyan::vfs::VirtualFileSystemImpl;
use tianyan::{Result as TianyanResult, TianyanError};

/// 根据配置创建模型服务。
///
/// 将模型服务配置转换为可直接使用的 `ModelServices`。
pub async fn create_model_services(config: &TianyanConfig) -> TianyanResult<ModelServices> {
    let model_configs: Vec<ModelConfig> = config
        .models
        .services
        .iter()
        .filter(|s| s.enabled)
        .map(|service_config| {
            let provider = match service_config.service_type {
                ModelServiceType::OpenAI => ModelProvider::OpenAI,
                ModelServiceType::Custom => ModelProvider::OpenAICompatible,
            };

            let api_key = service_config.api_key.clone().unwrap_or_default();

            ModelConfig::new(provider, &api_key)
                .with_name(&service_config.name)
                .with_base_url(&service_config.endpoint)
                .with_chat_model(&service_config.default_model)
                .with_embedding_model(&config.models.default_embedding_model)
                .with_timeout(service_config.timeout)
                .with_priority(service_config.priority)
        })
        .collect();

    ModelServices::from_configs(model_configs).await
}

/// Agent 构建工厂
///
/// 提供静态方法用于构建和验证 Agent 实例。
/// 所有方法均为静态方法，无需实例化。
pub struct AgentBuilderFactory;

impl AgentBuilderFactory {
    /// 构建 Agent 实例
    pub async fn build_agent(
        config: &TianyanConfig,
        vfs: Arc<VirtualFileSystemImpl>,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        skill_executor: Arc<SkillExecutor>,
    ) -> TianyanResult<Agent> {
        Self::validate_config(config)?;

        let model_services = create_model_services(config).await?;

        let retriever = DualLayerRetriever::new(vfs.clone());

        let agent = AgentBuilder::new()
            .with_config(config.agent.clone())
            .with_model_service(model_services.chat)
            .with_vfs(vfs.clone())
            .with_retriever(Arc::new(retriever))
            .with_skill_executor(skill_executor)
            .with_skill_registry(skill_registry)
            .with_session_manager(Arc::new(PersistentSessionManager::new(vfs)))
            .build()
            .map_err(|e| TianyanError::Internal(format!("Agent 构建失败：{}", e)))?;

        agent
            .initialize()
            .await
            .map_err(|e| TianyanError::Internal(format!("Agent 初始化失败：{}", e)))?;

        tracing::info!("Agent 构建并初始化成功");
        Ok(agent)
    }

    /// 构建 Agent 或降级为 WizardMode
    pub async fn build_agent_or_wizard(
        config: &TianyanConfig,
        vfs: Arc<VirtualFileSystemImpl>,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        skill_executor: Arc<SkillExecutor>,
    ) -> TianyanResult<Arc<dyn AgentCoordinator>> {
        match Self::build_agent(config, vfs, skill_registry, skill_executor).await {
            Ok(agent) => Ok(Arc::new(agent)),
            Err(e) => {
                tracing::warn!("Agent 构建失败 ({}), 使用向导模式", e);
                Ok(Arc::new(WizardModeAgent))
            }
        }
    }

    /// 验证配置有效性
    pub fn validate_config(config: &TianyanConfig) -> TianyanResult<()> {
        let enabled_services: Vec<_> = config
            .models
            .services
            .iter()
            .filter(|s| s.enabled)
            .collect();

        if enabled_services.is_empty() {
            return Err(TianyanError::Config(
                "没有启用的模型服务，请先完成配置".to_string(),
            ));
        }

        for service in &enabled_services {
            if service.api_key.is_none()
                || service
                    .api_key
                    .as_ref()
                    .map(|k| k.is_empty())
                    .unwrap_or(true)
            {
                return Err(TianyanError::Config(format!(
                    "模型服务 '{}' 未配置 API Key，请先完成配置",
                    service.name
                )));
            }

            if service.endpoint.is_empty() {
                return Err(TianyanError::Config(format!(
                    "模型服务 '{}' 未配置 API 端点，请先完成配置",
                    service.name
                )));
            }

            if service.default_model.is_empty() {
                return Err(TianyanError::Config(format!(
                    "模型服务 '{}' 未配置默认模型，请先完成配置",
                    service.name
                )));
            }
        }

        if config.models.default_chat_model.is_empty() {
            return Err(TianyanError::Config(
                "未配置默认聊天模型，请先完成配置".to_string(),
            ));
        }

        tracing::info!("配置验证通过");
        Ok(())
    }
}

/// 向导模式下的占位 Agent
///
/// 当配置无效时使用，聊天功能会返回错误
pub struct WizardModeAgent;

#[async_trait]
impl AgentCoordinator for WizardModeAgent {
    async fn process_message(
        &self,
        _session_id: &str,
        _message: &str,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::ModelService(
            "应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn process_message_stream(
        &self,
        _session_id: &str,
        _message: &str,
    ) -> TianyanResult<mpsc::Receiver<TianyanResult<AgentStreamChunk>>> {
        Err(TianyanError::ModelService(
            "应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn handle_clarification(
        &self,
        _session_id: &str,
        _answers: &str,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::ModelService(
            "应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn initialize(&self) -> TianyanResult<()> {
        Ok(())
    }

    async fn get_state(&self) -> AgentState {
        AgentState::default()
    }

    async fn shutdown(&self) -> TianyanResult<()> {
        Ok(())
    }
}
