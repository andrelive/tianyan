//! Application State Module
//!
//! 提供精简版的应用状态管理，只负责状态存储和访问。
//! 构建逻辑已委托给 AgentBuilderFactory。

// 标准库
use std::sync::Arc;

// 外部 crate
use tokio::sync::RwLock;

// 内部 crate
use tianyan::agent::AgentCoordinator;
use tianyan::config::TianyanConfig;
use tianyan::knowledge::{IngestorConfig, KnowledgeIngestor};
use tianyan::memory::{ExtractionConfig, MemoryExtractor};
use tianyan::observability::usage_stats::UsageStats;
use tianyan::scheduler::TaskScheduler;
use tianyan::session::{PersistentSessionManager, SessionManager};
use tianyan::skills::{
    register_builtin_skills, ExecutorConfig, SkillExecutor, SkillManager, SkillRefresher,
    SkillRegistry,
};
use tianyan::snapshot::SnapshotManager;
use tianyan::vfs::backend::sqlite_db::SqliteDb;
use tianyan::vfs::{SummaryEngine, VirtualFileSystemImpl};
use tianyan::{Result as TianyanResult, TianyanError};

use crate::agent_builder::{create_model_services, AgentBuilderFactory};
use crate::mcp_bridge::McpToolManager;

/// 构造模型服务创建错误（统一错误前缀，避免调用点重复拼装）。
fn model_services_error(e: impl std::fmt::Display) -> TianyanError {
    TianyanError::Custom(format!("模型服务错误：模型服务创建失败：{e}"))
}

// 类型别名
type SharedConfig = Arc<RwLock<TianyanConfig>>;
type SharedAgent = Arc<RwLock<Arc<dyn AgentCoordinator>>>;

/// 技能注册表同步句柄。
///
/// 会话边界刷新的承载者：新会话创建时把 VFS 中已学习技能（GEPA 产物）
/// 增量注册进 SkillRegistry。刷新是幂等的（仅注册新技能），失败不影响对话。
#[derive(Clone)]
pub struct SkillSync {
    manager: Arc<SkillManager>,
    registry: Arc<RwLock<SkillRegistry>>,
}

impl SkillSync {
    /// 将 VFS 已学习技能增量注册进注册表，返回新注册数。
    pub async fn refresh(&self) -> TianyanResult<usize> {
        let mut registry = self.registry.write().await;
        self.manager.refresh_registry(&mut registry).await
    }
}

/// SkillSync 作为核心层 [`SkillRefresher`] 的实现：
/// 压缩（会话转换点）时被 Agent 调用，增量注册 VFS 学习技能。
#[async_trait::async_trait]
impl SkillRefresher for SkillSync {
    async fn refresh_skills(&self) -> TianyanResult<usize> {
        self.refresh().await
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
    /// 技能注册表
    skill_registry: Arc<RwLock<SkillRegistry>>,
    /// 技能执行器
    skill_executor: Arc<SkillExecutor>,
    /// 技能管理器（VFS 命名空间访问；会话边界刷新已学习技能用）
    skill_manager: Arc<SkillManager>,
    /// 使用统计追踪器
    usage_stats: Arc<UsageStats>,
    /// 工作区快照管理器（配置了 working_directory 时启用）
    snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 模型服务（chat/embedding/vision，全组件共享；配置热更新时重建）
    model_services: Arc<RwLock<tianyan::model::ModelServices>>,
    /// MCP 客户端生命周期管理器（跨 agent reload 保持连接一致）
    mcp_tools: Arc<McpToolManager>,
    /// 定时任务调度器（无启用的模型 Provider 时为 None，在 `start_server` 中装配）
    scheduler: Arc<RwLock<Option<Arc<TaskScheduler>>>>,
    /// 全系统共享 SQLite（ADR-005；后台任务持久化/唤醒，ADR-013）
    sqlite_db: SqliteDb,
    /// 事件总线（T1 事件驱动：文件监听/webhook → 处理器/唤醒）
    event_bus: Arc<tianyan::events::EventBus>,
    /// 服务关停标志（Ctrl+C / SIGTERM / 桌面端退出时置位）。
    ///
    /// 流式请求据此取消进行中的 AgentLoop，使优雅关停不被长连接阻塞。
    shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
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
        sqlite_db: SqliteDb,
    ) -> TianyanResult<Self> {
        // 初始化技能注册表和执行器
        let mut skill_registry = SkillRegistry::new();
        let skill_config = {
            let sec = &config.security;
            let mut cfg = ExecutorConfig::new();
            cfg.skill_file_read_max_size = sec.skill_file_read_max_size;
            cfg.skill_file_read_timeout_secs = sec.skill_file_read_timeout_secs;
            cfg.skill_file_write_max_size = sec.skill_file_write_max_size;
            cfg.skill_file_write_timeout_secs = sec.skill_file_write_timeout_secs;
            cfg.skill_file_list_max_entries = sec.skill_file_list_max_entries;
            cfg.skill_http_timeout_secs = sec.skill_http_timeout_secs;
            cfg.skill_command_timeout_secs = sec.skill_command_timeout_secs;
            cfg
        };
        register_builtin_skills(&mut skill_registry, &skill_config);
        let skill_registry = Arc::new(RwLock::new(skill_registry));
        let skill_executor = Arc::new(SkillExecutor::new(skill_registry.clone(), skill_config));

        // VFS 技能自动发现：把之前学到的技能注册进 SkillRegistry，
        // 使它们可被列出、检索（GEPA 学习回路闭环）。
        // 启动时全量加载一次；运行中新进化的技能由会话边界刷新
        // （SkillSync::refresh，见 ChatService 接线）增量注册。
        let skill_manager = Arc::new(SkillManager::new(vfs.clone()));
        {
            let mut registry = skill_registry.write().await;
            match skill_manager.refresh_registry(&mut registry).await {
                Ok(registered) => {
                    tracing::info!(count = registered, "已注册 VFS 学习技能");
                }
                Err(e) => tracing::warn!(error = %e, "加载已学习技能失败"),
            }
        }

        // 初始化使用统计（复用全系统共享的 SqliteDb，ADR-005：单连接）
        let usage_stats = UsageStats::new(sqlite_db.clone())?;

        // 初始化工作区快照管理器（配置了 working_directory 时启用）
        let snapshot_manager = config.agent.working_directory.clone().map(|workdir| {
            let root = config.storage.data_dir.join("snapshots");
            let workdir = std::path::PathBuf::from(workdir);
            tracing::info!(
                "工作区快照已启用: 目录={}, 存储={}",
                workdir.display(),
                root.display()
            );
            Arc::new(SnapshotManager::new(root, workdir))
        });

        // MCP 工具桥接：连接配置中的 MCP 服务器，生成动态工具
        // （图片类返回，如浏览器截图，落盘到 {data_dir}/mcp_images/）
        let mcp_tools = Arc::new(McpToolManager::with_image_dir(
            config.storage.data_dir.join("mcp_images"),
        ));
        mcp_tools.sync(&config.mcp.servers).await;
        let dynamic_tools = mcp_tools.bridges().await;
        if !dynamic_tools.is_empty() {
            tracing::info!(tool_count = dynamic_tools.len(), "MCP 动态工具已就绪");
        }

        // 构建 Agent（传入 vfs + 技能组件 + 共享模型服务 + MCP 动态工具）
        let model_services = create_model_services(&config)
            .await
            .map_err(model_services_error)?;
        let agent = AgentBuilderFactory::build_agent_or_wizard(
            &config,
            model_services.clone(),
            vfs.clone(),
            skill_executor.clone(),
            usage_stats.clone(),
            snapshot_manager.clone(),
            Arc::new(SkillSync {
                manager: skill_manager.clone(),
                registry: skill_registry.clone(),
            }),
            dynamic_tools,
            Some(sqlite_db.clone()),
            crate::notification::global_notification_sink(),
        )
        .await?;

        // 创建持久化会话管理器
        let session_manager = Arc::new(PersistentSessionManager::new(vfs.clone()));

        Ok(Self {
            agent: Arc::new(RwLock::new(agent)),
            config: Arc::new(RwLock::new(config)),
            session_manager,
            vfs,
            skill_registry,
            skill_executor,
            skill_manager,
            usage_stats,
            snapshot_manager,
            model_services: Arc::new(RwLock::new(model_services)),
            mcp_tools,
            scheduler: Arc::new(RwLock::new(None)),
            sqlite_db,
            event_bus: Arc::new(tianyan::events::EventBus::new()),
            shutdown_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
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

    /// 获取技能注册表同步句柄。
    ///
    /// ChatService 在新会话创建时调用 [`SkillSync::refresh`]，把 GEPA 在
    /// 运行期间进化出的新技能增量注册进 SkillRegistry —— 新会话立即可用，
    /// 会话内保持冻结（system 前缀稳定，prompt 缓存不失效）。
    pub fn skill_sync(&self) -> SkillSync {
        SkillSync {
            manager: self.skill_manager.clone(),
            registry: self.skill_registry.clone(),
        }
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
        let dynamic_tools = self.mcp_tools.bridges().await;
        // 配置可能已变化（模型/API Key），重建共享模型服务
        let model_services = create_model_services(&config)
            .await
            .map_err(model_services_error)?;
        let new_agent = AgentBuilderFactory::build_agent_or_wizard(
            &config,
            model_services.clone(),
            self.vfs.clone(),
            self.skill_executor.clone(),
            self.usage_stats.clone(),
            self.snapshot_manager.clone(),
            Arc::new(self.skill_sync()),
            dynamic_tools,
            Some(self.sqlite_db.clone()),
            crate::notification::global_notification_sink(),
        )
        .await?;

        *self.agent.write().await = new_agent;
        *self.model_services.write().await = model_services;

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

    /// 获取虚拟文件系统
    ///
    /// # Returns
    /// * `Arc<VirtualFileSystemImpl>` - VFS 实例
    pub fn vfs(&self) -> Arc<VirtualFileSystemImpl> {
        self.vfs.clone()
    }

    /// 获取工作区快照管理器（未配置 working_directory 时为 None）。
    pub fn snapshot_manager(&self) -> Option<Arc<SnapshotManager>> {
        self.snapshot_manager.clone()
    }

    /// 获取配置
    ///
    /// # Returns
    /// * `Arc<RwLock<TianyanConfig>>` - 配置实例
    pub fn config(&self) -> Arc<RwLock<TianyanConfig>> {
        self.config.clone()
    }

    /// 获取技能注册表
    ///
    /// # Returns
    /// * `Arc<RwLock<SkillRegistry>>` - 技能注册表实例
    pub fn skill_registry(&self) -> Arc<RwLock<SkillRegistry>> {
        self.skill_registry.clone()
    }

    /// 获取技能执行器
    ///
    /// # Returns
    /// * `Arc<SkillExecutor>` - 技能执行器实例
    pub fn skill_executor(&self) -> Arc<SkillExecutor> {
        self.skill_executor.clone()
    }

    /// 获取使用统计追踪器
    ///
    /// # Returns
    /// * `Arc<UsageStats>` - 使用统计追踪器实例
    pub fn usage_stats(&self) -> Arc<UsageStats> {
        self.usage_stats.clone()
    }

    /// 装配定时任务调度器（`start_server` 在创建并注册任务后调用）。
    pub async fn attach_scheduler(&self, scheduler: Option<Arc<TaskScheduler>>) {
        *self.scheduler.write().await = scheduler;
    }

    /// 获取定时任务调度器（未装配或无 Provider 时为 None）。
    pub async fn scheduler(&self) -> Option<Arc<TaskScheduler>> {
        self.scheduler.read().await.clone()
    }

    /// 获取共享模型服务（配置热更新后自动指向新实例）。
    async fn shared_model_services(&self) -> TianyanResult<tianyan::model::ModelServices> {
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

        let chat_model = config
            .models
            .resolve(tianyan::config::ModelCapability::Chat)
            .map(|r| r.model)
            .unwrap_or_default();
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

        let chat_model = config
            .models
            .resolve(tianyan::config::ModelCapability::Chat)
            .map(|r| r.model)
            .unwrap_or_default();

        let extractor = MemoryExtractor::new(
            model_services.chat,
            ExtractionConfig {
                model: chat_model,
                ..ExtractionConfig::default()
            },
        );
        Ok(Arc::new(extractor))
    }

    /// 创建知识导入器（复用共享模型服务）。
    pub async fn create_knowledge_ingestor(&self) -> TianyanResult<KnowledgeIngestor> {
        let config = self.config.read().await.clone();

        let model_services = self.shared_model_services().await?;

        let vfs: Arc<dyn tianyan::vfs::VirtualFileSystem> = self.vfs.clone();

        let chat_model = config
            .models
            .resolve(tianyan::config::ModelCapability::Chat)
            .map(|r| r.model)
            .unwrap_or_default();
        let embedding_model = config
            .models
            .resolve(tianyan::config::ModelCapability::TextEmbedding)
            .or_else(|| {
                config
                    .models
                    .resolve(tianyan::config::ModelCapability::MultimodalEmbedding)
            })
            .map(|r| r.model)
            .unwrap_or_default();
        let vision_model = config
            .models
            .resolve(tianyan::config::ModelCapability::Vision)
            .map(|r| r.model)
            .unwrap_or_default();

        let ingestor = KnowledgeIngestor::new(
            IngestorConfig::new()
                .with_embedding_model(&embedding_model)
                .with_summary_model(&chat_model)
                .with_vision_model(&vision_model),
            model_services.chat,
            model_services.embedding,
            model_services.vision,
            vfs,
        );
        Ok(ingestor)
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
        // 断开所有 MCP 服务器连接（显式 shutdown，McpClient::drop 不会自动清理子进程）
        self.mcp_tools.shutdown().await;
        tracing::info!("应用已关闭");
        Ok(())
    }
}
