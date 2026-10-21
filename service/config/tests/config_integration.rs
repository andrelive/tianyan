//! 配置系统集成测试 — 从文件加载、验证、保存、重新加载的完整流程
//!
//! **随迁自 `core/tests/config_integration.rs`**（ADR-049 期 3-T4）：这三个用例走的是
//! 加载器 / 持久化 / 校验的**端到端**路径（`save_to_file` → 磁盘 → `load_from_file`），
//! 实现搬进本 crate 后继续由本 crate 做集成测试才有判别力。断言语义逐字未改，只把
//! `TianyanConfig::{save_to_file, load_from_file, validate}` 换成自由函数调用。

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tianyan::config::{ModelCapability, ModelEntry, ModelRef, ProviderConfig, TianyanConfig};
use tianyan_service_config::{load_from_file, save_to_file, validate};

fn make_test_config() -> TianyanConfig {
    let mut config = TianyanConfig::default();
    config.models.providers = vec![ProviderConfig {
        name: "test".to_string(),
        endpoint: "http://localhost:11434/v1".to_string(),
        api_key: Some("test-key".to_string()),
        models: vec![ModelEntry {
            name: "test-model".to_string(),
            capabilities: vec![ModelCapability::Chat],
            ..Default::default()
        }],
        timeout: 30,
        first_token_timeout: 300,
        enabled: true,
        headers: std::collections::HashMap::new(),
        thinking_field: None,
        dialect: None,
        ..Default::default()
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
    save_to_file(&original, &config_path).unwrap();
    assert!(config_path.exists());

    let reloaded = load_from_file(&config_path).unwrap();
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
    assert!(config.memory.auto_consolidation, "默认开启自动巩固");
    assert_eq!(config.retrieval.default_top_k, 10);
}

#[tokio::test]
async fn test_model_service_validation_rejects_empty_endpoint() {
    let mut config = make_test_config();
    config.models.providers[0].endpoint = String::new();
    let result = validate(&config);
    assert!(result.is_err(), "空 endpoint 应失败");
}
