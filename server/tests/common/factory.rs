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
        enabled: true,
        headers: std::collections::HashMap::new(),
        thinking_field: None,
        dialect: None,
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
    }
}
