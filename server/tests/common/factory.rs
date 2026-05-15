//! 测试工厂 — 快速创建可复用的测试对象

use tianyan::agent::AgentConfig;
use tianyan::config::{
    LoggingConfig, MemoryConfig, ModelRoutingConfig, ModelServiceConfig, ModelServiceType,
    ModelsConfig, PlannerConfig, RetrievalConfig, SecurityConfig, StorageConfig,
    SummaryServiceConfig, TianyanConfig, VectorStorageConfig,
};
use tianyan::planner::config::ContextConfig;

pub fn test_agent_config() -> AgentConfig {
    AgentConfig {
        enable_skills: true,
        enable_memory: true,
        stream_responses: false,
        enable_thinking: false,
        default_top_k: 3,
        enable_verification: false,
        learned_rules_top_k: 5,
        learned_rules_max_tokens: 800,
        ..Default::default()
    }
}

pub fn test_model_service_config(name: &str) -> ModelServiceConfig {
    ModelServiceConfig {
        name: name.to_string(),
        endpoint: "http://localhost:11434/v1".to_string(),
        api_key: Some("test-key".to_string()),
        default_model: "test-model".to_string(),
        models: vec!["test-model".to_string()],
        service_type: ModelServiceType::default(),
        timeout: 30,
        max_retries: 2,
        enabled: true,
        priority: 0,
        headers: std::collections::HashMap::new(),
    }
}

pub fn test_models_config() -> ModelsConfig {
    ModelsConfig {
        services: vec![test_model_service_config("mock-service")],
        default_chat_model: "test-model".to_string(),
        default_embedding_model: "text-embedding-3-small".to_string(),
        default_vision_model: "gpt-4-vision-preview".to_string(),
        routing: ModelRoutingConfig::default(),
    }
}

pub fn test_storage_config() -> StorageConfig {
    StorageConfig {
        data_dir: std::env::temp_dir().join("tianyan-test"),
        auto_cleanup: false,
        cleanup_days: 30,
        max_storage_size: 5368709120,
        vector: VectorStorageConfig {
            url: "http://localhost:6334".to_string(),
            collection_name: "tianyan_test".to_string(),
            vector_dimension: 768,
        },
    }
}

pub fn test_planner_config() -> PlannerConfig {
    PlannerConfig {
        max_iterations: 5,
        max_depth: 3,
        context_config: ContextConfig {
            age_decay_factor: 0.9,
            cleanup_threshold: 0.3,
            max_context_tokens: 4000,
        },
        executor_max_concurrency: 5,
        max_recovery_attempts: 2,
        enable_recovery: false,
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
        summary_service: SummaryServiceConfig::default(),
        planner: test_planner_config(),
    }
}
