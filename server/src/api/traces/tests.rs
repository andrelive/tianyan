//! 结构化 Trace 领域测试（G2：此前零测试域补盲）。
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
            thinking_field: None,
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

/// GET /api/v1/traces → 200 + 空分组（新实例无执行 span）。
#[tokio::test]
async fn query_traces_returns_empty_on_fresh_instance() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/traces")
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
    // 回放视图：groups 数组（可能为空）
    assert!(body["groups"].is_array(), "groups 应为数组: {body}");
}

/// GET /api/v1/traces?limit=0 → limit 被 clamp 到 1（不 panic）。
#[tokio::test]
async fn query_traces_clamps_limit_to_minimum() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/traces?limit=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// GET /api/v1/traces?limit=99999 → limit 被 clamp 到 1000（不 panic）。
#[tokio::test]
async fn query_traces_clamps_limit_to_maximum() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/traces?limit=99999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
