//! 测试工厂 — 快速创建可复用的测试对象
//!
//! 工具库：部分工厂函数仅被其他测试函数间接使用，保留供后续测试复用。

#![allow(dead_code)]

use tianyan::config::{
    AgentConfig, LoggingConfig, MemoryConfig, ModelCapability, ModelEntry, ModelPreferences,
    ModelRef, ModelsConfig, ProviderConfig, RetrievalConfig, SecurityConfig, StorageConfig,
    TianyanConfig, VectorStorageConfig,
};

pub fn test_agent_config() -> AgentConfig {
    AgentConfig {
        enable_skills: true,
        enable_memory: true,
        stream_responses: false,
        enable_thinking: false,
        default_top_k: 3,
        learned_rules_top_k: 5,
        learned_rules_max_tokens: 800,
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
            },
            ModelEntry {
                name: "text-embedding-3-small".to_string(),
                capabilities: vec![ModelCapability::TextEmbedding],
            },
            ModelEntry {
                name: "gpt-4-vision-preview".to_string(),
                capabilities: vec![ModelCapability::Vision],
            },
        ],
        timeout: 30,
        enabled: true,
        headers: std::collections::HashMap::new(),
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
    StorageConfig {
        data_dir: std::env::temp_dir().join("tianyan-test"),
        auto_cleanup: false,
        cleanup_days: 30,
        max_storage_size: 5368709120,
        vector: VectorStorageConfig {
            collection_name: "tianyan_test".to_string(),
            vector_dimension: 768,
        },
    }
}

pub fn test_tianyan_config() -> TianyanConfig {
    TianyanConfig {
        agent: test_agent_config(),
        storage: test_storage_config(),
        models: test_models_config(),
        logging: LoggingConfig::default(),
        security: SecurityConfig::default(),
        memory: MemoryConfig::default(),
        retrieval: RetrievalConfig::default(),
        mcp: Default::default(),
    }
}
