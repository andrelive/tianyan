//! Application State Module
//!
//! 提供精简版的应用状态管理，只负责状态存储和访问。
//! 构建逻辑已委托给 AgentBuilderFactory。

// 标准库
use std::sync::{Arc, Mutex};

// 外部 crate
use tokio::sync::RwLock;

// 内部 crate
use crate::scheduled_tasks::ScheduledAgentTaskManager;
use tianyan::agent::AgentCoordinator;
use tianyan::config::{ModelEntry, TianyanConfig};
use tianyan::db::Database;
use tianyan::goals::GoalStore;
use tianyan::knowledge::{IngestorConfig, KnowledgeIngestor};
use tianyan::memory::{ExtractionConfig, MemoryExtractor};
use tianyan::model::spec::ModelSpec;
use tianyan::observability::execution_log::ExecutionLog;
use tianyan::observability::usage_log::UsageLog;
use tianyan::observability::usage_stats::UsageStats;
use tianyan::scheduler::TaskScheduler;
use tianyan::session::search::SessionRecall;
use tianyan::session::{PersistentSessionManager, SessionManager};
use tianyan::skills::SkillManager;
use tianyan::snapshot::SnapshotManager;
use tianyan::todos::TodoStore;
use tianyan::vfs::{SummaryEngine, VirtualFileSystemImpl};
use tianyan::{Result as TianyanResult, TianyanError};

use crate::agent_builder::{create_model_services, AgentBuilderFactory};
use crate::api::clipboard::tool::ClipboardWriteTool;
use crate::api::clipboard::types::PendingCapture;
use crate::mcp_bridge::McpToolManager;

/// 构造模型服务创建错误（统一错误前缀，避免调用点重复拼装）。
fn model_services_error(e: impl std::fmt::Display) -> TianyanError {
    TianyanError::Custom(format!("模型服务错误：模型服务创建失败：{e}"))
}

/// 按配置派生工作区快照管理器（配置了 `working_directory` 时启用）。
///
/// 组合根单点：`AppState::new` 与 `AppState::reload_agent` 共用——热重载
/// 必须用**新配置**重建（T0-12：旧实现沿用启动期实例，改工作目录后工具在
/// 新目录、快照/回退仍在旧目录；初始为 None 时热更新后快照永久不可用）。
fn build_snapshot_manager(config: &TianyanConfig) -> Option<Arc<SnapshotManager>> {
    config.agent.working_directory.clone().map(|workdir| {
        let root = config.storage.data_dir.join("snapshots");
        let workdir = std::path::PathBuf::from(workdir);
        tracing::info!(
            "工作区快照已启用: 目录={}, 存储={}",
            workdir.display(),
            root.display()
        );
        Arc::new(SnapshotManager::new(root, workdir))
    })
}

// ── 模型解析单一入口（组合根去重） ────────────────────────────────────────
//
// 全 server 各装配点（state.rs 工厂、agent_builder.rs、scheduler 任务注册）
// 的"从配置解析能力模型名"统一收敛于此，消除 6 份重复解析。

/// 解析 Chat 能力模型名（未配置时空串，由下游取默认）。
pub(crate) fn resolve_chat_model(config: &TianyanConfig) -> String {
    config
        .models
        .resolve(tianyan::config::ModelCapability::Chat)
        .map(|r| r.model)
        .unwrap_or_default()
}

/// 解析 Chat 能力模型的上下文规格（显式 > 内置表 > 默认）。
///
/// 从已解析的 Chat [`tianyan::config::ModelRef`] 回查 provider 下的
/// [`tianyan::config::ModelEntry`]：显式字段优先，其次内置规格表，最后
/// 全局默认；模型未配置/未找到时返回 [`ModelSpec::default`]（不 panic），
/// 并输出 warn 日志提示静默降级（不再无声回退）。
pub(crate) fn resolve_chat_model_spec(config: &TianyanConfig) -> ModelSpec {
    let Some(model_ref) = config
        .models
        .resolve(tianyan::config::ModelCapability::Chat)
    else {
        tracing::warn!("chat 模型未解析到配置条目，使用默认规格 32K/8K");
        return ModelSpec::default();
    };
    let Some(provider) =
        tianyan::config::find_provider(&config.models.providers, &model_ref.provider)
    else {
        tracing::warn!(
            provider = %model_ref.provider,
            model = %model_ref.model,
            "chat 模型未解析到配置条目，使用默认规格 32K/8K"
        );
        return ModelSpec::default();
    };
    let Some(entry) = provider.models.iter().find(|m| m.name == model_ref.model) else {
        tracing::warn!(
            provider = %model_ref.provider,
            model = %model_ref.model,
            "chat 模型未解析到配置条目，使用默认规格 32K/8K"
        );
        return ModelSpec::default();
    };
    resolve_model_spec(&model_ref.provider, entry)
}

/// 解析单个模型的上下文规格（显式 > 内置表 > 默认），并输出结构化日志。
///
/// [`resolve_chat_model_spec`] 与 GET /api/v1/config 的 model_specs 填充共用，
/// 是"单模型解析"的唯一实现。日志分支：
/// - 显式配置字段 → `info`（显式配置）
/// - 命中内置规格表 → `info`（内置表匹配）
/// - 均未命中 → `warn`（默认规格降级，建议显式声明）
pub(crate) fn resolve_model_spec(provider_name: &str, entry: &ModelEntry) -> ModelSpec {
    let explicit = tianyan::model::spec::from_entry_fields(
        entry.context_length,
        entry.max_output_tokens,
        entry.max_input_tokens,
    );
    let spec = tianyan::model::spec::resolve_spec(explicit, provider_name, &entry.name);
    match explicit {
        Some(_) => {
            tracing::info!(
                provider = provider_name,
                model = %entry.name,
                context_length = spec.context_length,
                max_output_tokens = spec.max_output_tokens,
                "模型规格：显式配置"
            );
        }
        None => match tianyan::model::spec::builtin_spec(provider_name, &entry.name) {
            Some(_) => {
                tracing::info!(
                    provider = provider_name,
                    model = %entry.name,
                    context_length = spec.context_length,
                    max_output_tokens = spec.max_output_tokens,
                    "模型规格：内置表匹配"
                );
            }
            None => {
                tracing::warn!(
                    provider = provider_name,
                    model = %entry.name,
                    "模型未匹配内置规格表且未配置 context_length/max_output_tokens，使用默认规格 32K/8K，建议在配置中显式声明"
                );
            }
        },
    }
    spec
}

/// 解析嵌入模型名（TextEmbedding 优先，回退 MultimodalEmbedding）。
pub(crate) fn resolve_embedding_model(config: &TianyanConfig) -> String {
    config
        .models
        .resolve(tianyan::config::ModelCapability::TextEmbedding)
        .or_else(|| {
            config
                .models
                .resolve(tianyan::config::ModelCapability::MultimodalEmbedding)
        })
        .map(|r| r.model)
        .unwrap_or_default()
}

/// 解析 Vision 能力模型名。
pub(crate) fn resolve_vision_model(config: &TianyanConfig) -> String {
    config
        .models
        .resolve(tianyan::config::ModelCapability::Vision)
        .map(|r| r.model)
        .unwrap_or_default()
}

/// 构造知识导入器（唯一构造点：state.rs 工厂与 agent_builder 共用）。
pub(crate) fn build_knowledge_ingestor(
    config: &TianyanConfig,
    model_services: &tianyan::model::ModelServices,
    vfs: Arc<dyn tianyan::vfs::VirtualFileSystem>,
) -> KnowledgeIngestor {
    let chat_model = resolve_chat_model(config);
    let embedding_model = resolve_embedding_model(config);
    let vision_model = resolve_vision_model(config);

    KnowledgeIngestor::new(
        IngestorConfig::new()
            .with_embedding_model(&embedding_model)
            .with_summary_model(&chat_model)
            .with_vision_model(&vision_model),
        model_services.chat.clone(),
        model_services.embedding.clone(),
        model_services.vision.clone(),
        vfs,
    )
}

// 类型别名
type SharedConfig = Arc<RwLock<TianyanConfig>>;
type SharedAgent = Arc<RwLock<Arc<dyn AgentCoordinator>>>;

/// 角色注册表会话边界刷新（ADR-016：学习产物新会话立即可见）。
///
/// 与角色注册表同构：新会话创建时把 VFS 中角色（学习产物）增量合并进
/// 注册表；会话内不刷新（前缀稳定，prompt 缓存不失效）。
#[derive(Clone)]
pub struct RoleSync {
    store: tianyan::role_store::RoleStore,
    registry: Arc<tianyan::agent::RoleRegistry>,
}

impl RoleSync {
    /// 将 VFS 角色增量合并进注册表，返回新增角色数。
    pub async fn refresh(&self) -> TianyanResult<usize> {
        self.registry.refresh_from_store(&self.store).await
    }
}

/// 共享应用状态
///
/// 管理服务器核心组件的生命周期，包括 Agent、会话管理器、虚拟文件系统和摘要服务。
///
/// 本模块只负责状态存储和访问，不包含任何构建逻辑。
/// 构建逻辑已委托给 [`AgentBuilderFactory`]。
#[derive(Clone)]
pub struct AppState {
    /// Core Agent 实例（支持热重载）
    agent: SharedAgent,
    /// 应用配置
    config: SharedConfig,
    /// 会话管理器
    session_manager: Arc<dyn SessionManager>,
    /// 虚拟文件系统（所有组件共享）
    vfs: Arc<VirtualFileSystemImpl>,
    /// 技能管理器（VFS 命名空间访问；技能列表/详情/执行 API 数据源）
    skill_manager: Arc<SkillManager>,
    /// 角色 VFS 存储（ADR-016：角色列表/详情/会话 API 的权威数据源）。
    role_store: tianyan::role_store::RoleStore,
    /// 角色注册表（ADR-016：与 Agent 共享同一 Arc；会话边界刷新目标）。
    role_registry: Arc<tianyan::agent::RoleRegistry>,
    /// 使用统计追踪器
    usage_stats: Arc<UsageStats>,
    /// 结构化 Trace 收集器（G6：span 持久化与回放查询）
    trace_collector: Arc<tianyan::observability::trace::TraceCollector>,
    /// 执行记录日志（ADR-017 GEPA 数据层：execution_stats 等统计工具）
    execution_log: Arc<ExecutionLog>,
    /// 会话回忆服务（ADR-017 决策 6：FTS5 消息索引）
    session_recall: Arc<SessionRecall>,
    /// 会话权威存储（ADR-018：SQLite 表为唯一真相；vfs_read 兼容层依赖）
    session_store: Arc<tianyan::session::store::SessionStore>,
    /// 会话工作集注册表（ADR-035：读侧缓存 + **写侧收口**——API 会话写路径
    /// 唯一通道；与 Agent 共享同一 Arc）。
    working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
    usage_log: Arc<UsageLog>,
    /// 工作区快照管理器（配置了 working_directory 时启用）
    /// 工作区快照管理器（配置了 working_directory 时启用）。
    ///
    /// `std::sync::RwLock`（同步、只瞬时持锁）：读多写少——每次请求读一次，
    /// 仅配置热重载写一次；保持 getter 同步以免波及全部调用点（T0-12：
    /// 热重载必须换成**新工作目录**的实例，不能沿用启动期实例）。
    snapshot_manager: Arc<std::sync::RwLock<Option<Arc<SnapshotManager>>>>,
    /// 模型服务（chat/embedding/vision，全组件共享；配置热更新时重建）
    model_services: Arc<RwLock<tianyan::model::ModelServices>>,
    /// MCP 客户端生命周期管理器（跨 agent reload 保持连接一致）
    mcp_tools: Arc<McpToolManager>,
    /// 定时任务调度器（无启用的模型 Provider 时为 None，在 `start_server` 中装配）
    scheduler: Arc<RwLock<Option<Arc<TaskScheduler>>>>,
    /// 定时智能体任务管理器（start_server 创建后装配；无 Provider 时为 None）。
    scheduled_agent_tasks: Arc<RwLock<Option<Arc<ScheduledAgentTaskManager>>>>,
    /// 待办清单存储（todos.json；用户跟踪多步任务）。
    todo_store: Arc<TodoStore>,
    /// 目标存储（goals.json；长期目标 + 进度跟踪）。
    goal_store: Arc<GoalStore>,
    /// 全系统共享 SQLite（ADR-005；后台任务持久化/唤醒，ADR-013）
    sqlite_db: Arc<Database>,
    /// 事件总线（T1 事件驱动：文件监听/webhook → 处理器/唤醒）
    event_bus: Arc<tianyan::events::EventBus>,
    /// 服务关停标志（Ctrl+C / SIGTERM / 桌面端退出时置位）。
    ///
    /// 流式请求据此取消进行中的 AgentLoop，使优雅关停不被长连接阻塞。
    shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
    /// 剪贴板 outbox（agent `clipboard_write` 工具 → tauri 轮询消费）。
    ///
    /// 挂载于 AppState：配置热更新只重建 agent，此状态跨请求/跨重载保持。
    clipboard_outbox: Arc<Mutex<Vec<String>>>,
    /// 剪贴板 pending（capture 存 → 前端确认（respond）消费）。
    clipboard_pending: Arc<RwLock<Option<PendingCapture>>>,
    /// 用户问题服务（ask_user 同步等待用户回答；回答端点提交入口）。
    user_questions: Arc<tianyan::agent::user_questions::UserQuestionService>,
    /// 子智能体消息流事件广播（ADR-026：GET /tasks/stream SSE 订阅源）。
    pub(crate) task_event_tx: tokio::sync::broadcast::Sender<String>,
    /// 活跃 SSE 对话流的取消句柄：session_id → cancel 标志（供显式取消端点触发）。
    /// 跑完再取：客户端断开不再取消 agent，主动「停止」经此端点显式取消。
    stream_cancels:
        Arc<Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    /// 服务器优雅关停信号发送端（数据目录搬迁等 API 触发重启用；
    /// start_server 装配，未装配时为 None）。
    shutdown_tx: Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
}

impl AppState {
    /// 创建新的应用状态
    ///
    /// 使用 AgentBuilderFactory 初始化所有组件。
    ///
    /// # Arguments
    /// * `config` - 应用配置
    /// * `vfs` - 虚拟文件系统实例
    ///
    /// # Returns
    /// * `TianyanResult<Self>` - 成功返回 AppState，失败返回错误
    ///
    /// # Errors
    /// * 配置验证失败时返回 InvalidConfig 错误
    /// * 模型服务注册失败时返回 ModelService 错误
    /// * Agent 构建失败时返回 AgentCreation 错误
    pub async fn new(
        config: TianyanConfig,
        vfs: Arc<VirtualFileSystemImpl>,
        database: Arc<Database>,
    ) -> TianyanResult<Self> {
        // VFS 技能管理器（技能 = VFS 方法论文档；列表/详情/执行 API 数据源）
        let skill_manager = Arc::new(SkillManager::new(vfs.clone()));

        // 初始化使用统计（复用全系统共享的 SqliteDb，ADR-005：单连接）
        let usage_stats = UsageStats::new(database.clone())?;

        // 初始化结构化 Trace（G6：span 持久化与回放；失败仅告警）
        let trace_collector = tianyan::observability::trace::TraceCollector::new(database.clone())
            .map_err(|e| TianyanError::Custom(format!("TraceCollector 初始化失败：{e}")))?;

        // 初始化执行记录日志（ADR-017 GEPA 数据层；共享 SqliteDb 连接）
        let execution_log = ExecutionLog::new(database.clone())?;

        // 初始化会话回忆服务（ADR-017 决策 6：FTS5 消息索引；共享 SqliteDb 连接）
        let session_recall = SessionRecall::new(database.clone())?;
        // 初始化会话权威存储（ADR-018：SQLite 表为唯一真相，VFS 会话例外）
        let session_store = tianyan::session::store::SessionStore::new(database.clone())?;

        // 会话工作集注册表（ADR-035：读侧缓存 + 写侧收口）——server 侧与 Agent
        // 共享**同一 Arc**：API 写路径（删消息/重做/标题/删除会话）必须经它，
        // 否则缓存与库分叉（一致性由单一写入口保证；`ensure` 校验仅作违规探测）。
        let working_sets =
            tianyan::agent::working_set::WorkingSetRegistry::new(Some(session_store.clone()));

        // 初始化 LLM 用量日志（token 统计；共享 SqliteDb 连接）
        let usage_log = UsageLog::new(database.clone())?;

        // 初始化工作区快照管理器（配置了 working_directory 时启用；
        // 与 reload_agent 共用同一构造助手）
        let snapshot_manager = build_snapshot_manager(&config);

        // MCP 工具桥接：连接配置中的 MCP 服务器，生成动态工具
        // （图片类返回，如浏览器截图，落盘到 {data_dir}/mcp_images/）
        let mcp_tools = Arc::new(McpToolManager::with_image_dir(
            config.storage.data_dir.join("mcp_images"),
        ));
        mcp_tools.sync(&config.mcp.servers).await;
        // 剪贴板 outbox（组合根收敛：ClipboardWriteTool 与 API 层共享同一实例；
        // 配置热更新只重建 agent，此状态跨请求/跨重载保持）
        let clipboard_outbox = Arc::new(Mutex::new(Vec::new()));
        let mut dynamic_tools = mcp_tools.bridges().await;
        dynamic_tools.push(Arc::new(ClipboardWriteTool::new(clipboard_outbox.clone())));
        if !dynamic_tools.is_empty() {
            tracing::info!(
                tool_count = dynamic_tools.len(),
                "动态工具已就绪（含剪贴板写入）"
            );
        }

        // 构建 Agent（传入 vfs + 技能组件 + 共享模型服务 + MCP 动态工具；
        // session_manager / trace_collector 与 API 层共享同一实例——组合根收敛）
        let model_services = create_model_services(&config)
            .await
            .map_err(model_services_error)?;
        // 嵌入用量入账（Agent 侧实例：摘要任务/角色路由等也调嵌入）。
        // provider 名由全量 config 解析（与 VFS 侧同策略、同表）。
        crate::embedding_usage::attach_embedding_usage_sink(
            &model_services,
            usage_log.clone(),
            &config,
        );
        // ADR-016：角色注册表从 VFS 加载（内置种子 + 配置签名 upsert + 演化实体）
        let role_store = tianyan::role_store::RoleStore::new(vfs.clone());
        let role_registry = Arc::new(
            tianyan::agent::RoleRegistry::load_persisted(&role_store, &config.agent_roles).await?,
        );
        // ADR-016 P3：角色向量路由（suggest_role 工具）
        let role_router = Arc::new(tianyan::agent::RoleRouter::new(
            role_store.clone(),
            model_services.embedding.clone(),
            resolve_embedding_model(&config),
        ));

        // 创建持久化会话管理器（唯一实例：Agent 与 API 层共享，避免双写；
        // ADR-018：基于 SessionStore（SQLite 权威存储），append 时事务内同步 FTS）
        let session_manager = Arc::new(PersistentSessionManager::new(session_store.clone()));
        // 用户问题服务（ask_user 同步等待用户回答；回答端点与 Agent 共享同一实例）
        let user_questions = Arc::new(tianyan::agent::user_questions::UserQuestionService::new());
        // 子智能体消息流事件广播（ADR-026：面板实时流式；SSE 订阅源）
        let (task_event_tx, _) = tokio::sync::broadcast::channel::<String>(256);
        let task_event_sink: Arc<dyn tianyan::agent::background::TaskEventSink> =
            Arc::new(crate::agent_builder::TaskEventBroadcaster {
                tx: task_event_tx.clone(),
            });
        // ADR-028/031：会话管理器包装（消息不再广播——流式增量 + 完成事件
        // 到前端；任务状态/命令输出事件仍经统一通道）
        let session_manager: Arc<dyn SessionManager> =
            Arc::new(crate::event_push::BroadcastingSessionManager::new(
                session_manager,
                task_event_tx.clone(),
            ));
        // ADR-035 §3 补记：写侧收口后所有写入经工作集（`ws.append`），不再经
        // `SessionManager` wrapper——边界消息（System 通知 / 压缩点）的
        // 「落库即推送」改挂在工作集 `append` 上（同一门控与映射；core 不感知
        // 通道，回调由本装配层注入）。支持**后注入**：对已加载与后续新建的
        // 工作集一并生效（共享回调槽）。
        {
            let tx = task_event_tx.clone();
            working_sets.set_boundary_push(Arc::new(
                move |sid: &str, msg: &tianyan::common::types::StructuredMessage| {
                    crate::event_push::push_boundary_event(&tx, sid, msg);
                },
            ));
        }
        let agent = AgentBuilderFactory::build_agent_or_wizard(
            &config,
            model_services.clone(),
            vfs.clone(),
            usage_stats.clone(),
            snapshot_manager.clone(),
            dynamic_tools,
            Some(database.clone()),
            crate::notification::global_notification_sink(),
            session_manager.clone(),
            Some(trace_collector.clone()),
            execution_log.clone(),
            session_recall.clone(),
            session_store.clone(),
            role_registry.clone(),
            role_router,
            usage_log.clone(),
            user_questions.clone(),
            task_event_sink,
            task_event_tx.clone(),
            working_sets.clone(),
        )
        .await?;

        let data_dir = config.storage.data_dir.clone();
        Ok(Self {
            agent: Arc::new(RwLock::new(agent)),
            config: Arc::new(RwLock::new(config)),
            session_manager,
            vfs,
            skill_manager,
            role_store,
            role_registry,
            usage_stats,
            trace_collector,
            execution_log,
            working_sets,
            session_recall,
            session_store,
            usage_log,
            snapshot_manager: Arc::new(std::sync::RwLock::new(snapshot_manager)),
            model_services: Arc::new(RwLock::new(model_services)),
            mcp_tools,
            scheduler: Arc::new(RwLock::new(None)),
            scheduled_agent_tasks: Arc::new(RwLock::new(None)),
            todo_store: Arc::new(TodoStore::new(&data_dir)),
            goal_store: Arc::new(GoalStore::new(&data_dir)),
            sqlite_db: database,
            event_bus: Arc::new(tianyan::events::EventBus::new()),
            shutdown_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_event_tx,
            clipboard_outbox,
            clipboard_pending: Arc::new(RwLock::new(None)),
            user_questions,
            stream_cancels: Arc::new(Mutex::new(std::collections::HashMap::new())),
            shutdown_tx: Arc::new(Mutex::new(None)),
        })
    }

    /// 获取剪贴板 outbox 句柄。
    pub fn clipboard_outbox(&self) -> Arc<Mutex<Vec<String>>> {
        self.clipboard_outbox.clone()
    }

    /// 获取剪贴板 pending 句柄。
    pub fn clipboard_pending(&self) -> Arc<RwLock<Option<PendingCapture>>> {
        self.clipboard_pending.clone()
    }

    /// 获取用户问题服务（ask_user 回答提交入口）。
    pub fn user_questions(&self) -> Arc<tianyan::agent::user_questions::UserQuestionService> {
        self.user_questions.clone()
    }

    /// 装配服务器优雅关停信号发送端（start_server 创建 watch 通道后调用）。
    pub fn attach_shutdown_tx(&self, tx: tokio::sync::watch::Sender<bool>) {
        *self.shutdown_tx.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
    }

    /// 请求服务器优雅关停（数据目录搬迁等场景；未装配时静默）。
    pub fn request_shutdown(&self) {
        if let Some(tx) = self
            .shutdown_tx
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            let _ = tx.send(true);
        }
    }

    /// 获取活跃对话流取消注册表（session_id → cancel 标志）。
    pub fn stream_cancels(
        &self,
    ) -> Arc<Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>> {
        self.stream_cancels.clone()
    }

    /// 尝试为会话注册流取消标志（T1-10 同会话并发保护）。
    ///
    /// 同一会话已有活跃流时返回 `Err(ApiError::Conflict)`——此前直接覆盖取消槽，
    /// 旧流从此取消不到（「停止」失效）且两轮流并发写同一会话。检查与插入在
    /// 同一把锁内完成（原子）。
    pub fn try_register_stream(
        &self,
        session_id: &str,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), crate::api::shared::error::ApiError> {
        let mut registry = self
            .stream_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        register_stream_slot(&mut registry, session_id, cancel)
    }

    /// 服务关停标志（Ctrl+C / SIGTERM / 桌面端退出时置位）。
    pub fn shutdown_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.shutdown_flag.clone()
    }

    /// 获取当前 Agent 实例
    ///
    /// # Returns
    /// * `Arc<dyn AgentCoordinator>` - 当前 Agent 实例的 Arc 引用
    pub async fn agent(&self) -> Arc<dyn AgentCoordinator> {
        self.agent.read().await.clone()
    }

    /// 获取 Agent 共享句柄（配置热重载后自动指向新实例；演化综述执行器用）。
    pub fn agent_lock(&self) -> Arc<RwLock<Arc<dyn AgentCoordinator>>> {
        self.agent.clone()
    }

    /// 获取角色注册表同步句柄（ADR-016：新会话创建时刷新学习角色）。
    pub fn role_sync(&self) -> RoleSync {
        RoleSync {
            store: self.role_store.clone(),
            registry: self.role_registry.clone(),
        }
    }

    /// 角色 VFS 存储（角色 API 的权威数据源）。
    pub fn role_store(&self) -> tianyan::role_store::RoleStore {
        self.role_store.clone()
    }

    /// 获取结构化 Trace 收集器（G6 回放查询）。
    pub fn trace_collector(&self) -> Arc<tianyan::observability::trace::TraceCollector> {
        self.trace_collector.clone()
    }

    /// 更新应用配置并重新构建 Agent
    ///
    /// # Arguments
    /// * `config` - 新的配置
    ///
    /// # Returns
    /// * `TianyanResult<()>` - 成功返回 Ok，失败返回错误
    ///
    /// # Errors
    /// * Agent 构建失败时返回相应错误
    pub async fn update_config(&self, config: TianyanConfig) -> TianyanResult<()> {
        *self.config.write().await = config;
        self.reload_agent().await
    }

    /// 重新初始化 Agent（配置更新后调用）
    ///
    /// # Returns
    /// * `TianyanResult<()>` - 成功返回 Ok，失败返回错误
    ///
    /// # Errors
    /// * Agent 构建失败时返回相应错误
    pub async fn reload_agent(&self) -> TianyanResult<()> {
        let config = self.config.read().await.clone();
        // MCP 配置可能已变化：对齐连接（断开移除的、连接新增的），重建动态工具
        self.mcp_tools.sync(&config.mcp.servers).await;
        let mut dynamic_tools = self.mcp_tools.bridges().await;
        // 剪贴板写入工具随 agent 重建重新注入（复用同一 outbox 实例，
        // tauri 轮询路径不因热重载中断）
        dynamic_tools.push(Arc::new(ClipboardWriteTool::new(self.clipboard_outbox())));
        // 配置可能已变化（模型/API Key），重建共享模型服务
        let model_services = create_model_services(&config)
            .await
            .map_err(model_services_error)?;
        // 嵌入用量入账（Agent 侧实例：摘要任务/角色路由等也调嵌入）。
        // provider 名由全量 config 解析（与 VFS 侧同策略、同表）。
        crate::embedding_usage::attach_embedding_usage_sink(
            &model_services,
            self.usage_log(),
            &config,
        );
        // 同步更新 VFS 嵌入服务（热重载后语义检索立即生效，无需重启）
        self.vfs.set_embedding_provider(
            model_services.embedding.clone(),
            resolve_embedding_model(&config),
        );
        // ADR-016：配置热重载时应用持久化状态（配置签名变更 → 用户种子生效）
        self.role_registry
            .apply_persisted(&self.role_store, &config.agent_roles)
            .await?;
        // ADR-016 P3：热重载重建路由器（embedding 模型可能变化）
        let role_router = Arc::new(tianyan::agent::RoleRouter::new(
            self.role_store.clone(),
            model_services.embedding.clone(),
            resolve_embedding_model(&config),
        ));
        let new_snapshot_manager = build_snapshot_manager(&config);
        let new_agent = AgentBuilderFactory::build_agent_or_wizard(
            &config,
            model_services.clone(),
            self.vfs.clone(),
            self.usage_stats.clone(),
            new_snapshot_manager.clone(),
            dynamic_tools,
            Some(self.sqlite_db.clone()),
            crate::notification::global_notification_sink(),
            self.session_manager.clone(),
            Some(self.trace_collector.clone()),
            self.execution_log.clone(),
            self.session_recall.clone(),
            self.session_store.clone(),
            self.role_registry.clone(),
            role_router,
            self.usage_log.clone(),
            self.user_questions.clone(),
            Arc::new(crate::agent_builder::TaskEventBroadcaster {
                tx: self.task_event_tx.clone(),
            }),
            self.task_event_tx.clone(),
            // 热重载复用**同一**工作集（状态不能在 reload 时分裂：新 Agent
            // 必须看到旧 Agent 已物化的会话缓存）
            self.working_sets.clone(),
        )
        .await?;

        *self.agent.write().await = new_agent;
        *self.model_services.write().await = model_services;
        // T0-12：快照管理器随新配置替换（builder 成功后才提交，避免半更新
        // 状态——旧实现根本不重建，快照/回退停留在旧工作目录）
        *self
            .snapshot_manager
            .write()
            .unwrap_or_else(|e| e.into_inner()) = new_snapshot_manager;

        tracing::info!("Agent 重新加载成功");
        Ok(())
    }

    /// 获取会话管理器
    ///
    /// # Returns
    /// * `Arc<dyn SessionManager>` - 会话管理器实例
    pub fn session_manager(&self) -> Arc<dyn SessionManager> {
        self.session_manager.clone()
    }

    /// 获取会话权威存储（ADR-028：断点对齐增量查询）。
    pub fn session_store(&self) -> Arc<tianyan::session::store::SessionStore> {
        self.session_store.clone()
    }
    /// 获取会话工作集注册表（ADR-035：API 写路径收口唯一通道）。
    pub fn working_sets(&self) -> Arc<tianyan::agent::working_set::WorkingSetRegistry> {
        self.working_sets.clone()
    }

    /// 获取虚拟文件系统
    ///
    /// # Returns
    /// * `Arc<VirtualFileSystemImpl>` - VFS 实例
    pub fn vfs(&self) -> Arc<VirtualFileSystemImpl> {
        self.vfs.clone()
    }

    /// 获取技能管理器（VFS 技能命名空间访问；技能 API 数据源）。
    pub fn skill_manager(&self) -> Arc<SkillManager> {
        self.skill_manager.clone()
    }

    /// 获取工作区快照管理器（未配置 working_directory 时为 None）。
    pub fn snapshot_manager(&self) -> Option<Arc<SnapshotManager>> {
        self.snapshot_manager
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 获取全局默认工作目录（[agent] working_directory 配置；动态读取，
    /// 配置热更新后自动指向新值）。
    pub async fn agent_working_directory(&self) -> Option<std::path::PathBuf> {
        self.config
            .read()
            .await
            .agent
            .working_directory
            .clone()
            .map(std::path::PathBuf::from)
    }

    /// 获取配置
    ///
    /// # Returns
    /// * `Arc<RwLock<TianyanConfig>>` - 配置实例
    pub fn config(&self) -> Arc<RwLock<TianyanConfig>> {
        self.config.clone()
    }

    /// 获取使用统计追踪器
    ///
    /// # Returns
    /// * `Arc<UsageStats>` - 使用统计追踪器实例
    pub fn usage_stats(&self) -> Arc<UsageStats> {
        self.usage_stats.clone()
    }

    /// 获取执行记录日志（ADR-017 GEPA 数据层）。
    pub fn execution_log(&self) -> Arc<ExecutionLog> {
        self.execution_log.clone()
    }

    /// 获取会话回忆服务（ADR-017 决策 6）。
    pub fn session_recall(&self) -> Arc<SessionRecall> {
        self.session_recall.clone()
    }

    /// LLM 用量日志访问器（token 统计 API 依赖）。
    pub fn usage_log(&self) -> Arc<UsageLog> {
        self.usage_log.clone()
    }

    /// 装配定时任务调度器（`start_server` 在创建并注册任务后调用）。
    pub async fn attach_scheduler(&self, scheduler: Option<Arc<TaskScheduler>>) {
        *self.scheduler.write().await = scheduler;
    }

    /// 获取定时任务调度器（未装配或无 Provider 时为 None）。
    pub async fn scheduler(&self) -> Option<Arc<TaskScheduler>> {
        self.scheduler.read().await.clone()
    }

    /// 装配定时智能体任务管理器（start_server 创建后调用）。
    pub async fn attach_scheduled_agent_tasks(&self, m: Option<Arc<ScheduledAgentTaskManager>>) {
        *self.scheduled_agent_tasks.write().await = m;
    }

    /// 获取定时智能体任务管理器（未装配时为 None）。
    pub async fn scheduled_agent_tasks(&self) -> Option<Arc<ScheduledAgentTaskManager>> {
        self.scheduled_agent_tasks.read().await.clone()
    }

    /// 获取待办清单存储。
    pub fn todo_store(&self) -> Arc<TodoStore> {
        self.todo_store.clone()
    }

    /// 获取目标存储。
    pub fn goal_store(&self) -> Arc<GoalStore> {
        self.goal_store.clone()
    }

    /// 获取共享模型服务（配置热更新后自动指向新实例）。
    pub(crate) async fn shared_model_services(
        &self,
    ) -> TianyanResult<tianyan::model::ModelServices> {
        Ok(self.model_services.read().await.clone())
    }

    /// 获取事件总线（T1 事件驱动：webhook/文件监听发布）。
    pub fn event_bus(&self) -> Arc<tianyan::events::EventBus> {
        self.event_bus.clone()
    }

    /// 创建摘要引擎（复用共享模型服务，不重复创建 HTTP 客户端）。
    ///
    /// # Returns
    /// * `TianyanResult<Arc<SummaryEngine>>` - 摘要引擎实例
    pub async fn create_summary_engine(&self) -> TianyanResult<Arc<SummaryEngine>> {
        let config = self.config.read().await.clone();
        let model_services = self.shared_model_services().await?;
        let chat_model = resolve_chat_model(&config);
        // 创建 SummaryEngine
        let summary_engine = SummaryEngine::new(model_services.chat, &chat_model);

        Ok(Arc::new(summary_engine))
    }

    /// 创建记忆提取器（复用共享模型服务）。
    ///
    /// # Returns
    /// * `TianyanResult<Arc<MemoryExtractor>>` - 记忆提取器实例
    pub async fn create_memory_extractor(&self) -> TianyanResult<Arc<MemoryExtractor>> {
        let config = self.config.read().await.clone();
        let model_services = self.shared_model_services().await?;
        let chat_model = resolve_chat_model(&config);

        let extractor = MemoryExtractor::new(
            model_services.chat,
            ExtractionConfig {
                model: chat_model,
                ..ExtractionConfig::default()
            },
        );
        Ok(Arc::new(extractor))
    }

    /// 创建技能使用评审器（记忆提取同周期复审；chat + vfs + 聊天模型）。
    pub async fn create_skill_reviewer(
        &self,
    ) -> TianyanResult<Arc<tianyan::skills::SkillReviewer>> {
        let config = self.config.read().await.clone();
        let model_services = self.shared_model_services().await?;
        let chat_model = resolve_chat_model(&config);
        Ok(Arc::new(tianyan::skills::SkillReviewer::new(
            model_services.chat,
            self.vfs.clone(),
            chat_model,
        )))
    }

    /// 创建知识导入器（复用共享模型服务与唯一构造点 [`build_knowledge_ingestor`]）。
    pub async fn create_knowledge_ingestor(&self) -> TianyanResult<KnowledgeIngestor> {
        let config = self.config.read().await.clone();
        let model_services = self.shared_model_services().await?;
        let vfs: Arc<dyn tianyan::vfs::VirtualFileSystem> = self.vfs.clone();

        Ok(build_knowledge_ingestor(&config, &model_services, vfs))
    }

    /// 优雅关闭应用
    ///
    /// 关闭应用。
    ///
    /// # Returns
    /// * `TianyanResult<()>` - 成功返回 Ok
    ///
    /// # Errors
    /// * 正常情况下不会返回错误
    pub async fn shutdown(&self) -> TianyanResult<()> {
        tracing::info!("开始关闭应用...");
        // 主动中止运行中的轮（退出不再干等）：置位所有会话取消标志——AgentLoop
        // 在轮顶/流式 chunk/工具执行边界立即收尾。否则活跃的 /chat/stream 连接
        // 会拖住 axum 优雅关停（托盘退出卡死的根因：日志说"退出应用"但进程不退）。
        let cancelled = self.agent().await.cancel_all_active_turns().await;
        if cancelled > 0 {
            tracing::info!(count = cancelled, "已请求中止运行中的轮");
        }
        // 杀掉运行中的命令子进程（前台 + 后台统一；父进程退出不带走子进程——
        // 用户实测退出后任务管理器仍残留 cargo 进程）。取消标志覆盖不到后台命令，
        // 这里显式按进程树清理。
        let killed = tianyan::executor::kill_all_running_children().await;
        if killed > 0 {
            tracing::info!(count = killed, "已终止运行中的命令子进程");
        }
        // 断开所有 MCP 服务器连接（显式 shutdown，McpClient::drop 不会自动清理子进程）
        self.mcp_tools.shutdown().await;
        // 使用统计落盘（flush + PRAGMA optimize；失败不阻断关闭）
        if let Err(e) = self.usage_stats.shutdown().await {
            tracing::warn!(error = %e, "使用统计刷盘失败");
        }
        // 显式清理定时任务管理器与调度器引用（打破 manager ↔ scheduler 循环）
        *self.scheduled_agent_tasks.write().await = None;
        *self.scheduler.write().await = None;
        tracing::info!("应用已关闭");
        Ok(())
    }
}

/// 注册流取消槽（T1-10 同会话并发保护核心逻辑；纯函数便于单测）。
///
/// 已存在同会话槽 → `Err(Conflict)`（**不覆盖**；覆盖会让旧流「停止」失效）；
/// 否则插入并 `Ok`。
pub(crate) fn register_stream_slot(
    registry: &mut std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>,
    session_id: &str,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), crate::api::shared::error::ApiError> {
    if registry.contains_key(session_id) {
        return Err(crate::api::shared::error::ApiError::Conflict(format!(
            "会话 {session_id} 已有进行中的对话流，请等待完成或先停止"
        )));
    }
    registry.insert(session_id.to_string(), cancel);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tianyan::config::{
        find_provider, ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig,
        ProviderConfig,
    };

    #[test]
    fn test_register_stream_slot_rejects_duplicate_session() {
        // T1-10：同会话第二次注册必须 409（不覆盖旧槽——覆盖会让旧流「停止」失效）
        let mut registry = std::collections::HashMap::new();
        let first = Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(register_stream_slot(&mut registry, "s1", first.clone()).is_ok());

        let second = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let err = register_stream_slot(&mut registry, "s1", second.clone()).unwrap_err();
        assert!(
            matches!(err, crate::api::shared::error::ApiError::Conflict(_)),
            "同会话重复流应为 409 冲突：{err:?}"
        );
        // 旧槽未被覆盖：置位它仍作用于**旧**流（「停止」不失效）
        assert!(
            registry
                .get("s1")
                .map(|c| Arc::ptr_eq(c, &first))
                .unwrap_or(false),
            "注册冲突时不得覆盖既有取消槽"
        );
        // 不同会话互不影响
        assert!(register_stream_slot(&mut registry, "s2", second).is_ok());
        // 流结束移除后可再次注册（与 handlers 的 cleanup 对应）
        registry.remove("s1");
        assert!(
            register_stream_slot(
                &mut registry,
                "s1",
                Arc::new(std::sync::atomic::AtomicBool::new(false))
            )
            .is_ok(),
            "流结束（槽移除）后应可重新注册"
        );
    }

    fn provider_with(models: Vec<ModelEntry>) -> ProviderConfig {
        ProviderConfig {
            name: "deepseek".to_string(),
            endpoint: "https://api.deepseek.com/v1".to_string(),
            api_key: Some("sk-test".to_string()),
            models,
            timeout: 60,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
        }
    }

    fn chat_entry(name: &str) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            capabilities: vec![ModelCapability::Chat],
            ..Default::default()
        }
    }

    fn config_with(models: ModelsConfig) -> TianyanConfig {
        TianyanConfig {
            models,
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_chat_model_spec_explicit_fields_win() {
        // 显式三字段 → 返回显式 spec（不走内置表/默认）
        let entry = ModelEntry {
            context_length: Some(64_000),
            max_output_tokens: Some(4_000),
            max_input_tokens: Some(60_000),
            ..chat_entry("deepseek-v4-flash")
        };
        let models = ModelsConfig {
            providers: vec![provider_with(vec![entry])],
            preferences: ModelPreferences {
                chat: Some(ModelRef {
                    provider: "deepseek".to_string(),
                    model: "deepseek-v4-flash".to_string(),
                }),
                ..Default::default()
            },
        };
        let spec = resolve_chat_model_spec(&config_with(models));
        assert_eq!(spec.context_length, 64_000);
        assert_eq!(spec.max_output_tokens, 4_000);
        assert_eq!(spec.max_input_tokens, 60_000);
    }

    #[test]
    fn test_resolve_chat_model_spec_partial_explicit_fields() {
        // 仅 context_length 显式 → 其余字段用默认值（from_entry_fields 语义）
        let entry = ModelEntry {
            context_length: Some(128_000),
            ..chat_entry("my-chat-model")
        };
        let models = ModelsConfig {
            providers: vec![provider_with(vec![entry])],
            preferences: ModelPreferences {
                chat: Some(ModelRef {
                    provider: "deepseek".to_string(),
                    model: "my-chat-model".to_string(),
                }),
                ..Default::default()
            },
        };
        let spec = resolve_chat_model_spec(&config_with(models));
        assert_eq!(spec.context_length, 128_000);
        assert_eq!(spec.max_output_tokens, 8_192);
        assert_eq!(spec.max_input_tokens, 8_192);
    }

    #[test]
    fn test_resolve_chat_model_spec_builtin_table_hit() {
        // 无显式字段 + 内置表命中（deepseek / deepseek-v4-flash → 1M 窗口）
        let models = ModelsConfig {
            providers: vec![provider_with(vec![chat_entry("deepseek-v4-flash")])],
            preferences: ModelPreferences::default(),
        };
        let spec = resolve_chat_model_spec(&config_with(models));
        assert_eq!(spec.context_length, 1_000_000);
        assert_eq!(spec.max_output_tokens, 32_000);
        assert_eq!(spec.max_input_tokens, 968_000);
    }

    #[test]
    fn test_resolve_chat_model_spec_unknown_model_default() {
        // 无显式字段 + 内置表未命中 → 全局默认 32_768/8_192/8_192
        let models = ModelsConfig {
            providers: vec![provider_with(vec![chat_entry("deepseek-chat")])],
            preferences: ModelPreferences::default(),
        };
        let spec = resolve_chat_model_spec(&config_with(models));
        assert_eq!(spec, ModelSpec::default());
    }

    #[test]
    fn test_resolve_chat_model_spec_no_chat_model_default() {
        // 无任何 chat 能力模型 → 默认 spec（不 panic）
        let models = ModelsConfig::default();
        let spec = resolve_chat_model_spec(&config_with(models));
        assert_eq!(spec, ModelSpec::default());

        // preference 指向不存在的模型 → 回退默认（resolve 自动选择亦失败）
        let models = ModelsConfig {
            providers: vec![provider_with(vec![])],
            preferences: ModelPreferences {
                chat: Some(ModelRef {
                    provider: "deepseek".to_string(),
                    model: "ghost".to_string(),
                }),
                ..Default::default()
            },
        };
        let spec = resolve_chat_model_spec(&config_with(models));
        assert_eq!(spec, ModelSpec::default());
    }

    #[test]
    fn test_resolve_model_spec_explicit_builtin_default() {
        // 显式字段优先（provider 无关）
        let explicit_entry = ModelEntry {
            name: "custom-chat".to_string(),
            context_length: Some(64_000),
            max_output_tokens: Some(4_000),
            max_input_tokens: Some(60_000),
            ..Default::default()
        };
        let spec = resolve_model_spec("openai", &explicit_entry);
        assert_eq!(spec.context_length, 64_000);
        assert_eq!(spec.max_output_tokens, 4_000);
        assert_eq!(spec.max_input_tokens, 60_000);

        // 无显式 + 内置表命中（deepseek / deepseek-v4-flash → 1M 窗口）
        let builtin_entry = chat_entry("deepseek-v4-flash");
        let spec = resolve_model_spec("deepseek", &builtin_entry);
        assert_eq!(spec.context_length, 1_000_000);
        assert_eq!(spec.max_output_tokens, 32_000);
        assert_eq!(spec.max_input_tokens, 968_000);

        // 无显式 + 内置表未命中 → 全局默认 32_768/8_192/8_192
        let unknown_entry = chat_entry("my-local-model");
        let spec = resolve_model_spec("ollama", &unknown_entry);
        assert_eq!(spec, ModelSpec::default());
    }

    #[test]
    fn test_find_provider_lookup_used_by_resolve() {
        // 确认 entry 回查使用的 find_provider 语义：仅命中已启用 provider
        let models = ModelsConfig {
            providers: vec![provider_with(vec![chat_entry("deepseek-v4-flash")])],
            ..Default::default()
        };
        let p = find_provider(&models.providers, "deepseek").unwrap();
        assert_eq!(p.models[0].name, "deepseek-v4-flash");
    }
}
