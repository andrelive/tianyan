use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex as TokioMutex;

use crate::agent::r#loop::{AgentLoop, AgentLoopConfig};
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::{RoleRegistry, RoleRouter};
use crate::common::error::{Result, TianyanError};
use crate::config::AgentConfig;
use crate::config::AgentRolesConfig;
use crate::config::SecurityConfig;
use crate::context::compression::{CompressionConfig, ContextCompressor};
use crate::context::pipeline::ContextPipeline;
use crate::context::DualLayerRetriever;
use crate::executor::approval::{
    ActionPattern, ApprovalCondition, ApprovalDecision, ApprovalWorkflow, ApprovalWorkflowConfig,
    AutoApprovalRule, SessionApprovalNotifier,
};
use crate::executor::SecurityPolicy;
use crate::executor::{LlmJudge, VerificationGate};
use crate::knowledge::KnowledgeIngestor;
use crate::lsp::diagnostics::LspManager;
use crate::model::spec::ModelSpec;
use crate::model::ChatService;
use crate::observability::execution_log::ExecutionLog;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::scheduler::tasks::RuleRecorder;
use crate::session::SessionManager;
use crate::skills::{SkillExecutor, SkillRefresher};
use crate::snapshot::SnapshotManager;
use crate::vfs::VirtualFileSystem;

use super::agent_core::Agent;

/// 用于创建智能体的构建器。
pub struct AgentBuilder {
    config: AgentConfig,
    /// 默认聊天模型（用于 AgentLoop 和委托子 Agent）。
    model: Option<String>,
    model_service: Option<Arc<dyn ChatService>>,
    retriever: Option<Arc<DualLayerRetriever>>,
    vfs: Option<Arc<dyn VirtualFileSystem>>,
    skill_executor: Option<Arc<SkillExecutor>>,
    session_manager: Option<Arc<dyn SessionManager>>,
    knowledge_ingestor: Option<Arc<KnowledgeIngestor>>,
    security_config: Option<SecurityConfig>,
    usage_stats: Option<Arc<UsageStats>>,
    /// Web 工具配置（web_search / web_fetch；None 时不启用工具）。
    web_config: Option<crate::config::WebConfig>,
    /// 工作区快照管理器（配置了 working_directory 时启用）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 全局默认工作目录（[agent] working_directory 配置；会话级绑定缺省时
    /// 快照/工具操作以此为根）。
    default_working_directory: Option<PathBuf>,
    /// 技能注册表刷新钩子（压缩会话转换点时增量注册 VFS 学习技能）。
    skill_refresher: Option<Arc<dyn SkillRefresher>>,
    /// 后台任务 SQLite 持久化后端（ADR-013；None 时任务状态纯内存）。
    background_task_db: Option<crate::vfs::backend::sqlite_db::SqliteDb>,
    /// 系统通知通道（后台任务完成 / 审批挂起的桌面通知；None 时静默）。
    notification_sink: Option<crate::notification::SharedNotificationSink>,
    /// 子 Agent 角色配置（delegate_to_agent role 参数；None 时使用内置角色）。
    agent_roles: Option<AgentRolesConfig>,
    /// 注入的角色注册表（ADR-016：VFS 加载的持久化注册表；优先于 [agent_roles] 合成）。
    role_registry: Option<Arc<RoleRegistry>>,
    /// 角色向量路由器（ADR-016 P3：suggest_role 工具依赖）。
    role_router: Option<Arc<RoleRouter>>,
    /// 结构化 Trace 收集器（G6；None 时不记录 span）。
    trace_collector: Option<Arc<crate::observability::trace::TraceCollector>>,
    /// 执行记录日志（ADR-017 GEPA 数据层；None 时统计工具不可用、不持久化）。
    execution_log: Option<Arc<ExecutionLog>>,
    /// 会话回忆服务（ADR-017 决策 6：session_recall 工具依赖；None 时工具不可用）。
    session_recall: Option<Arc<crate::session::search::SessionRecall>>,
    /// 会话权威存储（ADR-018：vfs_read 读 tianyan://session/{id} 兼容层；None 时不可用）。
    session_store: Option<Arc<crate::session::store::SessionStore>>,
    usage_log: Option<Arc<crate::observability::usage_log::UsageLog>>,
    provider_by_model: std::collections::HashMap<String, String>,
    /// 聊天模型上下文规格（T5；注入 AgentLoop 并联动压缩窗口；None 时走默认窗口）。
    chat_model_spec: Option<ModelSpec>,
    /// 后台命令日志目录（execute_command(background) 日志落盘；None 时仅内存尾部）。
    command_logs_dir: Option<PathBuf>,
    /// 用户问题服务（ask_user 同步等待用户回答；None 时工具不可用）。
    user_questions: Option<Arc<crate::agent::user_questions::UserQuestionService>>,
}

impl AgentBuilder {
    /// 创建默认构建器。
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            model: None,
            model_service: None,
            retriever: None,
            vfs: None,
            skill_executor: None,
            session_manager: None,
            knowledge_ingestor: None,
            security_config: None,
            usage_stats: None,
            web_config: None,
            snapshot_manager: None,
            default_working_directory: None,
            skill_refresher: None,
            background_task_db: None,
            notification_sink: None,
            agent_roles: None,
            trace_collector: None,
            execution_log: None,
            session_recall: None,
            session_store: None,
            usage_log: None,
            provider_by_model: std::collections::HashMap::new(),
            chat_model_spec: None,
            command_logs_dir: None,
            role_registry: None,
            role_router: None,
            user_questions: None,
        }
    }

    /// 设置 Agent 配置。
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// 设置默认聊天模型（用于 AgentLoop 和 delegate_to_agent）。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// 设置聊天模型服务。
    pub fn with_model_service(mut self, service: Arc<dyn ChatService>) -> Self {
        self.model_service = Some(service);
        self
    }

    /// 设置双层检索器。
    pub fn with_retriever(mut self, retriever: Arc<DualLayerRetriever>) -> Self {
        self.retriever = Some(retriever);
        self
    }

    /// 设置虚拟文件系统。
    pub fn with_vfs(mut self, vfs: Arc<dyn VirtualFileSystem>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    /// 设置技能执行器。
    pub fn with_skill_executor(mut self, executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(executor);
        self
    }

    /// 设置会话管理器。
    pub fn with_session_manager(mut self, session_manager: Arc<dyn SessionManager>) -> Self {
        self.session_manager = Some(session_manager);
        self
    }

    /// 设置后台命令日志目录（execute_command(background) 工具日志落盘位置）。
    pub fn with_command_logs_dir(mut self, dir: PathBuf) -> Self {
        self.command_logs_dir = Some(dir);
        self
    }

    /// 设置知识导入器（knowledge_ingest 工具依赖）。
    pub fn with_knowledge_ingestor(mut self, ingestor: Arc<KnowledgeIngestor>) -> Self {
        self.knowledge_ingestor = Some(ingestor);
        self
    }

    /// 设置安全配置（控制命令黑名单、安全模式等）。
    pub fn with_security_config(mut self, config: SecurityConfig) -> Self {
        self.security_config = Some(config);
        self
    }

    /// 设置使用统计追踪器。
    pub fn with_usage_stats(mut self, stats: Arc<UsageStats>) -> Self {
        self.usage_stats = Some(stats);
        self
    }

    /// 设置 Web 工具配置（web_search / web_fetch 工具启用开关与后端）。
    pub fn with_web_config(mut self, config: crate::config::WebConfig) -> Self {
        self.web_config = Some(config);
        self
    }

    /// 设置工作区快照管理器（会话回退时恢复文件修改）。
    pub fn with_snapshot_manager(mut self, manager: Arc<SnapshotManager>) -> Self {
        self.snapshot_manager = Some(manager);
        self
    }

    /// 设置全局默认工作目录（[agent] working_directory 配置值）。
    ///
    /// 会话未绑定工作目录（或绑定目录不存在）时，快照捕获以此为根。
    pub fn with_default_working_directory(mut self, dir: Option<PathBuf>) -> Self {
        self.default_working_directory = dir;
        self
    }

    /// 设置技能注册表刷新钩子（压缩会话转换点时增量注册 VFS 学习技能）。
    pub fn with_skill_refresher(mut self, refresher: Arc<dyn SkillRefresher>) -> Self {
        self.skill_refresher = Some(refresher);
        self
    }

    /// 设置后台任务 SQLite 持久化后端（ADR-013：任务实体化，重启可恢复）。
    pub fn with_background_task_db(mut self, db: crate::vfs::backend::sqlite_db::SqliteDb) -> Self {
        self.background_task_db = Some(db);
        self
    }

    /// 设置系统通知通道（后台任务全部完成/失败、审批挂起时桌面通知）。
    ///
    /// 装配层（Tauri 实现）注入；未注入时通知静默（行为零变化）。
    pub fn with_notification_sink(
        mut self,
        sink: crate::notification::SharedNotificationSink,
    ) -> Self {
        self.notification_sink = Some(sink);
        self
    }

    /// 设置子 Agent 角色配置（delegate_to_agent role 参数）。
    ///
    /// 同名角色覆盖内置定义，新名字新增角色；未设置时使用内置角色。
    pub fn with_agent_roles(mut self, config: AgentRolesConfig) -> Self {
        self.agent_roles = Some(config);
        self
    }

    /// 注入角色注册表（ADR-016：VFS 加载的持久化注册表；优先于 [agent_roles] 合成）。
    pub fn with_role_registry(mut self, registry: Arc<RoleRegistry>) -> Self {
        self.role_registry = Some(registry);
        self
    }

    /// 注入角色向量路由器（ADR-016 P3：suggest_role 工具依赖）。
    pub fn with_role_router(mut self, router: Arc<RoleRouter>) -> Self {
        self.role_router = Some(router);
        self
    }

    /// 设置结构化 Trace 收集器（G6：轮次/工具/任务 span 持久化）。
    pub fn with_trace_collector(
        mut self,
        collector: Arc<crate::observability::trace::TraceCollector>,
    ) -> Self {
        self.trace_collector = Some(collector);
        self
    }

    /// 设置执行记录日志（ADR-017 GEPA 数据层：execution_stats 等工具与
    /// 执行记录持久化依赖；None 时工具不可用）。
    pub fn with_execution_log(mut self, log: Arc<ExecutionLog>) -> Self {
        self.execution_log = Some(log);
        self
    }

    /// 设置会话回忆服务（ADR-017 决策 6：session_recall 工具依赖）。
    pub fn with_session_recall(
        mut self,
        recall: Arc<crate::session::search::SessionRecall>,
    ) -> Self {
        self.session_recall = Some(recall);
        self
    }

    /// 设置会话权威存储（ADR-018：vfs_read 对 tianyan://session/{id} 的兼容读取）。
    pub fn with_session_store(mut self, store: Arc<crate::session::store::SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// 设置 LLM 用量日志（token 统计：每轮调用落库；None 时不记录）。
    pub fn with_usage_log(mut self, log: Arc<crate::observability::usage_log::UsageLog>) -> Self {
        self.usage_log = Some(log);
        self
    }

    /// 设置 model → provider 映射（用量日志 provider 维度；空表回落前缀推导）。
    pub fn with_provider_by_model(
        mut self,
        map: std::collections::HashMap<String, String>,
    ) -> Self {
        self.provider_by_model = map;
        self
    }

    /// 设置用户问题服务（ask_user 同步等待用户回答；None 时工具不可用）。
    pub fn with_user_questions(
        mut self,
        service: Arc<crate::agent::user_questions::UserQuestionService>,
    ) -> Self {
        self.user_questions = Some(service);
        self
    }

    /// 设置聊天模型上下文规格（T5）。
    ///
    /// 规格注入 AgentLoop（供后续请求构造使用），并把压缩器
    /// `context_window` 联动为 `spec.context_length`；未设置时压缩器
    /// 保持默认窗口 128_000，AgentLoop.chat_spec 为 None。
    pub fn with_chat_model_spec(mut self, spec: ModelSpec) -> Self {
        self.chat_model_spec = Some(spec);
        self
    }

    /// 构建 Agent 实例。
    pub fn build(self) -> Result<Agent> {
        let model_service = self
            .model_service
            .ok_or_else(|| TianyanError::Custom("内部错误：需要模型服务".to_string()))?;

        let retriever = self
            .retriever
            .ok_or_else(|| TianyanError::Custom("内部错误：需要检索器".to_string()))?;

        let vfs = self
            .vfs
            .ok_or_else(|| TianyanError::Custom("内部错误：需要虚拟文件系统".to_string()))?;

        let session_manager = self
            .session_manager
            .ok_or_else(|| TianyanError::Custom("内部错误：需要会话管理器".to_string()))?;

        // 构建可观测性指标（tool_registry 依赖）
        let metrics = AgentMetrics::new();

        // Build security policy from user config (or defaults)
        let security_config = self.security_config.clone().unwrap_or_default();
        let security_policy = SecurityPolicy::from_config(&security_config);

        // 构建审批工作流（write_file / execute_command 危险操作门控）
        // wait_for_approval 开启时：危险操作挂起等待 GUI 审批面板人工响应；
        // 默认关闭：走"询问用户 → 指纹确认"降级链路
        // 安全配置注入（T2-6 命令级审批策略）：
        //   blocked_commands → Deny 规则（强制拒绝，评估时 deny 优先，
        //                       无视自动放行规则）；
        //   allowed_commands → Approve 规则（自动放行）；
        //   prompt_commands  → 总是询问（强制走人工审批，不被自动放行）。
        let mut approval_config = ApprovalWorkflowConfig {
            wait_for_approval: security_config.wait_for_approval,
            ..Default::default()
        };
        approval_config.prompt_commands = security_config.prompt_commands.clone();
        for cmd in &security_config.blocked_commands {
            approval_config.auto_approval_rules.push(AutoApprovalRule {
                name: format!("security_blocked_{cmd}"),
                action_pattern: ActionPattern::CommandPattern(cmd.clone()),
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Deny,
                enabled: true,
            });
        }
        for cmd in &security_config.allowed_commands {
            approval_config.auto_approval_rules.push(AutoApprovalRule {
                name: format!("security_allowed_{cmd}"),
                action_pattern: ActionPattern::CommandPattern(cmd.clone()),
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Approve,
                enabled: true,
            });
        }
        // 完全放开模式：除 Deny 规则（黑名单）外全部自动批准。
        // 评估顺序保证 Deny 优先（check_auto_approval 第一遍先跑拒绝规则），
        // 因此 blocked_commands 黑名单在任何模式下都强制生效。
        if security_config.allow_all_operations {
            approval_config.auto_approval_rules.push(AutoApprovalRule {
                name: "allow_all_operations".to_string(),
                action_pattern: ActionPattern::Any,
                condition: ApprovalCondition::Always,
                decision: ApprovalDecision::Approve,
                enabled: true,
            });
        }
        let mut approval_notifier = SessionApprovalNotifier::new(session_manager.clone());
        if let Some(ref sink) = self.notification_sink {
            approval_notifier = approval_notifier.with_notification_sink(sink.clone());
        }
        let approval = Arc::new(
            ApprovalWorkflow::new(approval_config)
                .with_pending_notifier(Arc::new(approval_notifier)),
        );

        let chat_model = self.model.clone().unwrap_or_else(|| "default".to_string());

        // 构建 LLM-as-Judge 语义验证（verify_build 质量门控）
        let verification = Arc::new(VerificationGate::new(Some(LlmJudge::new(
            model_service.clone(),
            &chat_model,
        ))));

        // 构建规则记录器（工具执行失败时自动学习，供 GEPA 和 RuleSuggester 消费）
        let rule_recorder = Arc::new(RuleRecorder::new(vfs.clone()));

        let mut tool_registry = ToolRegistry::new(security_policy)
            .with_vfs(vfs.clone())
            .with_model_service(model_service.clone())
            .with_model(&chat_model)
            .with_metrics(metrics.clone())
            .with_approval_workflow(approval)
            .with_verification_gate(verification)
            .with_rule_recorder(rule_recorder)
            .with_lsp_manager(Arc::new(LspManager::new()))
            .with_session_manager(session_manager.clone());
        // 后台命令日志目录（execute_command(background) 日志落盘）
        if let Some(dir) = self.command_logs_dir {
            tool_registry = tool_registry.with_command_logs_dir(dir);
        }
        // 角色向量路由器（suggest_role 工具）
        if let Some(router) = self.role_router {
            tool_registry = tool_registry.with_role_router(router);
        }
        // 子 Agent 角色注册表（delegate_to_agent role 参数）
        if let Some(registry) = self.role_registry {
            // ADR-016：VFS 持久化注册表（内置种子 + 配置签名 + 演化实体）
            tool_registry = tool_registry.with_role_registry(registry);
        } else if let Some(agent_roles) = self.agent_roles {
            tool_registry =
                tool_registry.with_role_registry(Arc::new(RoleRegistry::from_config(&agent_roles)));
        }
        if let Some(ref stats) = self.usage_stats {
            tool_registry = tool_registry.with_usage_stats(stats.clone());
        }
        if let Some(ref executor) = self.skill_executor {
            tool_registry = tool_registry.with_skill_executor(executor.clone());
        }
        if let Some(ref ingestor) = self.knowledge_ingestor {
            tool_registry = tool_registry.with_knowledge_ingestor(ingestor.clone());
        }
        // Web 工具（web_search / web_fetch）：配置启用时构建客户端注入
        if let Some(web_config) = self.web_config {
            if web_config.enabled {
                match crate::executor::web::WebSearchClient::new(&web_config) {
                    Ok(client) => {
                        tool_registry = tool_registry.with_web_client(Arc::new(client));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Web 工具客户端初始化失败，web_search/web_fetch 不可用");
                    }
                }
            }
        }
        // 后台任务完成通知器：把通知持久化到父会话（主 LLM 下一轮看到并继续）
        tool_registry = tool_registry.with_task_notifier(Arc::new(
            crate::agent::background::SessionTaskNotifier::new(session_manager.clone()),
        ));
        // 后台命令完成通知器（execute_command(background) 终态注入父会话）
        tool_registry = tool_registry.with_command_notifier(Arc::new(
            crate::agent::background::SessionCommandNotifier::new(session_manager.clone()),
        ));
        // 后台任务系统通知通道（全部完成/失败时桌面通知）
        if let Some(ref sink) = self.notification_sink {
            tool_registry = tool_registry.with_notification_sink(sink.clone());
        }
        // 后台任务持久化（ADR-013：任务实体化，重启可查询可恢复）
        if let Some(ref db) = self.background_task_db {
            tool_registry = tool_registry.with_background_task_db(db.clone());
        }
        // 后台任务结果自审（G4，opt-in）：完成通知注入前 LLM 自审，
        // 未通过则结果带 [自审未通过] 标记，主 agent 复核
        if self.config.background_self_review {
            tool_registry = tool_registry.with_task_reviewer(Arc::new(
                crate::executor::judge::LlmTaskReviewer::new(model_service.clone(), &chat_model),
            ));
        }
        // 结构化 Trace（G6）：轮次/工具/任务 span 持久化（工具 + 后台任务）
        if let Some(ref trace) = self.trace_collector {
            tool_registry = tool_registry.with_trace_collector(trace.clone());
        }
        // 执行记录日志（ADR-017 GEPA 数据层：execution_stats 等统计工具 + 持久化）
        if let Some(ref log) = self.execution_log {
            tool_registry = tool_registry.with_execution_log(log.clone());
        }
        // 会话回忆服务（ADR-017 决策 6：session_recall 工具）
        if let Some(ref recall) = self.session_recall {
            tool_registry = tool_registry.with_session_recall(recall.clone());
        }
        // 会话权威存储（ADR-018：vfs_read 会话 URI 兼容层）
        if let Some(ref store) = self.session_store {
            tool_registry = tool_registry.with_session_store(store.clone());
        }
        // 用户问题服务（ask_user 同步等待用户回答）
        if let Some(ref service) = self.user_questions {
            tool_registry = tool_registry.with_user_questions(service.clone());
        }

        let mut agent_loop = AgentLoop::new(
            model_service.clone(),
            tool_registry,
            session_manager.clone(),
            AgentLoopConfig {
                max_turns: self.config.max_turns,
                ..Default::default()
            },
        );
        // 结构化 Trace（G6）：轮次 span
        if let Some(ref trace) = self.trace_collector {
            agent_loop = agent_loop.with_trace_collector(trace.clone());
        }
        // LLM 用量日志（token 统计）
        if let Some(ref usage_log) = self.usage_log {
            agent_loop = agent_loop.with_usage_log(usage_log.clone());
        }
        if !self.provider_by_model.is_empty() {
            agent_loop = agent_loop.with_provider_by_model(self.provider_by_model.clone());
        }
        // 聊天模型上下文规格注入（T5）：None 时 AgentLoop 内部走默认窗口
        agent_loop = agent_loop.with_chat_spec(self.chat_model_spec);

        // 构建上下文管线
        let context_pipeline = ContextPipeline::new(
            vfs.clone(),
            retriever.clone(),
            Arc::new(TokioMutex::new(ContextCompressor::new(
                model_service.clone(),
                CompressionConfig {
                    // 模型规格联动压缩窗口；无 spec 时保持默认 128_000（语义与
                    // DEFAULT_CONTEXT_WINDOW 一致，不能依赖 ..Default::default() 覆盖）。
                    context_window: self
                        .chat_model_spec
                        .map(|s| s.context_length)
                        .unwrap_or(128_000),
                    preserve_recent_messages: 6,
                    summary_model: chat_model.clone(),
                    ..CompressionConfig::default()
                },
            ))),
            self.config.default_top_k,
            self.config.learned_rules_top_k,
        );

        // ADR-017：会话末 GEPA 引擎移除——技能/角色演化统一由每日演化任务
        // （EvolutionTask + 演化智能体综述）驱动；执行轨迹持久化在
        // ToolObservabilityListener（GEPA 数据层）。

        Ok(Agent::new(
            chat_model,
            context_pipeline,
            metrics,
            agent_loop,
            session_manager,
            self.snapshot_manager,
            self.default_working_directory,
            self.skill_refresher,
        ))
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::types::{Message, StructuredMessage};
    use crate::context::DualLayerRetriever;
    use crate::model::spec::ModelSpec;
    use crate::model::MockChatService;
    use crate::session::{Session, SessionManager};
    use crate::test_utils::MockVfs;
    use crate::vfs::VirtualFileSystem;
    use async_trait::async_trait;

    struct MockSessionManager;
    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn add_structured_message(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> Result<()> {
            Ok(())
        }
        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> Result<()> {
            Ok(())
        }
        async fn create_session(&self, _id: &str, _message: Message) -> Result<Session> {
            Ok(Session::new(_id))
        }
        async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
            Ok(None)
        }
        async fn update_session(&self, _session: &Session) -> Result<()> {
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<Session>> {
            Ok(vec![])
        }
        async fn delete_session(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    /// 用最小依赖构建一个完整 Agent（测试 build() 全链路）。
    fn build_test_agent(with_spec: bool) -> Result<Agent> {
        let vfs = Arc::new(MockVfs::new());
        let mut builder = AgentBuilder::new()
            .with_model("test-model")
            .with_model_service(Arc::new(MockChatService::new()))
            .with_vfs(vfs.clone() as Arc<dyn VirtualFileSystem>)
            .with_retriever(Arc::new(DualLayerRetriever::new(
                vfs.clone() as Arc<dyn VirtualFileSystem>
            )))
            .with_session_manager(Arc::new(MockSessionManager));
        if with_spec {
            builder = builder.with_chat_model_spec(ModelSpec {
                context_length: 200_000,
                max_output_tokens: 16_000,
                max_input_tokens: 184_000,
            });
        }
        builder.build()
    }

    #[test]
    fn test_agent_builder() {
        let builder = AgentBuilder::new();
        assert_eq!(builder.config.max_turns, 200);
    }

    #[tokio::test]
    async fn test_with_chat_model_spec_sets_loop_spec_and_window() {
        // with_chat_model_spec(spec) → build() 后：
        // 1) AgentLoop.chat_spec == Some(spec)
        // 2) 压缩器 context_window == spec.context_length（窗口联动）
        let spec = ModelSpec {
            context_length: 200_000,
            max_output_tokens: 16_000,
            max_input_tokens: 184_000,
        };
        let vfs = Arc::new(MockVfs::new());
        let agent = AgentBuilder::new()
            .with_model("test-model")
            .with_model_service(Arc::new(MockChatService::new()))
            .with_vfs(vfs.clone() as Arc<dyn VirtualFileSystem>)
            .with_retriever(Arc::new(DualLayerRetriever::new(
                vfs.clone() as Arc<dyn VirtualFileSystem>
            )))
            .with_session_manager(Arc::new(MockSessionManager))
            .with_chat_model_spec(spec)
            .build()
            .unwrap();

        assert_eq!(
            agent.agent_loop.chat_spec,
            Some(spec),
            "spec 应注入 AgentLoop"
        );
        let status = agent
            .context_pipeline
            .compressor()
            .lock()
            .await
            .get_status(0);
        assert_eq!(
            status.context_window, spec.context_length,
            "压缩器 context_window 应与 spec.context_length 联动"
        );
    }

    #[tokio::test]
    async fn test_no_chat_model_spec_defaults_window() {
        // 无 spec 时：AgentLoop.chat_spec 为 None，压缩器 context_window 保持默认 128_000。
        let agent = build_test_agent(false).unwrap();
        assert_eq!(agent.agent_loop.chat_spec, None);

        let status = agent
            .context_pipeline
            .compressor()
            .lock()
            .await
            .get_status(0);
        assert_eq!(status.context_window, 128_000);
    }
}
