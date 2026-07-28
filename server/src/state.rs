//! Application State Module
//!
//! 提供精简版的应用状态管理，只负责状态存储和访问。
//! 构建逻辑已委托给 AgentBuilderFactory。

// 标准库
use std::sync::Arc;
use std::time::Duration;

// 外部 crate
use tokio::sync::{RwLock, Semaphore};
use tokio::time::timeout;

// 内部 crate
use tianyan::agent::AgentCoordinator;
use tianyan::config::TianyanConfig;
use tianyan::knowledge::{IngestorConfig, KnowledgeIngestor};
use tianyan::memory::{ExtractionConfig, MemoryExtractor};
use tianyan::session::{PersistentSessionManager, SessionManager};
use tianyan::observability::usage_stats::UsageStats;
use tianyan::observability::sqlite_db::SqliteDb;
use tianyan::skills::{
    register_builtin_skills, ExecutorConfig, SkillExecutor, SkillManager, SkillRegistry,
};
use tianyan::vfs::{SummaryEngine, VirtualFileSystemImpl};
use tianyan::{Result as TianyanResult, TianyanError};

use crate::agent_builder::{create_model_services, AgentBuilderFactory};

// 常量定义
const MAX_PENDING_TASKS: usize = 1000;
const SHUTDOWN_TIMEOUT_SECS: u64 = 30;
const TASK_CHECK_INTERVAL_MS: u64 = 100;

// 类型别名
type SharedConfig = Arc<RwLock<TianyanConfig>>;
type SharedAgent = Arc<RwLock<Arc<dyn AgentCoordinator>>>;

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
    /// 待处理任务信号量（控制并发数）
    pending_tasks: Arc<Semaphore>,
    /// 虚拟文件系统（所有组件共享）
    vfs: Arc<VirtualFileSystemImpl>,
    /// 技能注册表
    skill_registry: Arc<RwLock<SkillRegistry>>,
    /// 技能执行器
    skill_executor: Arc<SkillExecutor>,
    /// 使用统计追踪器
    usage_stats: Arc<UsageStats>,
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

        // VFS 技能自动发现：加载之前学到的技能
        {
            let skill_manager = SkillManager::new(vfs.clone());
            if let Ok(skills) = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { skill_manager.list_available_skills().await })
            }) {
                tracing::info!(count = skills.len(), "从VFS加载了已学习的技能");
            }
        }

        // 初始化使用统计（共享 SQLite 数据库）
        let db_path = config.storage.data_dir.join("usage_stats.db");
        let sqlite_db = SqliteDb::open(db_path)
            .map_err(|e| TianyanError::StorageBackend(format!("创建 SQLite 数据库失败：{}", e)))?;
        sqlite_db.init_all_schemas().await.map_err(|e| {
            TianyanError::StorageBackend(format!("初始化 SQLite 表失败：{}", e))
        })?;
        let usage_stats = UsageStats::new(sqlite_db).map_err(|e| {
            TianyanError::StorageBackend(format!("创建 UsageStats 失败：{}", e))
        })?;

        // 构建 Agent（传入 vfs + 技能组件）
        let agent = AgentBuilderFactory::build_agent_or_wizard(
            &config,
            vfs.clone(),
            skill_registry.clone(),
            skill_executor.clone(),
            usage_stats.clone(),
        )
        .await?;

        // 创建持久化会话管理器
        let session_manager = Arc::new(PersistentSessionManager::new(vfs.clone()));

        let pending_tasks = Arc::new(Semaphore::new(MAX_PENDING_TASKS));

        Ok(Self {
            agent: Arc::new(RwLock::new(agent)),
            config: Arc::new(RwLock::new(config)),
            session_manager,
            pending_tasks,
            vfs,
            skill_registry,
            skill_executor,
            usage_stats,
        })
    }

    /// 获取当前 Agent 实例
    ///
    /// # Returns
    /// * `Arc<dyn AgentCoordinator>` - 当前 Agent 实例的 Arc 引用
    pub async fn agent(&self) -> Arc<dyn AgentCoordinator> {
        self.agent.read().await.clone()
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
        let new_agent = AgentBuilderFactory::build_agent_or_wizard(
            &config,
            self.vfs.clone(),
            self.skill_registry.clone(),
            self.skill_executor.clone(),
            self.usage_stats.clone(),
        )
        .await?;

        *self.agent.write().await = new_agent;

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

    /// 创建摘要引擎
    ///
    /// # Returns
    /// * `TianyanResult<Arc<SummaryEngine>>` - 摘要引擎实例
    pub fn create_summary_engine(&self) -> TianyanResult<Arc<SummaryEngine>> {
        let config = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.config.read().await.clone() })
        });

        let model_services = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { create_model_services(&config).await })
        })
        .map_err(|e| TianyanError::ModelService(format!("模型服务创建失败：{}", e)))?;

        let chat_model = config
            .models
            .resolve(tianyan::config::ModelCapability::Chat)
            .map(|r| r.model)
            .unwrap_or_default();
        let embedding_model = config
            .models
            .resolve(tianyan::config::ModelCapability::TextEmbedding)
            .map(|r| r.model)
            .unwrap_or_default();

        // 创建 SummaryEngine
        let summary_engine = SummaryEngine::new(
            model_services.chat,
            model_services.embedding,
            &chat_model,
            &embedding_model,
        );

        Ok(Arc::new(summary_engine))
    }

    /// 创建记忆提取器
    ///
    /// # Returns
    /// * `TianyanResult<Arc<MemoryExtractor>>` - 记忆提取器实例
    pub fn create_memory_extractor(&self) -> TianyanResult<Arc<MemoryExtractor>> {
        let config = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.config.read().await.clone() })
        });

        let model_services = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { create_model_services(&config).await })
        })
        .map_err(|e| TianyanError::ModelService(format!("模型服务创建失败：{}", e)))?;

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

    /// 创建知识导入器。
    pub fn create_knowledge_ingestor(&self) -> TianyanResult<KnowledgeIngestor> {
        let config = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.config.read().await.clone() })
        });

        let model_services = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { create_model_services(&config).await })
        })
        .map_err(|e| TianyanError::ModelService(format!("模型服务创建失败：{}", e)))?;

        let vfs: Arc<dyn tianyan::vfs::VirtualFileSystem> = self.vfs.clone();

        let chat_model = config
            .models
            .resolve(tianyan::config::ModelCapability::Chat)
            .map(|r| r.model)
            .unwrap_or_default();
        let embedding_model = config
            .models
            .resolve(tianyan::config::ModelCapability::TextEmbedding)
            .or_else(|| config.models.resolve(tianyan::config::ModelCapability::MultimodalEmbedding))
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
    /// 等待所有待处理任务完成，带有超时保护。
    ///
    /// # Returns
    /// * `TianyanResult<()>` - 成功返回 Ok，超时也返回 Ok（仅记录警告）
    ///
    /// # Errors
    /// * 正常情况下不会返回错误
    pub async fn shutdown(&self) -> TianyanResult<()> {
        tracing::info!("开始关闭应用...");

        match timeout(
            Duration::from_secs(SHUTDOWN_TIMEOUT_SECS),
            self.wait_for_pending_tasks(),
        )
        .await
        {
            Ok(_) => {
                tracing::info!("所有任务已完成，应用已关闭");
                Ok(())
            }
            Err(_) => {
                tracing::warn!("关闭超时 ({}s)，仍有任务未完成", SHUTDOWN_TIMEOUT_SECS);
                Ok(())
            }
        }
    }

    /// 等待所有待处理任务完成
    async fn wait_for_pending_tasks(&self) {
        let initial_available = self.pending_tasks.available_permits();
        let pending_count = MAX_PENDING_TASKS - initial_available;

        if pending_count == 0 {
            tracing::debug!("没有待处理任务");
            return;
        }

        tracing::info!("等待 {} 个待处理任务完成...", pending_count);

        while self.pending_tasks.available_permits() < MAX_PENDING_TASKS {
            tokio::time::sleep(Duration::from_millis(TASK_CHECK_INTERVAL_MS)).await;
        }

        tracing::info!("所有待处理任务已完成");
    }
}
