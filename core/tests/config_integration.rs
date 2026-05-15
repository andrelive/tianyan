//! 配置系统集成测试 — 从文件加载、验证、保存、重新加载的完整流程

use tianyan::config::{ModelServiceConfig, TianyanConfig};

fn make_test_config() -> TianyanConfig {
    let mut config = TianyanConfig::default();
    config.models.services = vec![ModelServiceConfig {
        name: "test".to_string(),
        endpoint: "http://localhost:11434/v1".to_string(),
        api_key: Some("test-key".to_string()),
        default_model: "test-model".to_string(),
        models: vec!["test-model".to_string()],
        service_type: Default::default(),
        timeout: 30,
        max_retries: 2,
        enabled: true,
        priority: 0,
        headers: std::collections::HashMap::new(),
    }];
    config.models.default_chat_model = "test-model".to_string();
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
async fn test_config_validate_rejects_zero_max_iterations() {
    let mut config = make_test_config();
    config.planner.max_iterations = 0;
    let result = config.validate();
    assert!(result.is_err(), "零迭代应失败");
    assert!(
        result.unwrap_err().contains("max_iterations"),
        "错误应提及 max_iterations"
    );
}

#[tokio::test]
async fn test_config_validate_rejects_high_max_retries() {
    let mut config = make_test_config();
    config.models.services[0].max_retries = 100;
    let result = config.validate();
    assert!(result.is_err(), "max_retries > 10 应失败");
    assert!(
        result.unwrap_err().contains("重试次数"),
        "错误应提及重试次数"
    );
}

#[tokio::test]
async fn test_config_defaults_are_reasonable() {
    let config = make_test_config();
    assert_eq!(config.agent.default_top_k, 5);
    assert_eq!(config.planner.max_iterations, 10);
    assert_eq!(config.memory.consolidation_interval, 3600);
    assert_eq!(config.retrieval.default_top_k, 10);
    assert_eq!(config.summary_service.scan_interval_secs, 300);
}

#[tokio::test]
async fn test_model_service_validation_rejects_empty_endpoint() {
    let mut config = make_test_config();
    config.models.services[0].endpoint = String::new();
    let result = config.validate();
    assert!(result.is_err(), "空 endpoint 应失败");
}
