//! 配置 API 集成测试 — 测试运行时配置的 HTTP 端点

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use axum::{routing::get, Router};
use serde_json::json;

use common::factory::test_tianyan_config;
use common::harness::TestServer;

async fn make_router() -> Router {
    let config = test_tianyan_config();
    let shared = Arc::new(tokio::sync::RwLock::new(config));

    Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({"status": "ok"})) }),
        )
        .route("/api/v1/config", {
            let s = shared.clone();
            get(move || {
                let s = s.clone();
                async move {
                    let config = s.read().await;
                    axum::Json(json!({"config": *config}))
                }
            })
        })
        .route("/api/v1/config/models", {
            let s = shared.clone();
            get(move || {
                let s = s.clone();
                async move {
                    let config = s.read().await;
                    let providers: Vec<serde_json::Value> = config
                        .models
                        .providers
                        .iter()
                        .map(|p| {
                            json!({
                                "name": p.name,
                                "endpoint": p.endpoint,
                                "enabled": p.enabled,
                                "model_count": p.models.len(),
                            })
                        })
                        .collect();
                    let models: Vec<serde_json::Value> = config
                        .models
                        .providers
                        .iter()
                        .flat_map(|p| {
                            p.models.iter().map(|m| {
                                json!({
                                    "name": m.name,
                                    "provider": p.name,
                                    "capabilities": m.capabilities,
                                })
                            })
                        })
                        .collect();
                    axum::Json(json!({
                        "providers": providers,
                        "models": models,
                        "preferences": {
                            "chat": config.models.preferences.chat,
                            "embedding": config.models.preferences.embedding,
                            "vision": config.models.preferences.vision,
                        },
                    }))
                }
            })
        })
}

#[tokio::test]
async fn test_health_endpoint() {
    let router = make_router().await;
    let server = TestServer::start(router).await.unwrap();

    let resp = server.get("/health").await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    server.shutdown();
}

#[tokio::test]
async fn test_config_endpoint_returns_valid_json() {
    let router = make_router().await;
    let server = TestServer::start(router).await.unwrap();

    let resp = server.get("/api/v1/config").await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("config").is_some(), "响应应包含 config 字段");

    server.shutdown();
}

#[tokio::test]
async fn test_models_endpoint_returns_services() {
    let router = make_router().await;
    let server = TestServer::start(router).await.unwrap();

    let resp = server.get("/api/v1/config/models").await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.get("providers").is_some(),
        "models 响应应包含 providers 字段"
    );
    assert!(
        body.get("preferences").is_some(),
        "models 响应应包含 preferences 字段"
    );

    let providers = body["providers"].as_array().unwrap();
    assert!(!providers.is_empty(), "至少应有一项服务");

    let first = &providers[0];
    assert_eq!(first["name"], "mock-service");
    assert_eq!(first["enabled"], true);

    server.shutdown();
}

#[tokio::test]
async fn test_config_endpoint_consistent_across_requests() {
    let router = make_router().await;
    let server = TestServer::start(router).await.unwrap();

    let resp1 = server.get("/api/v1/config").await;
    let body1: serde_json::Value = resp1.json().await.unwrap();

    let resp2 = server.get("/api/v1/config").await;
    let body2: serde_json::Value = resp2.json().await.unwrap();

    assert_eq!(body1, body2, "连续请求应返回一致的配置");

    server.shutdown();
}
