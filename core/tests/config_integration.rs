//! 配置系统集成测试 — 从文件加载、验证、保存、重新加载的完整流程

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tianyan::config::{ModelCapability, ModelEntry, ModelRef, ProviderConfig, TianyanConfig};

fn make_test_config() -> TianyanConfig {
    let mut config = TianyanConfig::default();
    config.models.providers = vec![ProviderConfig {
        name: "test".to_string(),
        endpoint: "http://localhost:11434/v1".to_string(),
        api_key: Some("test-key".to_string()),
        models: vec![ModelEntry {
            name: "test-model".to_string(),
            capabilities: vec![ModelCapability::Chat],
        }],
        timeout: 30,
        enabled: true,
        headers: std::collections::HashMap::new(),
    }];
    config.models.preferences.chat = Some(ModelRef {
        provider: "test".to_string(),
        model: "test-model".to_string(),
    });
    config
}

#[tokio::test]
async fn test_config_save_and_reload_roundtrip() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("tianyan.toml");

    let original = make_test_config();
    original.save_to_file(&config_path).unwrap();
    assert!(config_path.exists());

    let reloaded = TianyanConfig::load_from_file(&config_path).unwrap();
    assert_eq!(reloaded.agent.enable_skills, original.agent.enable_skills);
    assert_eq!(reloaded.agent.enable_memory, original.agent.enable_memory);
    assert_eq!(reloaded.agent.default_top_k, original.agent.default_top_k);
    assert_eq!(
        reloaded.storage.vector.vector_dimension,
        original.storage.vector.vector_dimension
    );
}

#[tokio::test]
async fn test_config_defaults_are_reasonable() {
    let config = make_test_config();
    assert_eq!(config.agent.default_top_k, 5);
    assert_eq!(config.memory.consolidation_interval, 3600);
    assert_eq!(config.retrieval.default_top_k, 10);
}

#[tokio::test]
async fn test_model_service_validation_rejects_empty_endpoint() {
    let mut config = make_test_config();
    config.models.providers[0].endpoint = String::new();
    let result = config.validate();
    assert!(result.is_err(), "空 endpoint 应失败");
}
