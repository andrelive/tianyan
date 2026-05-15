//! 配置 API 集成测试 — 测试运行时配置的 HTTP 端点

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
                    let services: Vec<serde_json::Value> = config
                        .models
                        .services
                        .iter()
                        .map(|svc| {
                            json!({
                                "name": svc.name,
                                "endpoint": svc.endpoint,
                                "default_model": svc.default_model,
                                "enabled": svc.enabled,
                                "priority": svc.priority,
                            })
                        })
                        .collect();
                    axum::Json(json!({
                        "services": services,
                        "default_chat_model": config.models.default_chat_model,
                        "default_embedding_model": config.models.default_embedding_model,
                        "default_vision_model": config.models.default_vision_model,
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
        body.get("services").is_some(),
        "models 响应应包含 services 字段"
    );
    assert!(
        body.get("default_chat_model").is_some(),
        "models 响应应包含 default_chat_model"
    );

    let services = body["services"].as_array().unwrap();
    assert!(!services.is_empty(), "至少应有一项服务");

    let first = &services[0];
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
