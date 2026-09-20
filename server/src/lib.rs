//! Tianyan HTTP 服务器。

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use axum::http::HeaderValue;
use axum::{extract::DefaultBodyLimit, response::Json, routing::get, Router};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::watch;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

use tianyan::observability::usage_log::UsageLog;
use tianyan::scheduler::tasks::{
    EvolutionTask, GcTask, SnapshotGcTask, SummaryTask, UsageStatsFlushTask,
};
use tianyan::scheduler::{TaskContext, TaskDefinition, TaskScheduler};
use tianyan::vfs::VirtualFileSystem;

use crate::agent_builder::create_model_services;
use crate::api::events::processor as event_processor;
use crate::api::goals::tool::GoalTool;
use crate::api::todos::tool::TodoTool;
use crate::embedding_usage::attach_embedding_usage_sink;
use crate::evolution_executor::AgentEvolutionExecutor;
use crate::scheduled_tasks::manager::{ScheduledAgentTaskManager, SchedulerRegistrar};
use crate::scheduled_tasks::tool::{CreateTaskRequest, ScheduleTaskTool};

// Import API module
pub mod agent_builder;
pub mod api;
/// 嵌入用量入账（usage_logs）。
pub mod embedding_usage;
pub mod event_push;
pub mod evolution_executor;
pub mod mcp_bridge;
pub mod migration;
pub mod notification;
pub mod scheduled_tasks;
pub mod state;

use api::create_api_router;
use state::AppState;

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 服务器端口号
    pub port: u16,
    /// 服务器主机地址
    pub host: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "127.0.0.1".to_string(),
        }
    }
}

impl ServerConfig {
    /// Create a new server config with custom port
    pub fn with_port(port: u16) -> Self {
        Self {
            port,
            ..Default::default()
        }
    }

    /// Create a new server config with custom host and port
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// 从环境变量构造：`TIANYAN_PORT` 可覆盖端口（未设 / 非法 → 默认 3000）。
    ///
    /// 用途：e2e 用**备用端口**启动后端，刻意避开 3000——本地 3000 常被运行中的
    /// 桌面应用（`tianyan-tauri`）占用，强占端口会掐断用户正在进行的会话。
    /// 独立 `tianyan-server` 的用户也可借此换端口（不设该变量时行为完全不变）。
    pub fn from_env() -> Self {
        let raw = std::env::var("TIANYAN_PORT").ok();
        let config = Self::from_port_str(raw.as_deref());
        match raw.as_deref() {
            Some(value) if value.trim().parse::<u16>().is_err() => {
                tracing::warn!(
                    "TIANYAN_PORT 非法（{}），回退默认端口 {}",
                    value,
                    config.port
                );
            }
            Some(_) => tracing::info!("服务器端口经 TIANYAN_PORT 覆盖为 {}", config.port),
            None => {}
        }
        config
    }

    /// 端口字符串 → 配置（纯函数，便于判别力测试）：`None` / 非法值回退默认端口。
    fn from_port_str(port: Option<&str>) -> Self {
        match port.and_then(|value| value.trim().parse::<u16>().ok()) {
            Some(port) => Self::with_port(port),
            None => Self::default(),
        }
    }
}

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    /// 健康状态
    pub status: String,
    /// 服务版本号
    pub version: String,
}

/// Health check handler
async fn health_check() -> Json<HealthResponse> {
    let version = env!("CARGO_PKG_VERSION").to_string();

    Json(HealthResponse {
        status: "ok".to_string(),
        version,
    })
}

/// Get the static files directory path
///
/// This function tries to find the gui-vite/dist directory from various possible locations
fn get_static_dir() -> std::path::PathBuf {
    // 基准目录：当前工作目录（支持从项目根或 target/{debug,release} 启动）
    let current_dir = std::env::current_dir().unwrap_or_default();

    // Check if we're running from target/release or target/debug
    let possible_paths = [
        // From target/release or target/debug - go up 3 levels to project root
        current_dir.join("../../..").join("gui-vite/dist"),
        // Direct from project root
        current_dir.join("gui-vite/dist"),
        // From tauri directory
        current_dir.join("../gui-vite/dist"),
        // Legacy gui/dist layouts (kept for backward compatibility)
        current_dir.join("../../..").join("gui/dist"),
        current_dir.join("gui/dist"),
        current_dir.join("../gui/dist"),
    ];

    for path in &possible_paths {
        if path.exists() && path.join("index.html").exists() {
            return path.clone();
        }
    }

    // Default fallback - use the first option and let it fail gracefully if not found
    possible_paths[0].clone()
}

/// 应用级 VFS 初始化
///
/// 创建并初始化单一的 VFS 实例，供整个应用使用。
/// `sqlite_db` 为全系统共享连接（ADR-005：禁止第二个 SQLite 连接）。
async fn initialize_vfs_for_app(
    config: &tianyan::config::TianyanConfig,
    database: Arc<tianyan::db::Database>,
) -> tianyan::common::error::Result<Arc<tianyan::vfs::VirtualFileSystemImpl>> {
    use tianyan::config::StorageBackendType;
    use tianyan::vfs::{
        LanceDbVectorStore, SqliteBackend, StorageBackend, VirtualFileSystemBuilder,
    };

    // 1. 创建存储后端（config.storage.backend 决定 adapter）
    let storage: Arc<dyn StorageBackend> = match config.storage.backend {
        StorageBackendType::Local => {
            Arc::new(tianyan::vfs::LocalFileBackend::new(config.storage.clone()))
        }
        StorageBackendType::Sqlite => {
            info!(
                "VFS 使用 SQLite 存储后端（共享连接）：{}",
                database.path().display()
            );
            // clone：database 还需用于嵌入用量入账（UsageLog 共享同一连接）
            Arc::new(SqliteBackend::new(database.clone()))
        }
    };

    // LanceDB 需要 async 初始化
    let vector_storage = LanceDbVectorStore::new(&config.storage).await?;
    let vector_storage = Arc::new(vector_storage);

    // 2. 创建模型服务（用于嵌入服务），无配置时跳过
    let mut vfs_builder = VirtualFileSystemBuilder::new()
        .with_storage(storage)
        .with_vector_storage(vector_storage)
        .with_config(config.storage.clone());
    {
        let model_services = create_model_services(config).await;
        match model_services {
            Ok(ms) => {
                let emb_model = config
                    .models
                    .resolve(tianyan::config::ModelCapability::TextEmbedding)
                    .map(|r| r.model)
                    .unwrap_or_else(|| "text-embedding-3-small".to_string());
                // 嵌入用量入账（此前嵌入 token 完全不入 usage_logs：账单/统计盲区）。
                // 失败只告警：嵌入照常工作，仅不入账（不阻断启动链）。
                match UsageLog::new(database.clone()) {
                    Ok(usage_log) => {
                        attach_embedding_usage_sink(&ms, usage_log, config);
                    }
                    Err(e) => warn!("嵌入用量入账不可用（{}），嵌入调用将不入账", e),
                }
                // 直接注入 EmbeddingService（无桥接层）
                vfs_builder = vfs_builder.with_embedding_provider(ms.embedding, emb_model);
            }
            Err(e) => {
                warn!("无法创建模型服务（{}），VFS 将以无嵌入模式运行", e);
            }
        }
    }

    // 3. 初始化 VFS（单一实例）
    let vfs = vfs_builder.build().map_err(|e| {
        tianyan::TianyanError::Custom(format!("虚拟文件系统错误：VFS 构建失败：{}", e))
    })?;

    use tianyan::vfs::VfsCore;
    vfs.initialize().await?;

    bootstrap_app_vfs(&vfs).await?;

    Ok(Arc::new(vfs))
}

async fn bootstrap_app_vfs(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
) -> Result<(), tianyan::common::error::TianyanError> {
    use tianyan::agent::DEFAULT_SOUL;
    use tianyan::common::types::{AgentPath, ContentLevel, ContextNamespace, TianyanUri};
    use tianyan::vfs::{ContentStore, VfsCore};

    let soul_uri = AgentPath::Soul.uri();
    if !vfs
        .has_content(&soul_uri, ContentLevel::Detail)
        .await
        .unwrap_or(false)
    {
        vfs.create_file(&soul_uri).await?;
        vfs.write(&soul_uri, ContentLevel::Detail, DEFAULT_SOUL)
            .await?;
        tracing::info!("已创建默认核心提示词： tianyan://agent/soul");
    }

    let learned_uri = AgentPath::Learned.uri();
    if !vfs.exists(&learned_uri).await? {
        vfs.create_directory(&learned_uri).await?;
        tracing::info!("已创建学习规则目录： tianyan://agent/learned");
    }

    let user_uri = TianyanUri::new(ContextNamespace::User, vec![]);
    if !vfs
        .has_content(&user_uri, ContentLevel::Detail)
        .await
        .unwrap_or(false)
    {
        vfs.write(&user_uri, ContentLevel::Detail, "# 用户档案\n\n")
            .await?;
        tracing::info!("已创建默认用户档案： tianyan://user");
    }

    // 预置 planning 技能（VFS 方法论文档，与 GEPA 学习技能同构）：
    // 用户说"计划一下/做个方案/先别动手"时，模型调 call_skill("planning")，
    // 读到行为指南后按"只读研究 → 结构化计划 → 询问是否执行"行动。
    let planning_uri = TianyanUri::new(ContextNamespace::Skill, vec!["planning".to_string()]);
    if !vfs
        .has_content(&planning_uri, ContentLevel::Detail)
        .await
        .unwrap_or(false)
    {
        vfs.create_directory(&planning_uri).await?;
        vfs.write(&planning_uri, ContentLevel::Detail, PLANNING_SKILL_CONTENT)
            .await?;
        vfs.write(
            &planning_uri,
            ContentLevel::Abstract,
            "计划阶段软约束：只读研究 + 结构化计划 + 结束询问",
        )
        .await?;
        tracing::info!("已预置 planning 技能： tianyan://skill/planning");
    }

    tracing::info!("VFS 目录结构初始化完成");
    Ok(())
}

/// planning 技能内容（预置 VFS 方法论文档；原 PlanningHandler 静态指南）。
const PLANNING_SKILL_CONTENT: &str = "\
# 计划阶段

你已进入计划阶段（用户要求先规划再执行）。遵守以下约束：

1. **只读研究**：只使用只读工具收集信息（read_file / grep / search_vfs /\n   vfs_read / vfs_list / glob / list_dir / symbol_outline / lsp / web_search / web_fetch /\n   discover_tests）。不得调用 write_file / apply_edit / apply_patch / execute_command /\n   run_tests / verify_build / knowledge_ingest / delegate_to_agent（后台任务）。
2. **输出结构化计划**：按以下格式输出——\n   - 目标：一句话明确要完成什么\n   - 步骤：编号列表，每步注明涉及的文件/命令/风险\n   - 风险与验证：潜在副作用 + 每步完成后的验证方式（测试/检查命令，仅描述不执行）\n3. **结束询问**：计划输出完毕后，询问用户是否开始执行——用户确认前不执行任何写操作。\n4. 用户确认执行后，恢复正常（执行）行为。";

/// 允许的 CORS 来源（编译期常量，避免运行时 parse panic）。
const ALLOWED_ORIGINS: [HeaderValue; 6] = [
    HeaderValue::from_static("http://localhost:3000"),
    HeaderValue::from_static("http://localhost:1420"),
    HeaderValue::from_static("http://127.0.0.1:3000"),
    HeaderValue::from_static("http://127.0.0.1:1420"),
    HeaderValue::from_static("tauri://localhost"),
    HeaderValue::from_static("http://tauri.localhost"),
];

/// Create the application router with configuration
///
/// 支持无配置启动，用于配置向导模式
///
/// 返回 (Router, AppState) 元组，以便在关闭时访问 AppState。
/// `pub` 供集成测试（server/tests/）直接驱动真实路由与处理器。
pub async fn create_app(
    config: tianyan::config::TianyanConfig,
) -> tianyan::common::error::Result<(Router, Arc<AppState>)> {
    // 创建全系统共享的 SqliteDb（ADR-005：单文件、单连接）
    // - backend = "sqlite" 时：VFS 内容存储（vfs_entries 表）
    // - 始终：UsageStats 统计（skill_calls / doc_access 等表）
    let db_path = config
        .storage
        .sqlite_path
        .clone()
        .unwrap_or_else(|| config.storage.data_dir.join("tianyan.db"));
    // 统一写入门面：Database 门面（单连接 + schema 集中初始化）——全系统
    // 结构化存储的唯一入口（ADR-005），业务组件不再各自持 SqliteDb。
    let database = tianyan::db::Database::open(db_path).map_err(|e| {
        tianyan::TianyanError::Custom(format!("虚拟文件系统错误：打开 SQLite 数据库失败：{}", e))
    })?;
    if let Err(e) = database.init_schemas().await {
        return Err(tianyan::TianyanError::Custom(format!(
            "虚拟文件系统错误：初始化 SQLite Schema 失败：{}",
            e
        )));
    }

    // 在应用层初始化 VFS（单一实例，共享 SqliteDb）
    let vfs = initialize_vfs_for_app(&config, database.clone()).await?;

    // 命令执行底层（shell provider，ADR-037）：解析（auto 探测 / 用户配置）→
    // 安装全局 spec——失败即启动报错（含修复指引，不静默回退）。
    let shell_spec = tianyan::executor::shell::resolve(&config.executor)?;
    info!(
        kind = %shell_spec.kind.as_str(),
        executable = %shell_spec.executable.display(),
        "命令执行底层已就绪"
    );
    tianyan::executor::shell::install(shell_spec);

    // Create shared application state（传入 VFS + 共享 SqliteDb）
    let state = AppState::new(config, vfs, database).await?;

    info!("Application state initialized successfully");
    let state = Arc::new(state);

    // Configure CORS - 仅允许本地来源访问
    let cors = CorsLayer::new()
        .allow_origin(ALLOWED_ORIGINS)
        .allow_methods(Any)
        .allow_headers(Any);

    // Create API router
    let api_router = create_api_router(state.clone());

    // Serve static files (gui-vite/dist preferred, legacy gui/dist fallback)
    let static_dir = get_static_dir();
    info!("Serving static files from: {}", static_dir.display());

    let static_service = ServeDir::new(static_dir).append_index_html_on_directories(true);

    Ok((
        Router::new()
            .route("/health", get(health_check))
            .merge(api_router)
            .fallback_service(static_service)
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .layer(DefaultBodyLimit::max(50 * 1024 * 1024)), // 50MB 请求体限制
        state,
    ))
}

/// Start the HTTP server
///
/// # Arguments
///
/// * `config` - Server configuration (port, host, etc.)
/// * `tianyan_config` - Tianyan core configuration
///
/// # Errors
///
/// Returns an error if the server fails to start or bind to the address
///
/// # Example
///
/// ```no_run
/// use tianyan_server::{start_server, ServerConfig};
/// use tianyan::config::get_config;
///
/// #[tokio::main]
/// async fn main() -> tianyan::common::error::Result<()> {
///     let server_config = ServerConfig::with_port(3000);
///     let tianyan_config = get_config().clone();
///     start_server(server_config, tianyan_config).await
/// }
/// ```
pub async fn start_server(
    config: ServerConfig,
    tianyan_config: tianyan::config::TianyanConfig,
) -> tianyan::common::error::Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| tianyan::TianyanError::config(format!("无效地址: {}", e)))?;

    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
        error!("Failed to bind to address {}: {}", addr, e);
        tianyan::TianyanError::Custom(format!("网络错误：Failed to bind to address: {}", e))
    })?;

    start_server_with_listener(listener, tianyan_config).await
}

/// 使用调用方已绑定的 listener 启动服务器。
///
/// 与 [`start_server`] 的区别：监听 socket 由调用方提前绑定并传入，
/// 消除"探测端口 → 释放 → 重新绑定"之间的竞态窗口
/// （桌面端动态端口场景使用，见 `tauri::server::find_available_port`）。
pub async fn start_server_with_listener(
    listener: tokio::net::TcpListener,
    tianyan_config: tianyan::config::TianyanConfig,
) -> tianyan::common::error::Result<()> {
    start_server_inner(listener, tianyan_config, None).await
}

/// 使用调用方已绑定的 listener + 外部关闭信号启动服务器。
///
/// 桌面端使用：Tauri 退出时通过 watch channel 发送关闭信号，
/// 触发与 Ctrl+C / SIGTERM 相同的优雅关停链
/// （停止接收连接 → 停止调度器 → 等待 pending 任务，30s 超时）。
pub async fn start_server_with_shutdown_signal(
    listener: tokio::net::TcpListener,
    tianyan_config: tianyan::config::TianyanConfig,
    shutdown_signal: watch::Receiver<bool>,
) -> tianyan::common::error::Result<()> {
    start_server_inner(listener, tianyan_config, Some(shutdown_signal)).await
}

/// 服务器主体：装配应用 → 监听 → 优雅关停。
///
/// `external_shutdown` 为 `Some` 时，外部关闭信号与 Ctrl+C/SIGTERM 并列监听；
/// 为 `None` 时保持仅信号驱动（独立 server 二进制的既有语义）。
async fn start_server_inner(
    listener: tokio::net::TcpListener,
    tianyan_config: tianyan::config::TianyanConfig,
    external_shutdown: Option<watch::Receiver<bool>>,
) -> tianyan::common::error::Result<()> {
    let has_providers = tianyan_config.models.providers.iter().any(|p| p.enabled);
    let (app, state) = create_app(tianyan_config).await?;
    let task_scheduler: Option<Arc<TaskScheduler>> = if has_providers {
        // last_run 持久化（ADR-024）：宕机错过的事件任务重启后自动补跑
        let app_config = Arc::new(state.config().read().await.clone());
        let scheduler = Arc::new(
            TaskScheduler::new()
                .with_state_path(app_config.storage.data_dir.join("scheduler_state.json")),
        );

        // 创建任务上下文
        let vfs = state.vfs();
        let summary_engine = state.create_summary_engine().await?;
        let memory_extractor = state.create_memory_extractor().await?;
        let skill_reviewer = state.create_skill_reviewer().await?;

        let task_ctx = Arc::new(TaskContext::new(
            vfs.clone(),
            summary_engine,
            memory_extractor,
            skill_reviewer,
            app_config.clone(),
        ));

        // 向量库自愈回填（后台）：表为空（维度重建/首次建库）时把存量摘要重嵌入，
        // 恢复语义检索；表非空时 no-op。不阻塞启动。
        let backfill_vfs = vfs.clone();
        tokio::spawn(async move {
            match backfill_vfs.backfill_summary_vectors_if_empty().await {
                Ok(0) => {}
                Ok(n) => tracing::info!("向量库回填完成：重嵌入 {n} 条存量摘要"),
                Err(e) => tracing::warn!(error = %e, "向量库回填失败（内容更新时会渐进补齐）"),
            }
        });

        // 注册摘要任务（ADR-017：降频每 6 小时；知识导入同步摘要，本任务仅兜底；
        // 范围排除 Session 命名空间——会话回忆改 FTS5）
        scheduler
            .register_task(TaskDefinition::new(
                "summary_generation",
                "摘要生成",
                6 * 3600,
                Arc::new(SummaryTask::new()),
            ))
            .await?;

        // 注册自演化任务（ADR-017：每日一次；移除 memory_extraction / rule_extraction，
        // 记忆/技能/规则/组织形态的演化统一由演化智能体综述驱动）
        {
            let cfg = state.config().read().await.clone();
            if cfg.evolution.enabled {
                let executor = Arc::new(AgentEvolutionExecutor::new(
                    state.agent_lock(),
                    state.session_manager(),
                    state.working_sets(),
                    state::resolve_chat_model(&cfg),
                    cfg.evolution.review_role.clone(),
                ));
                // 旧 cron 配置自动换算为间隔（ADR-024 迁移）；换算不了回退每日一次
                let evolution_interval = match tianyan::scheduler::interval_from_cron(
                    &cfg.evolution.cron,
                ) {
                    Some(secs) => secs,
                    None => {
                        tracing::warn!(
                            old = %cfg.evolution.cron,
                            fallback_secs = 24 * 3600,
                            "evolution.cron 无法换算为间隔（ADR-024 已弃用 cron），回退每日一次；可从配置中删除该字段"
                        );
                        24 * 3600
                    }
                };
                scheduler
                    .register_task(TaskDefinition::new(
                        "evolution",
                        "自演化综述",
                        evolution_interval,
                        Arc::new(EvolutionTask::new(
                            executor,
                            cfg.evolution.max_items_per_run,
                        )),
                    ))
                    .await?;
            }
        }

        // 注册垃圾回收任务（每 6 小时；auto_cleanup 来自存储配置）
        scheduler
            .register_task(TaskDefinition::new(
                "garbage_collection",
                "垃圾回收",
                6 * 3600,
                Arc::new(GcTask::new(&app_config.storage)),
            ))
            .await?;

        // 注册使用统计刷盘任务（每 1 分钟；内存计数器 → SQLite，
        // 保证 /api/v1/stats 数据时效与内存有界，退出前由 shutdown 兜底）
        scheduler
            .register_task(TaskDefinition::new(
                "usage_stats_flush",
                "使用统计刷盘",
                60,
                Arc::new(UsageStatsFlushTask::new(state.usage_stats())),
            ))
            .await?;

        // 注册快照垃圾回收任务（每 12 小时；未配置工作区时 snapshot_manager 为 None，跳过）
        if let Some(sm) = state.snapshot_manager() {
            scheduler
                .register_task(TaskDefinition::new(
                    "snapshot_gc",
                    "快照垃圾回收",
                    12 * 3600,
                    Arc::new(SnapshotGcTask::new(sm)),
                ))
                .await?;
        }

        // 注册主动提醒任务（reminder.enabled 时；间隔取整分钟，至少 1 分钟）
        {
            let cfg = state.config().read().await.clone();
            if cfg.reminder.enabled {
                let interval_min = (cfg.reminder.interval_secs.max(60) / 60) as u64;
                // 复用共享 ModelServices（不重复创建 HTTP client 池）
                let model_services = state.shared_model_services().await?;
                let model_name = state::resolve_chat_model(&cfg);
                scheduler
                    .register_task(TaskDefinition::new(
                        "reminder",
                        "主动提醒",
                        interval_min * 60,
                        Arc::new(tianyan::scheduler::tasks::ReminderTask::new(
                            model_services.chat,
                            model_name,
                            notification::global_notification_sink(),
                            state.session_manager(),
                            cfg.reminder.max_per_run,
                            cfg.reminder.inject_to_session,
                        )),
                    ))
                    .await?;
            }
        }

        // T1 事件驱动：文件监听 + 事件处理器（events.enabled 时；
        // webhook 令牌/总线经 AppState 由 handler 直读，无需装配）
        {
            let events_config = state.config().read().await.events.clone();
            if events_config.enabled {
                // 文件监听（notify 独立线程运行；失败仅告警，webhook 仍可用）
                if !events_config.watch_dirs.is_empty() {
                    let watcher = tianyan::events::FileWatcher::new((*state.event_bus()).clone());
                    let dirs: Vec<std::path::PathBuf> = events_config
                        .watch_dirs
                        .iter()
                        .map(std::path::PathBuf::from)
                        .collect();
                    if let Err(e) = watcher
                        .start(dirs, events_config.watch_events.clone())
                        .await
                    {
                        warn!("文件监听启动失败（事件驱动部分不可用）：{}", e);
                    }
                }
                // 事件处理器（task 动作在 scheduler 可用时装配触发器；
                // 事件装配位于 has_providers 块内，scheduler/task_ctx 必存在）
                let deps = event_processor::ProcessorDeps::new(
                    state.agent().await,
                    state.session_manager(),
                )
                .with_task_trigger(Arc::new(
                    event_processor::SchedulerTaskTrigger::new(scheduler.clone(), task_ctx.clone()),
                ));
                event_processor::EventProcessor::start(
                    (*state.event_bus()).clone(),
                    tianyan::events::parse_rules(&events_config.rules),
                    deps,
                );
            }
        }

        // 启动任务调度器
        TaskScheduler::start_with_scheduler(scheduler.clone(), task_ctx.clone()).await?;
        info!("任务调度器已启动");

        // 定时智能体任务管理器（schedule_task 工具 + REST API + 后台循环）
        {
            let data_dir = state.config().read().await.storage.data_dir.clone();
            // 分层（避免循环引用）：manager 经 TaskRegistrar 接口与调度器交互，
            // 不直接持有 scheduler/task_ctx；注册器先创建、后 bind（装配顺序解耦）。
            let registrar = Arc::new(SchedulerRegistrar::new());
            let manager = Arc::new(ScheduledAgentTaskManager::new(
                state.agent_lock(),
                state.session_manager(),
                &data_dir,
                registrar.clone(),
                state.working_sets(),
            ));
            registrar
                .bind(Some(scheduler.clone()), Some(task_ctx.clone()))
                .await;
            state
                .attach_scheduled_agent_tasks(Some(manager.clone()))
                .await;
            // 注入 schedule_task 动态工具（agent 构建后追加，LLM 可见可调用）。
            // 解耦：工具只持创建请求通道（不依赖 manager 类型）；装配层在此创建
            // 消费者（持 manager 的 Weak）监听通道处理请求。通道生命周期随工具
            // 自动结束（agent drop → 工具 drop → sender drop → 通道关闭 → 消费者退出）。
            let (create_task_tx, mut create_task_rx) =
                tokio::sync::mpsc::channel::<CreateTaskRequest>(32);
            let manager_weak = Arc::downgrade(&manager);
            tokio::spawn(async move {
                while let Some(request) = create_task_rx.recv().await {
                    let result = match manager_weak.upgrade() {
                        Some(m) => Ok(m.create(&request.req).await),
                        None => Err("定时任务管理器不可用".to_string()),
                    };
                    let _ = request.resp_tx.send(result);
                }
            });
            state
                .agent_lock()
                .read()
                .await
                .register_dynamic_tools(vec![Arc::new(ScheduleTaskTool::new(create_task_tx))])
                .await;
            // 注入 todo/goal 动态工具（agent 可自主创建/跟踪/修复待办与目标——
            // 计划面板数据同源，非纯 UI 功能）
            state
                .agent_lock()
                .read()
                .await
                .register_dynamic_tools(vec![
                    Arc::new(TodoTool::new(state.todo_store())),
                    Arc::new(GoalTool::new(state.goal_store())),
                ])
                .await;
            // 恢复持久化任务（注册经 registrar 接口进调度器）
            manager.load_and_register().await;
        }

        Some(scheduler)
    } else {
        warn!("没有启用的模型服务，跳过所有定时任务注册");
        None
    };

    // 暴露调度器状态查询（无 Provider 时注入 None，端点返回空状态）
    state.attach_scheduler(task_scheduler.clone()).await;

    let addr = listener.local_addr().map_err(|e| {
        error!("Failed to read listener address: {}", e);
        tianyan::TianyanError::Custom(format!("网络错误：Failed to read listener address: {}", e))
    })?;

    info!("Starting Tianyan server on http://{}", addr);

    info!("Server is ready to accept connections");

    // 创建 shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // 装配到 AppState：数据目录搬迁等 API 可触发优雅关停（关停后由
    // 监督循环/主循环执行搬迁并重启）
    state.attach_shutdown_tx(shutdown_tx.clone());

    // 克隆 shutdown receiver 用于服务器优雅关闭
    let mut shutdown_rx_server = shutdown_rx.clone();
    // 内部关停监听（request_shutdown 触发；与外部信号并列，独立 server
    // 无外部信号源时同样生效）
    let mut shutdown_rx_internal = shutdown_rx.clone();

    // 启动服务器，设置优雅关闭
    let mut server_handle = tokio::spawn(async move {
        let server = axum::serve(listener, app);
        let shutdown_signal = async move {
            // 等待 shutdown 信号
            if shutdown_rx_server.changed().await.is_err() {
                tracing::warn!("关闭信号通道已断开，直接关闭服务器");
            }
            info!("收到关闭信号，服务器停止接受新连接");
        };

        server.with_graceful_shutdown(shutdown_signal).await
    });

    // 监听关闭信号（外部信号 / Ctrl+C / SIGTERM(unix) / 服务器退出）
    // 单 select 块 + cfg 分支属性：消除 unix/非 unix 双份近重复实现
    let shutdown_result = tokio::select! {
        external_result = wait_external_shutdown(external_shutdown) => {
            match external_result {
                "external" => {
                    info!("收到外部关闭信号（桌面端退出）");
                    state.shutdown_flag().store(true, std::sync::atomic::Ordering::Relaxed);
                    "external"
                }
                _ => {
                    error!("外部关闭信号通道断开");
                    "error"
                }
            }
        },
        internal_result = wait_internal_shutdown(&mut shutdown_rx_internal) => {
            match internal_result {
                "internal" => {
                    info!("收到内部关闭信号（数据目录搬迁等）");
                    state.shutdown_flag().store(true, std::sync::atomic::Ordering::Relaxed);
                    "internal"
                }
                _ => {
                    error!("内部关闭信号通道断开");
                    "error"
                }
            }
        },
        ctrl_c_result = tokio::signal::ctrl_c() => {
            match ctrl_c_result {
                Ok(()) => {
                    info!("捕获到 Ctrl+C 信号");
                    state.shutdown_flag().store(true, std::sync::atomic::Ordering::Relaxed);
                    "ctrl_c"
                }
                Err(e) => {
                    error!("监听 Ctrl+C 失败：{}", e);
                    "error"
                }
            }
        },
        sigterm_result = wait_sigterm() => {
            match sigterm_result {
                Some(reason) => {
                    info!("捕获到 {reason} 信号");
                    state.shutdown_flag().store(true, std::sync::atomic::Ordering::Relaxed);
                    "sigterm"
                }
                None => "sigterm_error",
            }
        },
        server_result = &mut server_handle => {
            match server_result {
                Ok(Ok(())) => {
                    info!("服务器正常退出");
                    "server_stopped"
                }
                Ok(Err(e)) => {
                    error!("服务器错误：{}", e);
                    "server_error"
                }
                Err(e) => {
                    error!("服务器任务失败：{}", e);
                    "server_task_error"
                }
            }
        }
    };

    // 如果是因为信号触发的关闭，发送 shutdown 信号
    if shutdown_result != "server_stopped"
        && shutdown_result != "server_error"
        && shutdown_result != "server_task_error"
    {
        info!("开始优雅关闭流程...");

        // 发送 shutdown 信号
        if let Err(e) = shutdown_tx.send(true) {
            error!("发送 shutdown 信号失败：{}", e);
        }

        // 等待服务器停止接受新连接
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 停止任务调度器
        if let Some(ref scheduler) = task_scheduler {
            info!("停止任务调度器...");
            if let Err(e) = scheduler.shutdown().await {
                error!("停止任务调度器失败：{}", e);
            }
        }

        // 调用 AppState::shutdown() 等待所有 pending 任务完成
        info!("等待待处理任务完成...");

        use tokio::time::{timeout, Duration};
        let max_wait = Duration::from_secs(30);

        match timeout(max_wait, state.shutdown()).await {
            Ok(Ok(())) => {
                info!("应用已优雅关闭");
            }
            Ok(Err(e)) => {
                error!("关闭过程中发生错误：{}", e);
            }
            Err(_) => {
                warn!("关闭超时 ({}s)，强制退出", max_wait.as_secs());
            }
        }
    }

    // 等待服务器任务完全结束：axum 优雅关停完成 → router/AppState drop →
    // SQLite/LanceDB 文件锁释放。数据目录搬迁依赖此顺序（Windows 上移动
    // 打开的文件会失败）；select 分支获胜时 JoinHandle 未被消费，此处可 await。
    // T1：加**超时兜底**——运行中的流/工具执行若未能及时收尾，不再无限等待
    // （托盘退出卡死的另一半：此处此前无上限，主进程永不执行 app.exit）。
    match tokio::time::timeout(tokio::time::Duration::from_secs(5), server_handle).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => error!("服务器任务异常结束：{}", e),
        Ok(Err(e)) => error!("服务器任务异常结束：{}", e),
        Err(_) => warn!("等待服务器关停超时（5s），继续退出流程"),
    }

    Ok(())
}

/// 等待内部关闭信号（AppState::request_shutdown 触发：数据目录搬迁等）。
///
/// 与外部信号并列监听；内部通道断开视为错误（正常路径不会发生）。
async fn wait_internal_shutdown(rx: &mut watch::Receiver<bool>) -> &'static str {
    if rx.changed().await.is_ok() {
        "internal"
    } else {
        "internal_error"
    }
}

/// 等待外部关闭信号。
///
/// 未提供信号源时永久挂起（保持仅 Ctrl+C/SIGTERM 驱动的既有语义）。
async fn wait_external_shutdown(external: Option<watch::Receiver<bool>>) -> &'static str {
    match external {
        Some(mut rx) => {
            if rx.changed().await.is_ok() {
                "external"
            } else {
                "external_error"
            }
        }
        None => std::future::pending().await,
    }
}

/// 等待 SIGTERM 信号（unix）；非 unix 平台永挂起——
/// 该分支在 select 中不可达，保持"仅外部/Ctrl+C 驱动"的既有语义。
async fn wait_sigterm() -> Option<&'static str> {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                if signal.recv().await.is_some() {
                    Some("SIGTERM")
                } else {
                    None
                }
            }
            Err(e) => {
                error!("监听 SIGTERM 失败：{}", e);
                None
            }
        }
    }
    #[cfg(not(unix))]
    {
        std::future::pending().await
    }
}

/// Start the server with default configuration
///
/// This is a convenience function that starts the server with default settings
/// (host: 127.0.0.1, port: 3000)
pub async fn start_server_default() -> tianyan::common::error::Result<()> {
    let tianyan_config = tianyan::config::get_config().clone();
    start_server(ServerConfig::default(), tianyan_config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 3000);
        assert_eq!(config.host, "127.0.0.1");
    }

    #[test]
    fn test_server_config_with_port() {
        let config = ServerConfig::with_port(8080);
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "127.0.0.1");
    }

    #[test]
    fn test_server_config_new() {
        let config = ServerConfig::new("0.0.0.0", 9000);
        assert_eq!(config.port, 9000);
        assert_eq!(config.host, "0.0.0.0");
    }

    #[test]
    fn test_server_config_from_port_str() {
        // 合法值：覆盖端口（容忍前后空白）
        assert_eq!(ServerConfig::from_port_str(Some("4321")).port, 4321);
        assert_eq!(ServerConfig::from_port_str(Some(" 4321 ")).port, 4321);
        // 非法 / 空串 / 越界 / 缺失：回退默认端口（不 panic，不静默改 host）
        assert_eq!(ServerConfig::from_port_str(Some("abc")).port, 3000);
        assert_eq!(ServerConfig::from_port_str(Some("")).port, 3000);
        assert_eq!(ServerConfig::from_port_str(Some("70000")).port, 3000);
        assert_eq!(ServerConfig::from_port_str(None).port, 3000);
        // 端口可配不改变暴露面：host 恒为回环
        assert_eq!(ServerConfig::from_port_str(Some("4321")).host, "127.0.0.1");
    }

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await;
        assert_eq!(response.0.status, "ok");
    }
}
