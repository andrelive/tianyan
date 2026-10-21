use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::r#loop::{AgentLoop, AgentLoopConfig};
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::{RoleRegistry, RoleRouter};
use crate::common::error::{Result, TianyanError};
use crate::config::AgentConfig;
use crate::config::AgentRolesConfig;
use crate::config::SecurityConfig;
use crate::context::{ContextPort, NullContextPort};
use crate::executor::approval::ApprovalGate;
use crate::executor::SecurityPolicy;
use crate::knowledge::KnowledgeIngestor;
use crate::model::spec::ModelSpec;
use crate::model::ChatService;
use crate::observability::execution_log::ExecutionLog;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::observability::RuleRecorder;
use crate::session::SessionManager;
use crate::snapshot::SnapshotManager;
use crate::vfs::VirtualFileSystem;

use super::agent_core::Agent;

/// 用于创建智能体的构建器。
pub struct AgentBuilder {
    config: AgentConfig,
    /// 默认聊天模型（用于 AgentLoop 和委托子 Agent）。
    model: Option<String>,
    model_service: Option<Arc<dyn ChatService>>,
    /// 上下文工程端口（注入/压缩/摘要提取；ADR-049 期 3-T2：实现已出 core，
    /// 由装配层经 `ContextFactory` 构造后注入）。
    ///
    /// `None` = 未装配 → [`NullContextPort`]（内存直通：无 system 前缀、不压缩）
    /// + 装配期告警，见 [`Self::with_context_port`]。
    context_port: Option<Arc<dyn ContextPort>>,
    vfs: Option<Arc<dyn VirtualFileSystem>>,
    session_manager: Option<Arc<dyn SessionManager>>,
    knowledge_ingestor: Option<Arc<KnowledgeIngestor>>,
    security_config: Option<SecurityConfig>,
    usage_stats: Option<Arc<UsageStats>>,
    /// 工作区快照管理器（配置了 working_directory 时启用）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 全局默认工作目录（[agent] working_directory 配置；会话级绑定缺省时
    /// 快照/工具操作以此为根）。
    default_working_directory: Option<PathBuf>,
    /// 后台任务 SQLite 持久化后端（ADR-013；None 时任务状态纯内存）。
    background_task_db: Option<Arc<crate::db::Database>>,
    /// 系统通知通道（后台任务完成 / 审批挂起的桌面通知；None 时静默）。
    notification_sink: Option<crate::notification::SharedNotificationSink>,
    /// 审批门控（危险操作门控；ADR-049 期 3-T1：实现已出 core，由装配层注入）。
    ///
    /// `None` = 未装配 → 工具放行分支（`ToolContext::ensure_approved` 的
    /// 「未装配直接放行」）+ 装配期告警，见 [`Self::with_approval_gate`]。
    approval_gate: Option<Arc<dyn ApprovalGate>>,
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
    /// 会话工作集注册表（外部注入优先：server 侧持有同一 Arc 做写侧收口，
    /// ADR-035 §3；未注入时 build 内自建）。
    working_sets: Option<Arc<crate::agent::working_set::WorkingSetRegistry>>,
    usage_log: Option<Arc<crate::observability::usage_log::UsageLog>>,
    provider_by_model: std::collections::HashMap<String, String>,
    /// 全局默认聊天提供商（会话未选择时的路由兜底；None 时不注入）。
    default_chat_provider: Option<String>,
    /// 每 provider 的聊天客户端（会话级模型选择路由；空时不注入）。
    chat_clients: std::collections::HashMap<String, crate::model::SharedChatService>,
    /// 聊天模型上下文规格（T5；注入 AgentLoop 并联动压缩窗口；None 时走默认窗口）。
    chat_model_spec: Option<ModelSpec>,
    /// 后台命令日志目录（execute_command(background) 日志落盘；None 时仅内存尾部）。
    command_logs_dir: Option<PathBuf>,
    /// 委托结果落盘目录（U2：完整结果写 `{dir}/{id}.md`；None 时不落盘）。
    task_results_dir: Option<PathBuf>,
    /// 工具输出落盘（ADR-047：超预算结果落盘目录 + 预算；None 时不落盘）。
    tool_output: Option<(PathBuf, crate::config::ToolOutputConfig)>,
    /// 用户问题服务（ask_user 同步等待用户回答；None 时工具不可用）。
    user_questions: Option<Arc<crate::agent::user_questions::UserQuestionService>>,
    /// 子智能体消息流事件通道（ADR-026：面板实时流式；None 时静默）。
    task_event_sink: Option<Arc<dyn crate::agent::background::TaskEventSink>>,
}

impl AgentBuilder {
    /// 创建默认构建器。
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            model: None,
            model_service: None,
            context_port: None,
            vfs: None,
            session_manager: None,
            knowledge_ingestor: None,
            security_config: None,
            usage_stats: None,
            snapshot_manager: None,
            default_working_directory: None,
            background_task_db: None,
            notification_sink: None,
            approval_gate: None,
            agent_roles: None,
            trace_collector: None,
            execution_log: None,
            session_recall: None,
            session_store: None,
            working_sets: None,
            usage_log: None,
            provider_by_model: std::collections::HashMap::new(),
            default_chat_provider: None,
            chat_clients: std::collections::HashMap::new(),
            chat_model_spec: None,
            command_logs_dir: None,
            task_results_dir: None,
            tool_output: None,
            role_registry: None,
            role_router: None,
            user_questions: None,
            task_event_sink: None,
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

    /// 注入上下文工程端口（系统前缀加载 + 会话压缩 + 压缩点摘要提取；
    /// ADR-049 期 3-T2）。
    ///
    /// 上下文**策略**实现已出 core（`plugins/policy-context`）：注入顺序（soul →
    /// AGENTS.md → learned rules → memories）、检索的 RRF 参数与记忆偏置、压缩阈值与
    /// 摘要提示词、意图分类关键词都在插件里。装配层先用 `ContextFactory` 把它构造成
    /// `Arc<dyn ContextPort>` 再经本方法交给 builder（与 `ApprovalGateFactory` /
    /// `TaskFactory` 同一套类型擦除手法）。
    ///
    /// **未注入（`None`）的语义**：用 [`NullContextPort`] 兜底——`load_injectable`
    /// 返回空可注入上下文（模型**没有** soul / rules / memories 前缀）、
    /// `compress_for_session` 恒 `None`（上下文永不压缩），并在 `build` 里 `warn!`
    /// 一条指路。为什么允许降级而不硬失败：`create_app`（构造对象图，47 处 lib 单测 +
    /// 嵌入式）靠「不要求装配」才能起实例；`start_server*` 另有启动期硬守卫
    /// （`has_context_factory`），生产误配在**启动前**响亮失败。可见性由装配期告警兜底
    /// （ADR-038：静默降级的症状是「模型像丢了人设 + 上下文无限增长」，看不出是降级）。
    pub fn with_context_port(mut self, port: Arc<dyn ContextPort>) -> Self {
        self.context_port = Some(port);
        self
    }

    /// 设置虚拟文件系统。
    pub fn with_vfs(mut self, vfs: Arc<dyn VirtualFileSystem>) -> Self {
        self.vfs = Some(vfs);
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

    /// 设置委托结果落盘目录（U2：完整结果写 `{dir}/{id}.md`，完成通知携带路径）。
    pub fn with_task_results_dir(mut self, dir: PathBuf) -> Self {
        self.task_results_dir = Some(dir);
        self
    }

    /// 设置工具输出落盘（ADR-047）：超预算工具结果的落盘目录 + 预算配置。
    ///
    /// 未调用 = 不落盘（工具结果原样进会话）。
    pub fn with_tool_output(
        mut self,
        dir: PathBuf,
        config: crate::config::ToolOutputConfig,
    ) -> Self {
        self.tool_output = Some((dir, config));
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

    /// 设置后台任务 SQLite 持久化后端（ADR-013：任务实体化，重启可恢复）。
    pub fn with_background_task_db(mut self, db: Arc<crate::db::Database>) -> Self {
        self.background_task_db = Some(db);
        self
    }

    /// 设置子智能体消息流事件通道（ADR-026：面板实时流式显示）。
    pub fn with_task_event_sink(
        mut self,
        sink: Arc<dyn crate::agent::background::TaskEventSink>,
    ) -> Self {
        self.task_event_sink = Some(sink);
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

    /// 注入审批门控（危险操作门控；ADR-049 期 3-T1）。
    ///
    /// 审批**策略**实现已出 core（`plugins/policy-approval`）：风险定级、自动规则
    /// 引擎、`ApprovalMode` 三态短路、人工审批挂起通道都在插件里，装配层先用
    /// `ApprovalGateFactory` 把它构造成 `Arc<dyn ApprovalGate>` 再经本方法交给
    /// builder（与 `ToolPluginFactory` / `VectorStorageFactory` 同一套类型擦除手法）。
    ///
    /// **未注入（`None`）的语义**：不装配审批门控——`ToolContext::ensure_approved`
    /// 走既有的「未装配直接放行」分支（工具照常执行），并在 `build` 里 `warn!` 一条
    /// 指路。为什么允许降级而不硬失败：默认配置就是 `ApprovalMode::Autonomous`
    /// （黑名单外全放行，ADR-033），且命令黑名单在 `SecurityPolicy` 守卫层**另有**
    /// 一份拦截；而 `create_app` 这组测试/嵌入式入口靠「构造对象图不要求装配」才能
    /// 起实例。可见性由装配期告警兜底（ADR-038：静默降级的症状会是「审批面板永远
    /// 空的 + 黑名单少一层复核」，看不出是降级还是没触发）。
    pub fn with_approval_gate(mut self, gate: Arc<dyn ApprovalGate>) -> Self {
        self.approval_gate = Some(gate);
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

    /// 注入会话工作集注册表（ADR-035：server 侧与 Agent 共享同一 Arc——
    /// 写侧收口与读侧缓存必须操作同一份工作集状态）。
    pub fn with_working_sets(
        mut self,
        registry: Arc<crate::agent::working_set::WorkingSetRegistry>,
    ) -> Self {
        self.working_sets = Some(registry);
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

    /// 设置全局默认聊天提供商（用量日志 provider 维度兜底；会话未选择时使用）。
    pub fn with_default_chat_provider(mut self, provider: Option<String>) -> Self {
        self.default_chat_provider = provider;
        self
    }

    /// 设置每 provider 的聊天客户端（会话级模型选择路由；空 = 禁用路由）。
    pub fn with_chat_clients(
        mut self,
        clients: std::collections::HashMap<String, crate::model::SharedChatService>,
    ) -> Self {
        self.chat_clients = clients;
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

        // 会话工作集（ADR-039：物化缓存 + 轮边界增量注入 + **写侧唯一入口**）。
        // 与 ToolRegistry/Agent/通知器共享**同一 Arc**（状态唯一）；外部注入
        // （server 侧）优先，未注入时自建。未装配会话存储时注册表仍在
        // （锁与生命周期管理可用），写路径 `ensure` 失败即告警（无直写回退）。
        let working_sets = self.working_sets.clone().unwrap_or_else(|| {
            crate::agent::working_set::WorkingSetRegistry::new(self.session_store.clone())
        });
        // 审批门控（write_file / apply_edit / apply_patch / execute_command
        // 危险操作门控）：**实现已出 core**（ADR-049 期 3-T1，`plugins/policy-approval`）。
        // 装配层先用 `ApprovalGateFactory` 把 `SecurityConfig` 的审批面
        // （`approval_mode` / `blocked_commands` / `allowed_commands` /
        // `prompt_commands`）+ 会话工作集 + 通知通道翻成实例，再经
        // `with_approval_gate` 注入——「配置 → 规则」的映射因此是插件里的单一事实源，
        // 内核不认识审批策略。
        let approval = self.approval_gate.clone();
        if approval.is_none() {
            tracing::warn!(
                "未装配审批门控（ApprovalGateFactory）：危险操作（write_file / apply_edit / \
                 apply_patch / execute_command）不经过审批门控直接放行——生产装配请用 \
                 create_app_with_assembly / start_server_with_assembly"
            );
        }

        let chat_model = self.model.clone().unwrap_or_else(|| "default".to_string());

        // 构建规则记录器（工具执行失败时自动学习，供 GEPA 和 RuleSuggester 消费）
        let rule_recorder = Arc::new(RuleRecorder::new(vfs.clone()));

        let mut tool_registry = ToolRegistry::new(security_policy)
            .with_vfs(vfs.clone())
            .with_model_service(model_service.clone())
            .with_model(&chat_model)
            .with_metrics(metrics.clone())
            .with_rule_recorder(rule_recorder)
            .with_session_manager(session_manager.clone());
        // 审批门控只在装配了就注入（未装配 → `ToolContext.approval_gate` 保持 None，
        // `ensure_approved` 走「未装配直接放行」分支；见上方告警）
        if let Some(gate) = approval {
            tool_registry = tool_registry.with_approval_gate(gate);
        }
        // 后台命令日志目录（execute_command(background) 日志落盘）
        if let Some(dir) = self.command_logs_dir {
            tool_registry = tool_registry.with_command_logs_dir(dir);
        }
        // 委托结果落盘目录（U2：完整结果写文件，完成通知携带路径）
        if let Some(dir) = self.task_results_dir {
            tool_registry = tool_registry.with_task_results_dir(dir);
        }
        // 工具输出落盘（ADR-047：超预算结果落盘，会话只留预览 + 路径）
        if let Some((dir, config)) = self.tool_output.clone() {
            tool_registry = tool_registry.with_tool_output(dir, &config);
        }
        // 并发配置（ADR-026）：委托双信号量（运行/排队）+ 终端命令上限
        tool_registry = tool_registry.with_concurrency(
            self.config.max_background_concurrency,
            self.config.max_background_queue,
            self.config.max_command_concurrency,
        );
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
        if let Some(ref ingestor) = self.knowledge_ingestor {
            tool_registry = tool_registry.with_knowledge_ingestor(ingestor.clone());
        }
        // 后台任务完成通知器：把通知持久化到父会话（主 LLM 下一轮看到并继续）。
        // ADR-035：经工作集落库（物化段同步推进 → 活动轮的轮边界续跑判定可发现它）
        tool_registry = tool_registry.with_task_notifier(Arc::new(
            crate::agent::background::SessionTaskNotifier::new(Some(working_sets.clone())),
        ));
        // 子智能体消息流事件通道（ADR-026：面板实时流式显示）
        if let Some(sink) = self.task_event_sink {
            tool_registry = tool_registry.with_task_event_sink(sink);
        }
        // 后台命令完成通知器（execute_command(background) 终态注入父会话）
        tool_registry = tool_registry.with_command_notifier(Arc::new(
            crate::agent::background::SessionCommandNotifier::new(Some(working_sets.clone())),
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
        tool_registry = tool_registry.with_working_sets(working_sets.clone());
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
        if self.default_chat_provider.is_some() {
            agent_loop = agent_loop.with_default_chat_provider(self.default_chat_provider.clone());
        }
        if !self.chat_clients.is_empty() {
            agent_loop = agent_loop.with_chat_clients(self.chat_clients.clone());
        }
        // 聊天模型上下文规格注入（T5）：None 时 AgentLoop 内部走默认窗口
        agent_loop = agent_loop.with_chat_spec(self.chat_model_spec);

        // 上下文工程端口（ADR-049 期 3-T2）：**实现已出 core**
        // （`plugins/policy-context`）。注入顺序、检索策略、压缩阈值与摘要提示词都在
        // 插件里——「`context_window` ↔ 模型规格联动」也随之进插件（
        // `tianyan_policy_context::pipeline`，单一事实源）。装配层先用
        // `ContextFactory` 造实例，再经 `with_context_port` 注入。
        let context_port = match self.context_port.clone() {
            Some(port) => port,
            None => {
                tracing::warn!(
                    "未装配上下文策略（ContextFactory）：模型没有 system 前缀（soul / 项目指令 / \
                     learned rules / memories），且上下文**永不压缩**——生产装配请用 \
                     create_app_with_assembly / start_server_with_assembly"
                );
                Arc::new(NullContextPort::new())
            }
        };

        // ADR-017：会话末 GEPA 引擎移除——技能/角色演化统一由每日演化任务
        // （EvolutionTask + 演化智能体综述）驱动；执行轨迹持久化在
        // ToolObservabilityListener（GEPA 数据层）。

        Ok(Agent::new(
            chat_model,
            context_port,
            metrics,
            agent_loop,
            session_manager,
            self.snapshot_manager,
            self.default_working_directory,
            working_sets,
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

    use crate::common::types::{InjectableContext, Message, StructuredMessage};
    use crate::model::spec::ModelSpec;
    use crate::model::MockChatService;
    use crate::session::{Session, SessionManager};
    use crate::test_utils::MockVfs;
    use crate::vfs::VirtualFileSystem;
    use async_trait::async_trait;

    struct MockSessionManager;
    #[async_trait]
    impl SessionManager for MockSessionManager {
        // ADR-039：破坏性写不在 trait 上（写路径唯一入口 = 会话工作集）
        async fn create_session(&self, _id: &str, _message: Message) -> Result<Session> {
            Ok(Session::new(_id))
        }
        async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
            Ok(None)
        }
        async fn list_sessions(&self) -> Result<Vec<Session>> {
            Ok(vec![])
        }
    }

    /// 假上下文端口（**测接线，不测策略**）：唯一作用是把「哪个实例被装配进去」
    /// 变成可观测的判别标记 —— `extract_marker_summary` 返回装配时给的字符串。
    ///
    /// 为什么不给 `ContextPort` 加一个 `marker()` 之类的方法：那是只服务测试的
    /// 契约污染（生产唯一实现永远返回同一个常量）。这里借用契约里**本来就有**的
    /// 第三个方法当作身份探针，零新增方法。
    struct StubContextPort {
        marker: &'static str,
    }

    #[async_trait]
    impl ContextPort for StubContextPort {
        async fn load_injectable(
            &self,
            _query: &str,
            _working_directory: Option<&std::path::Path>,
        ) -> Result<InjectableContext> {
            Ok(InjectableContext::new())
        }

        async fn compress_for_session(
            &self,
            _messages: &[Message],
            _session_id: &str,
            _recent_input_tokens: usize,
            _force: bool,
            _existing_summary: Option<&str>,
        ) -> Option<StructuredMessage> {
            None
        }

        fn extract_marker_summary(&self, _marker: &StructuredMessage) -> Option<String> {
            Some(self.marker.to_string())
        }
    }

    /// 端口身份探针（任意消息即可：桩不看内容）。
    fn probe_marker(agent: &Agent) -> Option<String> {
        let probe = crate::context::ContextAssembler::message_to_structured(
            &Message::user("probe"),
            "s1",
            None,
            None,
        );
        agent.context_pipeline.extract_marker_summary(&probe)
    }

    /// 用最小依赖构建一个完整 Agent（测试 build() 全链路）。
    fn build_test_agent(with_spec: bool) -> Result<Agent> {
        let vfs = Arc::new(MockVfs::new());
        let mut builder = AgentBuilder::new()
            .with_model("test-model")
            .with_model_service(Arc::new(MockChatService::new()))
            .with_vfs(vfs.clone() as Arc<dyn VirtualFileSystem>)
            .with_context_port(Arc::new(StubContextPort { marker: "injected" }))
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

    /// `with_chat_model_spec(spec)` → `AgentLoop.chat_spec == Some(spec)`
    /// 且注入的**就是**装配层给的那个端口。
    ///
    /// ADR-049 期 3-T2 的断言迁移：「压缩器 `context_window` 与 `spec.context_length`
    /// 联动」的原断言随管线搬进 `plugins/policy-context`
    /// （`pipeline_tests::test_pipeline_links_compressor_window_to_chat_spec`）——那测的是
    /// **管线的行为**；core 这里测**接线**（spec 送进循环、port 不被替换），比原来靠
    /// 具体类型穿透测得更准，且不弱化：窗口值仍是插件侧的精确相等断言。
    #[tokio::test]
    async fn test_with_chat_model_spec_injects_spec_into_loop() {
        let spec = ModelSpec {
            context_length: 200_000,
            max_output_tokens: 16_000,
            max_input_tokens: 184_000,
        };
        let vfs = Arc::new(MockVfs::new());
        let agent = AgentBuilder::new()
            .with_model("test-model")
            .with_model_service(Arc::new(MockChatService::new()))
            .with_vfs(vfs as Arc<dyn VirtualFileSystem>)
            .with_context_port(Arc::new(StubContextPort { marker: "injected" }))
            .with_session_manager(Arc::new(MockSessionManager))
            .with_chat_model_spec(spec)
            .build()
            .unwrap();

        assert_eq!(
            agent.agent_loop.chat_spec,
            Some(spec),
            "spec 应注入 AgentLoop"
        );
        assert_eq!(
            probe_marker(&agent).as_deref(),
            Some("injected"),
            "装配层注入的端口必须原样进入 Agent（不得被兜底实现替换）"
        );
    }

    /// 无 spec 时 `AgentLoop.chat_spec` 为 `None`（压缩窗口的缺省值判定在插件侧：
    /// `pipeline_tests::test_pipeline_defaults_compressor_window_without_spec`）。
    #[tokio::test]
    async fn test_no_chat_model_spec_keeps_loop_spec_none() {
        let agent = build_test_agent(false).unwrap();
        assert_eq!(agent.agent_loop.chat_spec, None);
        assert_eq!(
            probe_marker(&agent).as_deref(),
            Some("injected"),
            "无 spec 不影响端口装配"
        );
    }

    /// 未装配端口 → 兜底 `NullContextPort`（降级路径要**有判别力**：症状是
    /// 「模型没有 system 前缀 + 上下文永不压缩」，不能只靠日志）。
    ///
    /// 判别力：若把 `build()` 的兜底改成 panic / `Err`，本测试必红；若兜底实现
    /// 误报「压缩已发生」（`Some(...)`），`maybe_compress_and_persist` 就会落一个
    /// 假压缩点并收缩工作集段——用 `extract_marker_summary` 恒 `None` 锁住它。
    #[tokio::test]
    async fn test_missing_context_port_falls_back_to_null_port() {
        let vfs = Arc::new(MockVfs::new());
        let agent = AgentBuilder::new()
            .with_model("test-model")
            .with_model_service(Arc::new(MockChatService::new()))
            .with_vfs(vfs as Arc<dyn VirtualFileSystem>)
            .with_session_manager(Arc::new(MockSessionManager))
            .build()
            .expect("未装配端口不应让 build 失败（create_app 路径依赖这条）");

        assert_eq!(
            probe_marker(&agent).as_deref(),
            None,
            "未装配时应落到 NullContextPort（恒 None），而不是某个真实实现"
        );
        let injectable = agent
            .context_pipeline
            .load_injectable("q", None)
            .await
            .expect("降级实现不报错");
        assert!(
            injectable.soul.is_empty(),
            "降级实现给空可注入上下文（不伪造 soul）"
        );
        assert!(
            agent
                .context_pipeline
                .compress_for_session(&[], "s1", usize::MAX, true, None)
                .await
                .is_none(),
            "降级实现永不压缩（force 也不压缩）"
        );
    }
}
