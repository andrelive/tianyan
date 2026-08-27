//! 工具领域测试（G2：此前零测试域补盲）。
//!
//! 处理器层测试通过 `create_app` 构建完整 AppState 后以 `tower::ServiceExt::oneshot`
//! 直击路由（跟随 workspace/clipboard 领域模式）。

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempfile::tempdir;
use tianyan::config::{
    ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig, ProviderConfig,
    TianyanConfig,
};
use tower::ServiceExt;

use crate::create_app;

/// 构建带 mock 模型提供商的测试配置（`ModelServices::from_config` 强制要求
/// chat/embedding/vision 三者齐备）。
fn test_config(data_dir: &Path) -> TianyanConfig {
    let mut config = TianyanConfig::default();
    config.storage.data_dir = data_dir.to_path_buf();
    config.models = ModelsConfig {
        providers: vec![ProviderConfig {
            name: "mock".to_string(),
            endpoint: "http://localhost:11434/v1".to_string(),
            api_key: Some("test-key".to_string()),
            models: vec![
                ModelEntry {
                    name: "test-model".to_string(),
                    capabilities: vec![ModelCapability::Chat],
                    ..Default::default()
                },
                ModelEntry {
                    name: "embed".to_string(),
                    capabilities: vec![ModelCapability::TextEmbedding],
                    ..Default::default()
                },
                ModelEntry {
                    name: "vision".to_string(),
                    capabilities: vec![ModelCapability::Vision],
                    ..Default::default()
                },
            ],
            timeout: 30,
            enabled: true,
            headers: std::collections::HashMap::new(),
        }],
        preferences: ModelPreferences {
            chat: Some(ModelRef {
                provider: "mock".to_string(),
                model: "test-model".to_string(),
            }),
            embedding: Some(ModelRef {
                provider: "mock".to_string(),
                model: "embed".to_string(),
            }),
            vision: Some(ModelRef {
                provider: "mock".to_string(),
                model: "vision".to_string(),
            }),
        },
    };
    config
}

/// GET /api/v1/tools → 200 + 非空工具列表（内置工具注册于 ToolRegistry）。
#[tokio::test]
async fn list_tools_returns_builtin_tools() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/tools")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let tools = body["tools"].as_array().expect("tools 应为数组");
    assert!(!tools.is_empty(), "内置工具列表不应为空");
    assert_eq!(body["total"].as_u64().unwrap() as usize, tools.len());
    // 关键内置工具必须存在（与 LLM 收到的 tools 列表同源）
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in ["read_file", "write_file", "execute_command", "search_vfs"] {
        assert!(names.contains(&expected), "缺少内置工具 {expected}");
    }
}

/// 工具条目携带 name/description/parameters 三要素（前端工具面板契约）。
#[tokio::test]
async fn list_tools_entries_have_contract_fields() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/tools")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    for tool in body["tools"].as_array().unwrap() {
        assert!(tool["name"].is_string(), "name 缺失: {tool}");
        assert!(tool["description"].is_string(), "description 缺失: {tool}");
        assert!(tool["parameters"].is_object(), "parameters 缺失: {tool}");
    }
}
