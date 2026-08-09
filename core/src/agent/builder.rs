use std::sync::Arc;

use tokio::sync::Mutex as TokioMutex;

use crate::agent::r#loop::{AgentLoop, AgentLoopConfig};
use crate::agent::tool_registry::ToolRegistry;
use crate::common::error::{Result, TianyanError};
use crate::config::AgentConfig;
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
use crate::model::ChatService;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::scheduler::tasks::RuleRecorder;
use crate::session::SessionManager;
use crate::skills::learning::{SkillLearningConfig, SkillLearningEngine};
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
    /// 技能注册表刷新钩子（压缩会话转换点时增量注册 VFS 学习技能）。
    skill_refresher: Option<Arc<dyn SkillRefresher>>,
    /// 后台任务 SQLite 持久化后端（ADR-013；None 时任务状态纯内存）。
    background_task_db: Option<crate::vfs::backend::sqlite_db::SqliteDb>,
    /// 系统通知通道（后台任务完成 / 审批挂起的桌面通知；None 时静默）。
    notification_sink: Option<crate::notification::SharedNotificationSink>,
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
            skill_refresher: None,
            background_task_db: None,
            notification_sink: None,
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
            .with_lsp_manager(Arc::new(LspManager::new()));
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
        // 后台任务系统通知通道（全部完成/失败时桌面通知）
        if let Some(ref sink) = self.notification_sink {
            tool_registry = tool_registry.with_notification_sink(sink.clone());
        }
        // 后台任务持久化（ADR-013：任务实体化，重启可查询可恢复）
        if let Some(ref db) = self.background_task_db {
            tool_registry = tool_registry.with_background_task_db(db.clone());
        }

        let agent_loop = AgentLoop::new(
            model_service.clone(),
            tool_registry,
            session_manager.clone(),
            AgentLoopConfig {
                max_turns: self.config.max_turns,
                ..Default::default()
            },
        );

        // 构建上下文管线
        let context_pipeline = ContextPipeline::new(
            vfs.clone(),
            retriever.clone(),
            Arc::new(TokioMutex::new(ContextCompressor::new(
                model_service.clone(),
                CompressionConfig {
                    preserve_recent_messages: 6,
                    summary_model: chat_model.clone(),
                    ..CompressionConfig::default()
                },
            ))),
            self.config.default_top_k,
            self.config.learned_rules_top_k,
        );

        // 构建技能学习引擎
        let skill_learning_engine = if self.config.enable_skills {
            Some(SkillLearningEngine::new(
                model_service.clone(),
                vfs.clone(),
                SkillLearningConfig {
                    generation_model: chat_model.clone(),
                    ..SkillLearningConfig::default()
                },
            ))
        } else {
            None
        };

        Ok(Agent::new(
            chat_model,
            context_pipeline,
            metrics,
            skill_learning_engine,
            agent_loop,
            session_manager,
            self.snapshot_manager,
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

    #[test]
    fn test_agent_builder() {
        let builder = AgentBuilder::new();
        assert!(builder.config.enable_skills);
        assert!(builder.config.enable_memory);
    }
}
