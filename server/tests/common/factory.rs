//! 测试工厂 — 快速创建可复用的测试对象
//!
//! 工具库：部分工厂函数仅被其他测试函数间接使用，保留供后续测试复用。

#![allow(dead_code)]

use tianyan::common::logging::LoggingConfig;
use tianyan::config::{
    AgentConfig, MemoryConfig, ModelCapability, ModelEntry, ModelPreferences, ModelRef,
    ModelsConfig, ProviderConfig, RetrievalConfig, SecurityConfig, StorageConfig, TianyanConfig,
    VectorStorageConfig,
};

pub fn test_agent_config() -> AgentConfig {
    AgentConfig {
        default_top_k: 3,
        learned_rules_top_k: 5,
        ..Default::default()
    }
}

pub fn test_provider(name: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        endpoint: "http://localhost:11434/v1".to_string(),
        api_key: Some("test-key".to_string()),
        models: vec![
            ModelEntry {
                name: "test-model".to_string(),
                capabilities: vec![ModelCapability::Chat],
                ..Default::default()
            },
            ModelEntry {
                name: "text-embedding-3-small".to_string(),
                capabilities: vec![ModelCapability::TextEmbedding],
                ..Default::default()
            },
            ModelEntry {
                name: "gpt-4-vision-preview".to_string(),
                capabilities: vec![ModelCapability::Vision],
                ..Default::default()
            },
        ],
        timeout: 30,
        first_token_timeout: 300,
        enabled: true,
        headers: std::collections::HashMap::new(),
        thinking_field: None,
        dialect: None,
        ..Default::default()
    }
}

pub fn test_models_config() -> ModelsConfig {
    ModelsConfig {
        providers: vec![test_provider("mock-service")],
        preferences: ModelPreferences {
            chat: Some(ModelRef {
                provider: "mock-service".to_string(),
                model: "test-model".to_string(),
            }),
            embedding: Some(ModelRef {
                provider: "mock-service".to_string(),
                model: "text-embedding-3-small".to_string(),
            }),
            vision: Some(ModelRef {
                provider: "mock-service".to_string(),
                model: "gpt-4-vision-preview".to_string(),
            }),
        },
    }
}

pub fn test_storage_config() -> StorageConfig {
    test_storage_config_with_data_dir(std::env::temp_dir().join("tianyan-test"))
}

pub fn test_storage_config_with_data_dir(data_dir: std::path::PathBuf) -> StorageConfig {
    StorageConfig {
        data_dir,
        backend: Default::default(),
        sqlite_path: None,
        max_storage_size: 5368709120,
        auto_cleanup: false,
        cleanup_days: 30,
        vector: VectorStorageConfig {
            collection_name: "tianyan_test".to_string(),
            vector_dimension: 768,
        },
    }
}

/// 使用默认临时目录创建测试配置（固定路径：多个测试并发时请改用
/// [`test_tianyan_config_with_data_dir`] 传入独立目录）。
pub fn test_tianyan_config() -> TianyanConfig {
    test_tianyan_config_with_data_dir(std::env::temp_dir().join("tianyan-test"))
}

/// 使用指定数据目录创建测试配置。
///
/// 集成测试驱动真实 `create_app` 时每个测试应传入独立的 `tempdir`，
/// 避免 SQLite/LanceDB 数据目录被并发测试互相污染。
pub fn test_tianyan_config_with_data_dir(data_dir: std::path::PathBuf) -> TianyanConfig {
    TianyanConfig {
        agent: test_agent_config(),
        agent_roles: Default::default(),
        storage: test_storage_config_with_data_dir(data_dir),
        models: test_models_config(),
        logging: LoggingConfig::default(),
        security: SecurityConfig::default(),
        memory: MemoryConfig::default(),
        retrieval: RetrievalConfig::default(),
        mcp: Default::default(),
        web: Default::default(),
        clipboard: Default::default(),
        events: Default::default(),
        evolution: Default::default(),
        reminder: Default::default(),
        executor: Default::default(),
        tool_output: Default::default(),
    }
}

/// 集成测试用的**真实装配**（ADR-049，期 2 序 4 / 序 5a）。
///
/// 存储后端按 `config.storage.backend` 分发到真实的 `SqliteBackend` /
/// `LocalFileBackend`——与生产装配器（`app` bin / `tauri`）同一份选择逻辑，
/// 只是把「哪个 adapter」的 match 从 server 库搬到了装配侧（D5）。
///
/// 内置任务清单同样注入真实实现（序 5a）：迁移前 `start_server*` **会**注册那 5 个
/// 任务，若集成测试走「未装配 → 不注册」的降级路径，测的就不是生产的形态
/// （与序 4 的「兜底后端」同一条纪律：**不要用 `AssemblyServices::default()` 驱动
/// 集成测试**）。`create_app` 路径本就不注册任务，因此那里的行为不受影响。
///
/// **不要用 `AssemblyServices::default()` 驱动集成测试**：那会落到内存兜底后端
/// （`EphemeralStorageBackend`），测试虽然能过，却不再覆盖真实 SQL 语句、表结构
/// 与文件目录语义——而 `StorageBackendType` 的默认值就是 `Sqlite`，也就是说退化
/// 成兜底等于**判别力回退**（序 4 迁移时一度如此，已在本函数修正）。
///
/// 注意：本函数能成立的前提是 `tianyan-storage-sqlite` 只是 server 的 dev-dep
/// 且**不被 server 依赖**（无环）；core 的同类需求不能照抄——core 若这样做会被
/// 编译两份，trait 与类型出现两个不兼容实例（见 `core/src/vfs/backend/mod.rs`）。
pub fn test_assembly() -> tianyan_server::AssemblyServices {
    use std::sync::Arc;

    use tianyan::config::StorageBackendType;
    use tianyan::vfs::StorageBackend;

    let factory: tianyan_server::StorageBackendFactory = Arc::new(|config, database| {
        let storage: Arc<dyn StorageBackend> = match config.backend {
            StorageBackendType::Local => Arc::new(tianyan_storage_sqlite::LocalFileBackend::new(
                config.clone(),
            )),
            StorageBackendType::Sqlite => {
                Arc::new(tianyan_storage_sqlite::SqliteBackend::new(database))
            }
        };
        Ok(storage)
    });

    // 定时任务清单：与 `app` / `tauri` 同一份插件实现（任务名/间隔/启用条件全在插件里）
    // 顺序拼接在装配层（D2-2）：`splice(1..1, …)` = evolution 紧跟 summary_generation。
    let task_factory: tianyan_server::TaskFactory =
        Arc::new(|services: &tianyan_server::TaskServices| {
            let mut registrations = tianyan_task_builtin::tasks(services);
            registrations.splice(1..1, tianyan_task_evolution::tasks(services));
            registrations
        });

    // 审批门控（ADR-049 期 3-T1）：与 `app` / `tauri` 同一份插件实现。「配置 → 自动
    // 审批规则」的映射住插件里（`tianyan_policy_approval::gate`），这里只做类型擦除
    // 包装。**不注入**就会落到「未装配 → 工具直接放行」的降级路径，测的就不是生产
    // 形态（与序 4「不要用 `AssemblyServices::default()` 驱动集成测试」同一条纪律）。
    let approval_factory: tianyan_server::ApprovalGateFactory = Arc::new(
        |security: &SecurityConfig,
         working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
         sink: tianyan::notification::SharedNotificationSink| {
            tianyan_policy_approval::gate(security, working_sets, sink)
        },
    );

    // 上下文策略（ADR-049 期 3-T2）：与 `app` / `tauri` 同一份插件实现。「注入顺序 /
    // 检索参数 / 压缩阈值 / 压缩窗口 ↔ 模型规格联动」全在插件里
    // （`tianyan_policy_context::pipeline`），这里只做类型擦除包装。
    // **不注入**就会落到 core 的空直通兜底（无 system 前缀 + 不压缩），集成测试测的
    // 就不是生产形态（与序 4「不要用 `AssemblyServices::default()` 驱动集成测试」
    // 同一条纪律；`start_server*` 入口另有硬守卫，缺件根本起不来）。
    let context_factory: tianyan_server::ContextFactory =
        Arc::new(|services: &tianyan_server::ContextServices| {
            tianyan_policy_context::pipeline(services)
        });

    tianyan_server::AssemblyServices::new()
        .with_storage_factory(factory)
        .with_task_factory(task_factory)
        .with_approval_factory(approval_factory)
        .with_context_factory(context_factory)
}
