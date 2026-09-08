//! Agent Builder Module
//!
//! 提供 Agent 构建工厂，负责 Agent 实例的创建和配置验证。

// 标准库
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// 外部 crate
use async_trait::async_trait;
use tokio::sync::mpsc;

// 内部 crate
use tianyan::agent::DynamicToolExecutor;
use tianyan::agent::{
    Agent, AgentBuilder, AgentCoordinator, AgentResponse, AgentState, AgentStreamChunk,
};
use tianyan::config::{ModelCapability, TianyanConfig};
use tianyan::context::DualLayerRetriever;
use tianyan::db::Database;
use tianyan::model::ModelServices;
use tianyan::observability::execution_log::ExecutionLog;
use tianyan::observability::usage_log::UsageLog;
use tianyan::observability::usage_stats::UsageStats;
use tianyan::session::SessionManager;
use tianyan::skills::{SkillExecutor, SkillRefresher};
use tianyan::vfs::VirtualFileSystemImpl;
use tianyan::{Result as TianyanResult, TianyanError};

/// 子智能体消息流事件广播（ADR-026：core 事件通道 → SSE 订阅者分发）。
///
/// 事件为 ChatStreamEvent 同构 JSON，附加 task_id 归集到面板对应任务；
/// 无订阅者时静默丢弃（broadcast 语义）。
pub struct TaskEventBroadcaster {
    /// 广播通道（GET /tasks/stream 订阅）。
    pub tx: tokio::sync::broadcast::Sender<String>,
}

#[async_trait]
impl tianyan::agent::background::TaskEventSink for TaskEventBroadcaster {
    async fn emit(&self, task_id: &str, event: serde_json::Value) {
        let mut payload = event;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("task_id".to_string(), serde_json::json!(task_id));
        }
        if let Ok(json) = serde_json::to_string(&payload) {
            let _ = self.tx.send(json);
        }
    }
}

use crate::state::{build_knowledge_ingestor, resolve_chat_model, resolve_chat_model_spec};

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
    /// `sqlite_db` 为全系统共享数据库（后台任务持久化 + 唤醒，ADR-013；
    /// None 时任务状态纯内存、无唤醒）。
    /// `notification_sink` 为系统通知通道（后台任务完成/审批挂起；动态包装，
    /// Tauri 后注册亦生效）。
    /// `session_manager` / `trace_collector` 由调用方（AppState）创建并持有——
    /// 组合根收敛：API 层与 Agent 共享同一实例，避免双写/双收集。
    #[allow(clippy::too_many_arguments)]
    pub async fn build_agent(
        config: &TianyanConfig,
        model_services: ModelServices,
        vfs: Arc<VirtualFileSystemImpl>,
        skill_executor: Arc<SkillExecutor>,
        usage_stats: Arc<UsageStats>,
        snapshot_manager: Option<Arc<tianyan::snapshot::SnapshotManager>>,
        skill_refresher: Arc<dyn SkillRefresher>,
        dynamic_tools: Vec<Arc<dyn DynamicToolExecutor>>,
        sqlite_db: Option<Arc<Database>>,
        notification_sink: tianyan::notification::SharedNotificationSink,
        session_manager: Arc<dyn SessionManager>,
        trace_collector: Option<Arc<tianyan::observability::trace::TraceCollector>>,
        execution_log: Arc<ExecutionLog>,
        session_recall: Arc<tianyan::session::search::SessionRecall>,
        session_store: Arc<tianyan::session::store::SessionStore>,
        role_registry: Arc<tianyan::agent::RoleRegistry>,
        role_router: Arc<tianyan::agent::RoleRouter>,
        usage_log: Arc<UsageLog>,
        user_questions: Arc<tianyan::agent::user_questions::UserQuestionService>,
        task_event_sink: Arc<dyn tianyan::agent::background::TaskEventSink>,
        event_tx: tokio::sync::broadcast::Sender<String>,
    ) -> TianyanResult<Arc<Agent>> {
        Self::validate_config(config)?;

        let retriever = DualLayerRetriever::new(vfs.clone()).with_usage_stats(usage_stats.clone());

        // 模型名解析与 KnowledgeIngestor 构造收敛于 state.rs 唯一入口
        // （resolve_chat_model / resolve_chat_model_spec / build_knowledge_ingestor）
        let chat_model = resolve_chat_model(config);
        let chat_model_spec = resolve_chat_model_spec(config);

        // 用量日志的 provider 维度：从配置构建 model → provider 映射
        // （模型名可能不含 provider 前缀，如 deepseek-v4-flash → opencode）。
        let mut provider_by_model = std::collections::HashMap::new();
        for provider in &config.models.providers {
            for model in &provider.models {
                provider_by_model.insert(model.name.clone(), provider.name.clone());
            }
        }
        let knowledge_ingestor = build_knowledge_ingestor(
            config,
            &model_services,
            vfs.clone() as Arc<dyn tianyan::vfs::VirtualFileSystem>,
        );

        let agent = AgentBuilder::new()
            .with_config(config.agent.clone())
            .with_model(&chat_model)
            .with_chat_model_spec(chat_model_spec)
            .with_model_service(model_services.chat)
            .with_role_registry(role_registry)
            .with_role_router(role_router)
            .with_vfs(vfs.clone())
            .with_retriever(Arc::new(retriever))
            .with_skill_executor(skill_executor)
            .with_knowledge_ingestor(Arc::new(knowledge_ingestor))
            .with_security_config(config.security.clone())
            .with_web_config(config.web.clone())
            .with_usage_stats(usage_stats)
            .with_default_working_directory(
                config.agent.working_directory.clone().map(PathBuf::from),
            )
            .with_command_logs_dir(config.storage.data_dir.join("command_logs"))
            .with_session_manager(session_manager)
            .with_execution_log(execution_log)
            .with_session_recall(session_recall)
            .with_session_store(session_store)
            .with_usage_log(usage_log)
            .with_provider_by_model(provider_by_model)
            .with_user_questions(user_questions);
        let agent = match snapshot_manager {
            Some(sm) => agent.with_snapshot_manager(sm),
            None => agent,
        };
        let mut builder = agent.with_skill_refresher(skill_refresher);
        if let Some(trace) = trace_collector {
            // G6：结构化 Trace（与 AppState 共享同一收集器，单一写入路径）
            builder = builder.with_trace_collector(trace);
        }
        if let Some(db) = sqlite_db {
            builder = builder.with_background_task_db(db);
        }
        // 系统通知通道（动态包装：Tauri 后注册亦生效）
        builder = builder.with_notification_sink(notification_sink);
        // 子智能体消息流事件通道（ADR-026：面板实时流式显示）
        builder = builder.with_task_event_sink(task_event_sink);
        let agent = builder
            .build()
            .map_err(|e| TianyanError::Custom(format!("内部错误：Agent 构建失败：{}", e)))?;

        // 注入动态工具（如 MCP 工具桥接）——失败仅告警，不阻塞 Agent 启动
        agent.register_dynamic_tools(dynamic_tools).await;

        // ADR-013：注册任务唤醒器（Weak 自引用转发器 → Agent::process_wake）。
        // 后台任务全部完成/失败时自动触发主 agent 新一轮生成。
        // ADR-031：唤醒轮流式化——转发器把唤醒轮的流式事件（AgentStreamChunk）
        // 转成 ChatStreamEvent JSON（type=chat_stream）推入统一事件通道，
        // 前端打字机看到汇总输出（不再依赖落库广播）。
        let agent_arc = Arc::new(agent);
        let forward = {
            let event_tx = event_tx.clone();
            Arc::new(
                move |session_id: &str,
                      mut rx: mpsc::Receiver<
                    Result<tianyan::agent::AgentStreamChunk, tianyan::TianyanError>,
                >| {
                    let event_tx = event_tx.clone();
                    let session_id = session_id.to_string();
                    tokio::spawn(async move {
                        while let Some(chunk_result) = rx.recv().await {
                            let Ok(chunk) = chunk_result else { continue };
                            let event = crate::api::chat::services::map_chunk_to_event(
                                chunk,
                                "wake",
                                &session_id,
                                0,
                            );
                            if let Ok(mut payload) = serde_json::to_value(&event) {
                                payload["type"] = serde_json::json!("chat_stream");
                                if let Ok(json) = serde_json::to_string(&payload) {
                                    let _ = event_tx.send(json);
                                }
                            }
                        }
                    });
                },
            )
        };
        agent_arc
            .register_task_waker(Arc::new(tianyan::agent::AgentWakeForwarder::new(
                &agent_arc, forward,
            )))
            .await;

        agent_arc
            .initialize()
            .await
            .map_err(|e| TianyanError::Custom(format!("内部错误：Agent 初始化失败：{}", e)))?;

        tracing::info!("Agent 构建并初始化成功");
        Ok(agent_arc)
    }

    /// 构建 Agent 或降级为 WizardMode
    #[allow(clippy::too_many_arguments)]
    pub async fn build_agent_or_wizard(
        config: &TianyanConfig,
        model_services: ModelServices,
        vfs: Arc<VirtualFileSystemImpl>,
        skill_executor: Arc<SkillExecutor>,
        usage_stats: Arc<UsageStats>,
        snapshot_manager: Option<Arc<tianyan::snapshot::SnapshotManager>>,
        skill_refresher: Arc<dyn SkillRefresher>,
        dynamic_tools: Vec<Arc<dyn DynamicToolExecutor>>,
        sqlite_db: Option<Arc<Database>>,
        notification_sink: tianyan::notification::SharedNotificationSink,
        session_manager: Arc<dyn SessionManager>,
        trace_collector: Option<Arc<tianyan::observability::trace::TraceCollector>>,
        execution_log: Arc<ExecutionLog>,
        session_recall: Arc<tianyan::session::search::SessionRecall>,
        session_store: Arc<tianyan::session::store::SessionStore>,
        role_registry: Arc<tianyan::agent::RoleRegistry>,
        role_router: Arc<tianyan::agent::RoleRouter>,
        usage_log: Arc<UsageLog>,
        user_questions: Arc<tianyan::agent::user_questions::UserQuestionService>,
        task_event_sink: Arc<dyn tianyan::agent::background::TaskEventSink>,
        event_tx: tokio::sync::broadcast::Sender<String>,
    ) -> TianyanResult<Arc<dyn AgentCoordinator>> {
        match Self::build_agent(
            config,
            model_services,
            vfs,
            skill_executor,
            usage_stats,
            snapshot_manager,
            skill_refresher,
            dynamic_tools,
            sqlite_db,
            notification_sink,
            session_manager,
            trace_collector,
            execution_log,
            session_recall,
            session_store,
            role_registry,
            role_router,
            usage_log,
            user_questions,
            task_event_sink,
            event_tx,
        )
        .await
        {
            Ok(agent) => Ok(agent),
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
            return Err(TianyanError::config("没有启用的模型提供商，请先完成配置"));
        }

        for provider in &enabled_providers {
            if provider.api_key.is_none()
                || provider
                    .api_key
                    .as_ref()
                    .map(|k| k.is_empty())
                    .unwrap_or(true)
            {
                return Err(TianyanError::config(format!(
                    "模型提供商 '{}' 未配置 API Key，请先完成配置",
                    provider.name
                )));
            }

            if provider.endpoint.is_empty() {
                return Err(TianyanError::config(format!(
                    "模型提供商 '{}' 未配置 API 端点，请先完成配置",
                    provider.name
                )));
            }

            if provider.models.is_empty() {
                return Err(TianyanError::config(format!(
                    "模型提供商 '{}' 未配置任何模型，请先完成配置",
                    provider.name
                )));
            }
        }

        // 确保至少有一个可用聊天模型
        if config.models.resolve(ModelCapability::Chat).is_none() {
            return Err(TianyanError::config(
                "未找到可用的聊天模型（需要 chat 能力标签），请先完成配置",
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
        _message: &tianyan::Message,
        _model: Option<&str>,
        _thinking_effort: Option<String>,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::Custom(
            "模型服务错误：应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn process_message_stream(
        &self,
        _session_id: &str,
        _message: &tianyan::Message,
        _model: Option<&str>,
        _cancel: Option<Arc<AtomicBool>>,
        _thinking_effort: Option<String>,
        _user_message_id: Option<&str>,
    ) -> TianyanResult<mpsc::Receiver<TianyanResult<AgentStreamChunk>>> {
        Err(TianyanError::Custom(
            "模型服务错误：应用未配置。请先完成配置向导。".to_string(),
        ))
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

    async fn respond_approval(
        &self,
        _request_id: &str,
        _decision: tianyan::executor::approval::ApprovalDecision,
        _reason: Option<String>,
        _edited_command: Option<String>,
    ) -> TianyanResult<()> {
        Err(TianyanError::Custom(
            "模型服务错误：应用未配置。请先完成配置向导。".to_string(),
        ))
    }

    async fn get_state(&self) -> AgentState {
        AgentState::default()
    }

    async fn background_tasks(&self) -> Vec<tianyan::agent::background::BackgroundTask> {
        Vec::new()
    }

    async fn cancel_background_task(&self, _task_id: &str) -> TianyanResult<bool> {
        // 向导模式未装配 Agent，无后台任务
        Ok(false)
    }

    async fn compress_session(
        &self,
        _session_id: &str,
    ) -> TianyanResult<Option<tianyan::common::types::StructuredMessage>> {
        // 向导模式未装配 Agent，压缩不可用
        Ok(None)
    }
}
