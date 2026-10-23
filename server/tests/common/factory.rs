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

/// 集成测试用的**真实装配**（ADR-049 期 2 序 4 / 序 5a；**期 4 起只剩一行**）。
///
/// ## 期 4：这里不再手抄装配清单
///
/// 期 4 之前本函数是**第二份**手抄的 9 个工厂（`app` bin 与 `tauri` 各一份，合计
/// 三份副本），每份都靠注释「与 `app` / `tauri` 同一份插件实现」互相提醒——插槽
/// 签名一变，先改的那份能编、后改的那份报错；而**顺序纪律**在某一份里被单独调整时
/// 连编译都不会失败。期 4 把 11 个工厂与两份清单收敛到 `tianyan_assembly`（唯一
/// 实现），本函数只取 [`tianyan_assembly::policies`]。
///
/// **为什么是 `policies()` 而不是 `full()`**：`full()` 额外含两个「环境相关」插槽
/// ——动态工具清单（`create_app` 路径本就不注册插件工具）与真实向量存储（LanceDB
/// 会为每个测试建目录）。集成测试不需要它们；而 `policies()` 里的 9 个插槽与生产
/// **完全同形**：
///
/// - 存储后端按 `config.storage.backend` 分发到真实 `SqliteBackend` /
///   `LocalFileBackend`——与生产装配器同一份选择逻辑（D5）；
/// - 任务清单是**真实插件项**（迁移前 `start_server*` 会注册那 5 个 + 演化任务，
///   `splice(1..1, …)` 的跨插件位置也只在装配层）；
/// - 审批 / 上下文 / 评审 / provider / 工具管线全部注入真实实现。
///
/// **不要用 `AssemblyServices::default()` 驱动集成测试**：那会落到内存兜底后端
/// （`EphemeralStorageBackend`），测试虽然能过，却不再覆盖真实 SQL 语句、表结构
/// 与文件目录语义——而 `StorageBackendType` 的默认值就是 `Sqlite`，也就是说退化
/// 成兜底等于**判别力回退**（序 4 迁移时一度如此，已在本函数修正）。
///
/// ## 依赖形状：dev-dependency 环（**Cargo 允许**）
///
/// `tianyan-assembly` 依赖 `tianyan-server`（取 `AssemblyServices` 与 11 个插槽
/// 类型别名），而本包把它作为 **dev-dependency**：`server --dev--> assembly -->
/// server`。这不违反 Cargo 的环检测——dev-dep 不参与**构建**图的环检测，测试目标
/// 的依赖图里 server lib 只编一份，与普通 dev-dep 无区别。
///
/// 它**不是** core 那种「同源包含 + dev-dep 导致编译两份」的陷阱
/// （见 `core/src/vfs/backend/mod.rs`）：那里是 `#[path]` 把同一批源文件编进两个
/// crate，于是 `StorageBackend` 出现两个不兼容实例；这里是两个独立 crate 的正常
/// 依赖，`AssemblyServices` 始终只有 server 里那一份定义。
///
/// 代价（已实测并接受）：server 的测试目标现在会连带编译 `tianyan-assembly` 的全部
/// 依赖（含 LanceDB 链），首次 `cargo test -p tianyan-server` 因此变慢；换来的是
/// 「9 个工厂只有一份实现」——插槽变化时在**编译期**暴露，而不是等某个装配器运行时
/// 缺件（静默降级最贵的那种 bug）。
pub fn test_assembly() -> tianyan_server::AssemblyServices {
    tianyan_assembly::policies()
}
