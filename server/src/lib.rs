//! Tianyan HTTP 服务器。

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use axum::http::{HeaderValue, StatusCode};
use axum::{
    extract::DefaultBodyLimit,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::watch;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

use tianyan::observability::usage_log::UsageLog;
use tianyan::scheduler::{TaskContext, TaskDefinition, TaskScheduler};
use tianyan::vfs::VirtualFileSystem;

use crate::api::events::processor as event_processor;
use crate::embedding_usage::attach_embedding_usage_sink;
use crate::evolution_executor::AgentEvolutionExecutor;
use crate::scheduled_tasks::manager::{ScheduledAgentTaskManager, SchedulerRegistrar};

// Import API module
pub mod agent_builder;
pub mod api;
/// 装配扩展点（ADR-049：插件工具清单 + 向量存储工厂）。
pub mod assembly;
/// 嵌入用量入账（usage_logs）。
pub mod embedding_usage;
pub mod event_push;
pub mod evolution_executor;
pub mod mcp_bridge;
pub mod migration;
mod model_services;
pub mod notification;
pub mod scheduled_tasks;
pub mod session_leases;
pub mod state;

use api::create_api_router;
use state::AppState;

pub use assembly::{
    ApprovalGateFactory, AssemblyServices, ContextFactory, ContextPort, ContextServices,
    ManifestAssemblies, PolicyAssemblies, ProviderAssemblies, ProviderEmbedFactory,
    ProviderEmbedServices, ProviderLlmFactory, ProviderLlmServices, SkillReviewFactory,
    StorageAssemblies, StorageBackendFactory, TaskFactory, TaskRegistration, TaskReviewerFactory,
    TaskServices, ToolPipelineFactory, ToolPipelineWiring, VectorStorageFactory,
};
pub use model_services::{create_model_services, ModelServices};

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
    // 显式覆盖（测试 / 自定义部署）：优先于所有猜测路径。
    // 测试进程的 cwd 是 crate 目录（`server/`），下面第 3 条能命中；但 CI 或嵌
    // 入式部署未必，故留一条确定性入口。
    if let Ok(dir) = std::env::var("TIANYAN_STATIC_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
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
///
/// `assembly` 提供向量存储实现（[`StorageAssemblies::vector`]）：
/// 本库**不认识任何具体向量实现**（LanceDB 等），实现由装配层注入
/// （ADR-049 期 2 序 3：lancedb/arrow/datafusion 依赖链不进 core）。
///
/// `assembly` 同时提供结构化存储实现（[`StorageAssemblies::backend`]）：
/// `config.storage.backend`（`Local` / `Sqlite`）**选择哪个 adapter 也由装配层
/// 决定**（ADR-049 D5 / 期 2 序 4）——本库只把配置与共享连接递过去。
async fn initialize_vfs_for_app(
    config: &tianyan::config::TianyanConfig,
    database: Arc<tianyan::db::Database>,
    assembly: &AssemblyServices,
) -> tianyan::common::error::Result<Arc<tianyan::vfs::VirtualFileSystemImpl>> {
    use tianyan::vfs::{StorageBackend, VirtualFileSystemBuilder};

    // 1. 创建存储后端（实现与 adapter 选择都在装配层；clone：database 还需用于
    //    嵌入用量入账——UsageLog 共享同一连接）
    let storage: Arc<dyn StorageBackend> = assembly
        .storage()
        .backend(&config.storage, database.clone())?;
    info!(
        backend = ?config.storage.backend,
        "VFS 存储后端已装配（实现由装配层注入）"
    );

    // 向量存储由装配层注入（async 初始化：LanceDB 建连/建表）
    let vector_storage = assembly.storage().vector(&config.storage).await?;

    // 2. 创建模型服务（用于嵌入服务），无配置时跳过
    let mut vfs_builder = VirtualFileSystemBuilder::new()
        .with_storage(storage)
        .with_vector_storage(vector_storage)
        .with_config(config.storage.clone());
    {
        let model_services = create_model_services(config, assembly);
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

1. **只读研究**：只使用只读工具收集信息（read_file / grep / search_vfs /\n   vfs_read / vfs_list / glob / list_dir / symbol_outline / repo_map / web_search / web_fetch）。\n   不得调用 write_file / apply_edit / apply_patch / execute_command /\n   knowledge_ingest / delegate_to_agent（后台任务）。
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
/// **无插件工具、无向量存储**（[`AssemblyServices`] 的降级路径：空向量存储——
/// VFS 内容读写/摘要/工具全部可用，但没有语义检索）。生产装配（bin / 桌面端）
/// 用 [`create_app_with_assembly`] 注入插件工具与向量存储实现。
///
/// 返回 (Router, AppState) 元组，以便在关闭时访问 AppState。
/// `pub` 供集成测试（server/tests/）直接驱动真实路由与处理器。
pub async fn create_app(
    config: tianyan::config::TianyanConfig,
) -> tianyan::common::error::Result<(Router, Arc<AppState>)> {
    create_app_inner(config, AssemblyServices::default()).await
}

/// 插件装配所需的**非 `ToolContext` 能力**（store 等）。
///
/// 多数插件工具只需 `ToolContext`（路径解析 + 安全策略），但**会话绑定**的
/// 工具（`todo` / `goal`）还要数据存储。`TodoStore` / `GoalStore` 留在 core
/// （它们是 store 不是工具，REST API / 会话面板也直接用），且工具与 API 必须
/// 读写**同一份**实例——工具不能自建 store。
///
/// 因此清单工厂的签名是 `Fn(&ToolContext, PluginServices)`（可重复调用：agent
/// 重建后要重新注入）；本库从
/// [`AppState`] 取出同一份 store 交给清单（[`AppState::todo_store`] /
/// [`AppState::goal_store`]），插件 crate 自己不认识 `AppState`。server 库
/// 仍**不依赖任何插件 crate**（闸门 A：改一行工具不重编 server）。
#[derive(Clone)]
pub struct PluginServices {
    todo_store: Arc<tianyan::todos::TodoStore>,
    goal_store: Arc<tianyan::goals::GoalStore>,
    web_config: tianyan::config::WebConfig,
}

impl PluginServices {
    /// 以 AppState 持有的同一份 store 与 Web 配置构造（[`create_app_inner`] / `register_plugin_tools` 单点调用）。
    pub fn new(
        todo_store: Arc<tianyan::todos::TodoStore>,
        goal_store: Arc<tianyan::goals::GoalStore>,
        web_config: tianyan::config::WebConfig,
    ) -> Self {
        Self {
            todo_store,
            goal_store,
            web_config,
        }
    }

    /// 待办存储（与 REST API 共享同一实例）。
    pub fn todo_store(&self) -> Arc<tianyan::todos::TodoStore> {
        self.todo_store.clone()
    }

    /// 目标存储（与 REST API 共享同一实例）。
    pub fn goal_store(&self) -> Arc<tianyan::goals::GoalStore> {
        self.goal_store.clone()
    }

    /// Web 工具配置（`web_search` / `web_fetch` 插件据此构造客户端；
    /// 构造失败时插件自行降级为空清单，不影响装配）。
    pub fn web_config(&self) -> tianyan::config::WebConfig {
        self.web_config.clone()
    }
}

/// 插件工具工厂（ADR-049 期 1：**装配清单住在 bin**）。
///
/// server 库**不认识任何具体插件类型**——只认 core 的 `ToolContext` 契约、
/// [`PluginServices`] 与 `DynamicToolExecutor` trait。类型擦除是刻意的：若
/// server 库 `use` 了插件 crate，改一行工具就会重编 server 库（13.4K 源码），
/// 闸门 A 直接失效。
/// 插件清单工厂（**可重复调用**）。
///
/// `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：Agent 会因配置热重载 / 模型切换 / MCP 变更
/// 反复重建，每次重建都要重新注入插件工具——一次性工厂会让插件工具在 reload 后
/// 永久丢失（`todo` / `read_file` / `grep` 等全部消失，直到重启进程）。
pub type ToolPluginFactory = Arc<
    dyn Fn(
            &tianyan::agent::ToolContext,
            PluginServices,
        ) -> Vec<Arc<dyn tianyan::agent::DynamicToolExecutor>>
        + Send
        + Sync,
>;

/// 带完整装配的应用构造（**bin / 桌面端用**）。
///
/// 装配顺序：Agent 先就绪（内部注册表持有 `ToolContext`）→ 取 ctx + store
/// （[`PluginServices`]）→ 插件工具工厂注入（追加到动态工具末尾，内置顺序不动）
/// → 向量存储 / 结构化存储工厂 → 内置任务清单（在 `start_server_inner` 注册）。
///
/// 未装配的插槽走降级路径（无插件工具 / 空向量存储 / 内存存储后端 / 无内置任务），
/// 每处都有 `warn!` / `error!` 指路（见 [`AssemblyServices`]）。**生产装配不要漏**。
/// 注意：[`create_app`]（无装配）与之并存，是测试/嵌入式的对象图构造入口。
pub async fn create_app_with_assembly(
    config: tianyan::config::TianyanConfig,
    assembly: AssemblyServices,
) -> tianyan::common::error::Result<(Router, Arc<AppState>)> {
    create_app_inner(config, assembly).await
}

/// 应用装配主体（`assembly` 决定插件工具与向量存储实现）。
async fn create_app_inner(
    config: tianyan::config::TianyanConfig,
    assembly: AssemblyServices,
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
    let vfs = initialize_vfs_for_app(&config, database.clone(), &assembly).await?;

    // 命令执行底层（shell provider，ADR-037）：解析（auto 探测 / 用户配置）→
    // 安装全局 spec——失败即启动报错（含修复指引，不静默回退）。
    let shell_spec = tianyan::executor::shell::resolve(&config.executor)?;
    info!(
        kind = %shell_spec.kind.as_str(),
        executable = %shell_spec.executable.display(),
        "命令执行底层已就绪"
    );
    tianyan::executor::shell::install(shell_spec);

    // Create shared application state（传入 VFS + 共享 SqliteDb + 三个插槽组）
    //
    // 审批门控（ADR-049 期 3-T1）：工厂交给组合根 `AppState::new`——门控要绑定
    // **与 Agent 同一个**会话工作集（ADR-039 单写入口）与通知通道，而工作集是
    // `AppState::new` 内部创建的；在本函数里另建一份会让审批提示写进另一份状态。
    // 工厂同时存进 `AppState` 供 `reload_agent`（配置热重载）重建时复用。
    //
    // 上下文策略（ADR-049 期 3-T2）：同一个 `Option<Factory>` 也交给组合根——能力集合
    // 里要用 `AppState` 内部创建的 `usage_stats` 与共享的模型服务，在本函数里组装会
    // 多一份实例（统计分流）。
    //
    // 工具管线（ADR-049 期 3-T5）：观测 / 落盘的实现已出 core，工厂同样交给组合根
    // → `AgentBuilder`（接线值 `usage_stats` / `trace_collector` / `execution_log`
    // 都是 `AppState`/builder 内部的同一份实例）。
    //
    // 期 4b：7 个平铺的 `Option<Factory>` 收敛成 3 个**组**（`manifests` /
    // `policies` / `providers`；`storage` 组在 `initialize_vfs_for_app` 就用完了）。
    // 传 clone 而不是 move：`assembly` 下面还要取 `plugin_factory` 注入插件工具
    // （组是若干 `Arc` 的集合，克隆只加引用计数）。
    let state = AppState::new(
        config,
        vfs,
        database,
        assembly.manifests.clone(),
        assembly.policies.clone(),
        assembly.providers.clone(),
    )
    .await?;

    // 插件工具注入（ADR-049 期 1）：agent 就绪 → 取 ctx + services → 工厂构造
    // → 注册。失败语义：ctx 不可得时静默跳过（插件是可选增强，不是启动硬依赖）。
    //
    // `PluginServices` 携带 AppState 的 store 实例（`todo` / `goal` 工具需要）：
    // 装配清单是唯一知道「哪个插件需要哪份 store」的地方，本库只负责递过去。
    if let Some(factory) = assembly.manifests().plugin_factory() {
        // 工厂存进 AppState：`reload_agent` 重建 agent 时用**同一份**清单重新注入
        // （否则插件工具在热重载后永久丢失，直到重启进程）。
        state.attach_plugin_factory(factory).await;
        let agent = state.agent().await;
        state.register_plugin_tools(&agent).await;
    }

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

    // SPA 回退（修复：`/chat` 等前端子路由直接访问/刷新返回 404 白屏）。
    //
    // 前端是 history 路由（`/chat`、`/chat/{session_id}`、`/settings`…），磁盘上没有
    // 对应文件，用户按 F5 时浏览器请求的就是 `/chat` —— 必须回 `index.html` 交给前端路由。
    //
    // 实现选择（**纠错记录**）：最初用「教科书写法」
    // `ServeDir::not_found_service(ServeFile::new(index))`，实测仍返回 404，一度误判为
    // axum 0.8 + tower-http 0.6 的版本缺陷。查 tower-http 源码后确认是**用法错误**：
    //
    //     pub fn not_found_service<F2>(self, f: F2) -> ServeDir<SetStatus<F2>> {
    //         self.fallback(SetStatus::new(f, StatusCode::NOT_FOUND))   // 强制 404
    //     }
    //
    // 它的语义是「404 + 自定义 body」，**不是** SPA fallback（那要求 200 + index.html），
    // 所以 404 是正确行为。SPA 要用 `.fallback(ServeFile::new(index))`（不带 SetStatus）。
    // 这里最终选用显式 handler，是因为它能区分两类未命中（见下），比 ServeDir::fallback
    // 更精确：
    // - `/assets/*` 交给 `ServeDir`（保留正确 MIME 与缓存头）；
    // - 无扩展名的路径（前端路由）→ `index.html`；
    // - 带扩展名的（`/missing.js`）→ 404，**不**伪装成 HTML（否则浏览器把 HTML 当 JS
    //   解析，报出与真实原因无关的语法错误）；
    // - `/api/v1/**` 的未知路径由 api_router 的 `api_not_found` 先兜住（404 JSON），
    //   不会落到这里变成 HTML。
    let spa_index = static_dir.join("index.html");
    let spa_fallback = move |uri: axum::http::Uri| {
        let index = spa_index.clone();
        async move {
            let path = uri.path();
            let last_segment = path.rsplit('/').next().unwrap_or("");
            // 带扩展名 → 静态资源缺失，如实 404（不吞成 HTML）
            if last_segment.contains('.') {
                return (StatusCode::NOT_FOUND, "静态资源不存在").into_response();
            }
            match tokio::fs::read(&index).await {
                Ok(bytes) => (
                    [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    bytes,
                )
                    .into_response(),
                Err(e) => {
                    // 前端产物缺失（未 npm run build / 部署不完整）：如实报错，不静默白屏
                    error!("SPA 外壳读取失败（{}）：{}", index.display(), e);
                    (
                        StatusCode::NOT_FOUND,
                        format!("前端产物缺失：{}", index.display()),
                    )
                        .into_response()
                }
            }
        }
    };
    let assets_service = ServeDir::new(static_dir.join("assets"));

    Ok((
        Router::new()
            .route("/health", get(health_check))
            .merge(api_router)
            .nest_service("/assets", assets_service)
            .fallback(spa_fallback)
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .layer(DefaultBodyLimit::max(50 * 1024 * 1024)), // 50MB 请求体限制
        state,
    ))
}

/// Start the HTTP server（**唯一的生产启动入口之一**：必须带装配）。
///
/// # 为什么没有「不带装配」的 `start_server`
///
/// ADR-049 期 2 收尾时删除了全部不带装配的启动变体（`start_server` /
/// `start_server_with_listener` / `start_server_with_shutdown_signal` /
/// `*_and_plugins` / `start_server_default`，当时全仓**零外部调用者**）。
/// 理由是结构性的：不带装配的启动会拿到 `EphemeralStorageBackend`（进程内存、
/// 重启即空）——服务器起得来、数据不落盘，症状是「检索永远为空」而非报错。
/// VFS 存储是**启动必需项**，不该在「忘了装配」时静默降级；把装配从可选参数
/// 收成唯一通道，误用就从「运行期隐患」变成「编译期不存在」。
///
/// 构造对象图（测试/嵌入式）仍可用 [`create_app`]——它不启动服务、允许内存后端。
///
/// # Arguments
///
/// * `config` - Server configuration (port, host, etc.)
/// * `tianyan_config` - Tianyan core configuration
/// * `assembly` - 装配（插件工具 / 向量存储 / 结构化存储 / 内置任务清单）
///
/// # Errors
///
/// Returns an error if the server fails to start or bind to the address
/// （`docs/architecture/decisions/049-microkernel-plugin-boundaries.md` 期 2 序 3/4/5a）
pub async fn start_server_with_assembly(
    config: ServerConfig,
    tianyan_config: tianyan::config::TianyanConfig,
    assembly: AssemblyServices,
) -> tianyan::common::error::Result<()> {
    let listener = bind_listener(&config).await?;
    start_server_with_listener_and_assembly(listener, tianyan_config, assembly).await
}

/// 绑定监听 socket（各启动入口共用；错误文案单点）。
async fn bind_listener(
    config: &ServerConfig,
) -> tianyan::common::error::Result<tokio::net::TcpListener> {
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| tianyan::TianyanError::config(format!("无效地址: {}", e)))?;

    tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
        error!("Failed to bind to address {}: {}", addr, e);
        tianyan::TianyanError::Custom(format!("网络错误：Failed to bind to address: {}", e))
    })
}

/// 使用调用方已绑定的 listener + 完整装配启动服务器。
///
/// 监听 socket 由调用方提前绑定并传入，消除「探测端口 → 释放 → 重新绑定」的
/// 竞态窗口（桌面端动态端口场景使用，见 `tauri::server::find_available_port`）。
///
/// 装配是**必填**（见 [`start_server_with_assembly`] 的「为什么没有不带装配的
/// `start_server`」）。
pub async fn start_server_with_listener_and_assembly(
    listener: tokio::net::TcpListener,
    tianyan_config: tianyan::config::TianyanConfig,
    assembly: AssemblyServices,
) -> tianyan::common::error::Result<()> {
    start_server_inner(listener, tianyan_config, None, assembly).await
}

/// 桌面端：listener + 外部关闭信号 + 完整装配（**桌面端唯一启动入口**）。
///
/// Tauri 退出时通过 watch channel 发送关闭信号，触发与 Ctrl+C / SIGTERM 相同的
/// 优雅关停链（停止接收连接 → 停止调度器 → 等待 pending 任务，30s 超时）。
pub async fn start_server_with_shutdown_signal_and_assembly(
    listener: tokio::net::TcpListener,
    tianyan_config: tianyan::config::TianyanConfig,
    shutdown_signal: watch::Receiver<bool>,
    assembly: AssemblyServices,
) -> tianyan::common::error::Result<()> {
    start_server_inner(listener, tianyan_config, Some(shutdown_signal), assembly).await
}

/// 服务器主体：装配应用 → 监听 → 优雅关停。
///
/// `external_shutdown` 为 `Some` 时，外部关闭信号与 Ctrl+C/SIGTERM 并列监听；
/// 为 `None` 时保持仅信号驱动（独立 server 二进制的既有语义）。
async fn start_server_inner(
    listener: tokio::net::TcpListener,
    tianyan_config: tianyan::config::TianyanConfig,
    external_shutdown: Option<watch::Receiver<bool>>,
    assembly: AssemblyServices,
) -> tianyan::common::error::Result<()> {
    // 启动服务的硬前置：结构化存储后端必须由装配层注入。
    //
    // 为什么在「删掉全部不带装配的启动入口」之后还要这道守卫：
    // `AssemblyServices::default()` 仍是合法值（`start_server_with_assembly(cfg, cfg,
    // default())` 编译得过），它会让 VFS 落到 `EphemeralStorageBackend`（进程内存）——
    // 服务器起得来、内容不落盘、重启即空。存储是启动必需项，这类误配必须在**启动前**
    // 响亮失败，而不是等运行期第一次写入时丢数据。
    //
    // `create_app`（测试/嵌入式构造对象图）**不经过**此守卫，仍允许内存后端：那条路径
    // 的实例生命周期由调用方自己负责，且 47 处单测依赖它。
    if !assembly.storage().has_storage_factory() {
        return Err(tianyan::TianyanError::invalid_input(
            "装配缺陷：启动服务器必须注入 StorageBackendFactory（结构化存储是启动必需项；\
             缺失会静默使用进程内存后端 → 内容不落盘、重启即丢）。生产装配请用 app/tauri \
             的 assembly()；测试/嵌入式构造对象图请改用 create_app",
        ));
    }
    // 启动服务的第二道硬前置：上下文策略必须由装配层注入。
    //
    // 缺失的后果与存储同级（甚至更显眼）：agent 会落到 `NullContextPort`——模型**没有**
    // system 前缀（soul / 项目说明 / learned rules / memories 全无），且上下文**永不压缩**
    // （会话链无限增长，直到上游 400）。服务起得来，但「天演」已经不是它自己。
    // 与审批降级的差异（T1 只 `warn!`）就在这里：审批缺失少一层策略复核，
    // 上下文缺失等于人设与记忆全丢。
    //
    // `create_app`（测试/嵌入式构造对象图）**不经过**此守卫，仍允许空直通兜底：那条路径
    // 的实例生命周期由调用方自己负责，且 47 处单测依赖它。
    if !assembly.policies().has_context_factory() {
        return Err(tianyan::TianyanError::invalid_input(
            "装配缺陷：启动服务器必须注入 ContextFactory（上下文策略是启动必需项；\
             缺失会让 agent 失去全部 system 前缀且上下文永不压缩）。生产装配请用 app/tauri \
             的 assembly()；测试/嵌入式构造对象图请改用 create_app",
        ));
    }
    let has_providers = tianyan_config.models.providers.iter().any(|p| p.enabled);
    // clone：`AssemblyServices` 是 4 个**组**，每组的工厂都是 `Arc`（克隆只加引用
    // 计数），而任务清单工厂在应用构造之后才用到（见下方「内置定时任务」块）。
    let task_assembly = assembly.clone();
    let (app, state) = create_app_inner(tianyan_config, assembly).await?;
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
        let session_recall = state.session_recall();

        let task_ctx = Arc::new(TaskContext::new(
            vfs.clone(),
            summary_engine,
            Some(session_recall),
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

        // 定时任务（ADR-049 期 2 序 5a + 5b）：**全部任务的实现与注册清单都在插件里**
        // （`plugins/task-builtin` 的 5 项 + `plugins/task-evolution` 的演化任务）。
        // 本库只做两件事——把能力集合（[`TaskServices`]）递过去、按返回顺序逐条
        // `register_task`；任务名 / 显示名 / 间隔 / 启用条件全在插件里，server 不认识
        // 任何具体任务（D2-3）。
        //
        // 顺序纪律：注册顺序 = 调度器扫描顺序 = 同一轮到期任务的执行顺序
        // （`RegisteredTask`：「注册顺序 = 扫描顺序」）。**跨插件的顺序由装配层的
        // `task_factory` 拼接决定**（`splice(1..1, …)`：演化任务紧跟
        // `summary_generation`，与迁移前一致）——本库只按清单顺序注册。
        {
            let task_services = TaskServices {
                // 注册期配置快照（提醒的 enabled/interval/max_per_run、存储的 GC 阈值、
                // 演化的 cron / max_items_per_run）
                config: app_config.as_ref().clone(),
                usage_stats: state.usage_stats(),
                snapshot_manager: state.snapshot_manager(),
                session_manager: state.session_manager(),
                notification_sink: notification::global_notification_sink(),
                // 复用共享 ModelServices（不重复创建 HTTP client 池）
                chat: state.shared_model_services().await?.chat,
                chat_model: state::resolve_chat_model(&app_config),
                working_sets: Some(state.working_sets()),
                // 自演化综述执行器（ADR-017）：**server 侧业务实现**（在专用演化会话里
                // 跑一轮主 agent 产出 diff 计划），必须留在本库；插件只认识 core 的
                // `EvolutionReviewExecutor` 契约。这正是 D2 的形状——装配期注入
                // `Arc<dyn Trait>`：server 构造实现 → 放进能力集合 → 插件用它构造任务。
                // `enabled = false` → `None`（插件因此不注册演化任务）——与迁移前
                // `if cfg.evolution.enabled` 门控同一语义。
                evolution_executor: if app_config.evolution.enabled {
                    Some(Arc::new(AgentEvolutionExecutor::new(
                        state.agent_lock(),
                        state.working_sets(),
                        state::resolve_chat_model(&app_config),
                        app_config.evolution.review_role.clone(),
                    )))
                } else {
                    None
                },
            };

            for registration in task_assembly.manifests().tasks(&task_services) {
                scheduler
                    .register_task(TaskDefinition::new(
                        registration.name,
                        registration.display_name,
                        registration.interval_secs,
                        registration.handler,
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
            state.attach_agent_task_manager(Some(manager.clone())).await;
            // 组件工具（server 自有能力：`schedule_task`）注入：**唯一装配点**。
            //
            // 与 `reload_agent` 共用 `component_dynamic_tools()`——此前这里是两段
            // 手写 `register_dynamic_tools`，热重载重建 agent 时不会重放，
            // 工具静默消失（`tool: 未知工具：schedule_task`）直到重启进程。
            // 通道创建与消费者 spawn 已收口进 `attach_agent_task_manager`。
            //
            // `todo` / `goal` 已迁至 `plugins/tools-todo`（ADR-049 期 1），
            // 由插件清单在 `create_app_inner` 里注册——server 库不依赖插件 crate。
            state
                .agent_lock()
                .read()
                .await
                .register_dynamic_tools(state.component_dynamic_tools().await)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 启动服务必须带结构化存储工厂——否则会静默落到进程内存后端（内容不落盘）。
    ///
    /// 判别力：删掉 `start_server_inner` 入口的那道守卫，本测试立刻变红（默认装配会
    /// 一路走到底、真的去建表并开始监听）。这是「忘记装配」从运行期隐患变成启动期
    /// 硬失败的锁。
    #[tokio::test]
    async fn test_start_server_requires_storage_factory() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定临时端口");
        let err = start_server_inner(
            listener,
            tianyan::config::TianyanConfig::default(),
            None,
            AssemblyServices::default(),
        )
        .await
        .expect_err("未装配 StorageBackendFactory 时启动必须失败");
        assert!(
            err.is_invalid_input(),
            "错误类别应为 invalid_input（ADR-014 语义谓词，不做消息文本匹配）：{err}"
        );
        assert!(
            format!("{err}").contains("StorageBackendFactory"),
            "错误信息须点明缺的是哪个工厂（否则装配缺陷难以定位）：{err}"
        );
    }

    /// 启动服务的**第二道**硬前置：上下文策略工厂必须注入。
    ///
    /// 判别力：删掉 `start_server_inner` 的 `has_context_factory()` 守卫，本测试立刻变红
    /// （会一路走到底：建库、建 VFS、建 agent、开始监听）。存储工厂在这里显式注入，
    /// 让本测试只锁「上下文」这一道——两道守卫因此各有独立的红/绿判据。
    #[tokio::test]
    async fn test_start_server_requires_context_factory() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定临时端口");
        let storage_factory: StorageBackendFactory = Arc::new(|_config, _database| {
            let backend: Arc<dyn tianyan::vfs::StorageBackend> =
                Arc::new(assembly::EphemeralStorageBackend::new());
            Ok(backend)
        });
        let err = start_server_inner(
            listener,
            tianyan::config::TianyanConfig::default(),
            None,
            AssemblyServices {
                storage: StorageAssemblies::default().with_storage_factory(storage_factory),
                ..Default::default()
            },
        )
        .await
        .expect_err("未装配 ContextFactory 时启动必须失败");
        assert!(
            err.is_invalid_input(),
            "错误类别应为 invalid_input（ADR-014 语义谓词，不做消息文本匹配）：{err}"
        );
        assert!(
            format!("{err}").contains("ContextFactory"),
            "错误信息须点明缺的是哪个工厂（否则装配缺陷难以定位）：{err}"
        );
    }

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

    /// planning 技能提示词的工具清单不得漂移。
    ///
    /// 残留已删除工具名（如 v0.5.8 移除的 `lsp`）或在新增工具后漏补，都会
    /// 引导模型调用不存在的工具（白跑一轮 + 用户可见失败）。解析方式：从
    /// 清单起始（`read_file`）到右括号之间的 token（斜杠/空白分隔），逐个
    /// 断言存在于内置工具表（`builtin_tool_names` 跨 crate 单一事实源）。
    #[test]
    fn test_planning_skill_tool_mentions_are_registered() {
        let content = PLANNING_SKILL_CONTENT;
        let start = content
            .find("read_file")
            .expect("planning 只读清单应以 read_file 开头");
        let tail = &content[start..];
        let end = tail.find('）').unwrap_or(tail.len());
        let mentions: Vec<&str> = tail[..end]
            .split(|c: char| c == '/' || c.is_whitespace())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .collect();
        assert!(!mentions.is_empty(), "应能从 planning 提示词解析出工具清单");
        // 已知工具全集（内置 + 已迁插件）：提示词清单里的工具可能来自插件
        // （read_file / glob 等已迁至 plugins/tools-fs，core 侧无法枚举）。
        let registered = tianyan::agent::known_tool_names();
        for m in &mentions {
            assert!(
                registered.contains(m),
                "planning 技能提示词含未注册工具：{m}（工具删除/改名后需同步提示词）"
            );
        }
    }
}
