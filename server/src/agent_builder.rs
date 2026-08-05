//! Agent Builder Module
//!
//! 提供 Agent 构建工厂，负责 Agent 实例的创建和配置验证。

// 标准库
use std::sync::Arc;

// 外部 crate
use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};

// 内部 crate
use tianyan::agent::DynamicToolExecutor;
use tianyan::agent::{
    Agent, AgentBuilder, AgentCoordinator, AgentResponse, AgentState, AgentStreamChunk,
};
use tianyan::config::{ModelCapability, TianyanConfig};
use tianyan::context::DualLayerRetriever;
use tianyan::knowledge::{IngestorConfig, KnowledgeIngestor};
use tianyan::model::ModelServices;
use tianyan::observability::usage_stats::UsageStats;
use tianyan::session::PersistentSessionManager;
use tianyan::skills::{SkillExecutor, SkillRegistry};
use tianyan::vfs::VirtualFileSystemImpl;
use tianyan::{Result as TianyanResult, TianyanError};

/// 根据配置创建模型服务。
pub async fn create_model_services(config: &TianyanConfig) -> TianyanResult<ModelServices> {
    ModelServices::from_config(&config.models).await
}

/// Agent 构建工厂
///
/// 提供静态方法用于构建和验证 Agent 实例。
/// 所有方法均为静态方法，无需实例化。
pub struct AgentBuilderFactory;

impl AgentBuilderFactory {
    /// 构建 Agent 实例
    ///
    /// `model_services` 由调用方（AppState）创建并持有，配置热更新时重建，
    /// 避免每次构建 Agent 时重复创建 HTTP 客户端。
    ///
    /// `dynamic_tools` 为外部注册的扩展工具（如 MCP 工具桥接），构建后注入 Agent。
    #[allow(clippy::too_many_arguments)]
    pub async fn build_agent(
        config: &TianyanConfig,
        model_services: ModelServices,
        vfs: Arc<VirtualFileSystemImpl>,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        skill_executor: Arc<SkillExecutor>,
        usage_stats: Arc<UsageStats>,
        snapshot_manager: Option<Arc<tianyan::snapshot::SnapshotManager>>,
        dynamic_tools: Vec<Arc<dyn DynamicToolExecutor>>,
    ) -> TianyanResult<Agent> {
        Self::validate_config(config)?;

        let retriever = DualLayerRetriever::new(vfs.clone()).with_usage_stats(usage_stats.clone());

        // 从配置中解析各能力模型名称
        let chat_model = config
            .models
            .resolve(ModelCapability::Chat)
            .map(|r| r.model)
            .unwrap_or_default();

        let embedding_model = config
            .models
            .resolve(ModelCapability::TextEmbedding)
            .or_else(|| config.models.resolve(ModelCapability::MultimodalEmbedding))
            .map(|r| r.model)
            .unwrap_or_default();

        let vision_model = config
            .models
            .resolve(ModelCapability::Vision)
            .map(|r| r.model)
            .unwrap_or_default();

        // Build KnowledgeIngestor so the knowledge_ingest agent tool works.
        let knowledge_ingestor = KnowledgeIngestor::new(
            IngestorConfig::new()
                .with_embedding_model(&embedding_model)
                .with_summary_model(&chat_model)
                .with_vision_model(&vision_model),
            model_services.chat.clone(),
            model_services.embedding.clone(),
            model_services.vision.clone(),
            vfs.clone(),
        );

        let agent = AgentBuilder::new()
            .with_config(config.agent.clone())
            .with_model(&chat_model)
            .with_model_service(model_services.chat)
            .with_vfs(vfs.clone())
            .with_retriever(Arc::new(retriever))
            .with_skill_executor(skill_executor)
            .with_skill_registry(skill_registry)
            .with_knowledge_ingestor(Arc::new(knowledge_ingestor))
            .with_security_config(config.security.clone())
            .with_usage_stats(usage_stats)
            .with_session_manager(Arc::new(PersistentSessionManager::new(vfs)));
        let agent = match snapshot_manager {
            Some(sm) => agent.with_snapshot_manager(sm),
            None => agent,
        };
        let agent = agent
            .build()
            .map_err(|e| TianyanError::Custom(format!("内部错误：Agent 构建失败：{}", e)))?;

        // 注入动态工具（如 MCP 工具桥接）——失败仅告警，不阻塞 Agent 启动
        agent.register_dynamic_tools(dynamic_tools).await;

        agent
            .initialize()
            .await
            .map_err(|e| TianyanError::Custom(format!("内部错误：Agent 初始化失败：{}", e)))?;

        tracing::info!("Agent 构建并初始化成功");
        Ok(agent)
    }

    /// 构建 Agent 或降级为 WizardMode
    #[allow(clippy::too_many_arguments)]
    pub async fn build_agent_or_wizard(
        config: &TianyanConfig,
        model_services: ModelServices,
        vfs: Arc<VirtualFileSystemImpl>,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        skill_executor: Arc<SkillExecutor>,
        usage_stats: Arc<UsageStats>,
        snapshot_manager: Option<Arc<tianyan::snapshot::SnapshotManager>>,
        dynamic_tools: Vec<Arc<dyn DynamicToolExecutor>>,
    ) -> TianyanResult<Arc<dyn AgentCoordinator>> {
        match Self::build_agent(
            config,
            model_services,
            vfs,
            skill_registry,
            skill_executor,
            usage_stats,
            snapshot_manager,
            dynamic_tools,
        )
        .await
        {
            Ok(agent) => Ok(Arc::new(agent)),
            Err(e) => {
                tracing::warn!("Agent 构建失败 ({}), 使用向导模式", e);
                Ok(Arc::new(WizardModeAgent))
            }
        }
    }

    /// 验证配置有效性
    pub fn validate_config(config: &TianyanConfig) -> TianyanResult<()> {
        let enabled_providers: Vec<_> = config
            .models
            .providers
            .iter()
            .filter(|p| p.enabled)
            .collect();

        if enabled_providers.is_empty() {
            return Err(TianyanError::Custom(
                "配置错误：没有启用的模型提供商，请先完成配置".to_string(),
            ));
        }

        for provider in &enabled_providers {
            if provider.api_key.is_none()
                || provider
                    .api_key
                    .as_ref()
                    .map(|k| k.is_empty())
                    .unwrap_or(true)
            {
                return Err(TianyanError::Custom(format!(
                    "配置错误：模型提供商 '{}' 未配置 API Key，请先完成配置",
                    provider.name
                )));
            }

            if provider.endpoint.is_empty() {
                return Err(TianyanError::Custom(format!(
                    "配置错误：模型提供商 '{}' 未配置 API 端点，请先完成配置",
                    provider.name
                )));
            }

            if provider.models.is_empty() {
                return Err(TianyanError::Custom(format!(
                    "配置错误：模型提供商 '{}' 未配置任何模型，请先完成配置",
                    provider.name
                )));
            }
        }

        // 确保至少有一个可用聊天模型
        if config.models.resolve(ModelCapability::Chat).is_none() {
            return Err(TianyanError::Custom(
                "配置错误：未找到可用的聊天模型（需要 chat 能力标签），请先完成配置".to_string(),
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
        _model: Option<&str>,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::Custom(
            "模型服务错误：应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn process_message_stream(
        &self,
        _session_id: &str,
        _message: &str,
        _model: Option<&str>,
    ) -> TianyanResult<mpsc::Receiver<TianyanResult<AgentStreamChunk>>> {
        Err(TianyanError::Custom(
            "模型服务错误：应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn handle_clarification(
        &self,
        _session_id: &str,
        _answers: &str,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::Custom(
            "模型服务错误：应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn initialize(&self) -> TianyanResult<()> {
        Ok(())
    }

    async fn approval_status(
        &self,
    ) -> TianyanResult<tianyan::executor::approval::ApprovalStatusSnapshot> {
        // 向导模式未装配审批工作流：返回默认配置与空队列
        Ok(tianyan::executor::approval::ApprovalStatusSnapshot {
            config: tianyan::executor::approval::ApprovalWorkflowConfig::default(),
            pending_approvals: Vec::new(),
            pending_confirmations: Vec::new(),
            recent_records: Vec::new(),
            confirmed_action_count: 0,
        })
    }

    async fn get_state(&self) -> AgentState {
        AgentState::default()
    }

    async fn shutdown(&self) -> TianyanResult<()> {
        Ok(())
    }
}
