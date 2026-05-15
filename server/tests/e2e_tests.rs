//! 端到端系统测试 — 完整启动 → API 调用 → 验证流程

mod common;

use std::sync::Arc;

use axum::{routing::get, Router};
use serde_json::json;

use common::factory::test_tianyan_config;
use common::harness::TestServer;

/// 构建完整的测试路由（模拟 server/src/api 的完整路由结构）
async fn full_router() -> Router {
    let config = Arc::new(tokio::sync::RwLock::new(test_tianyan_config()));

    Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({"status": "ok"})) }),
        )
        .route("/api/v1/config", {
            let s = config.clone();
            get(move || {
                let s = s.clone();
                async move {
                    let c = s.read().await;
                    axum::Json(json!({"config": *c}))
                }
            })
        })
        .route("/api/v1/config/models", {
            let s = config.clone();
            get(move || {
                let s = s.clone();
                async move {
                    let c = s.read().await;
                    let services: Vec<serde_json::Value> = c
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
                        "default_chat_model": c.models.default_chat_model,
                        "default_embedding_model": c.models.default_embedding_model,
                        "default_vision_model": c.models.default_vision_model,
                    }))
                }
            })
        })
}

#[tokio::test]
async fn test_full_startup_health_models_flow() {
    let router = full_router().await;
    let server = TestServer::start(router).await.unwrap();

    let health_resp = server.get("/health").await;
    assert_eq!(health_resp.status(), 200);

    let config_resp = server.get("/api/v1/config").await;
    assert_eq!(config_resp.status(), 200);

    let models_resp = server.get("/api/v1/config/models").await;
    assert_eq!(models_resp.status(), 200);
    let body: serde_json::Value = models_resp.json().await.unwrap();

    assert_eq!(body["default_chat_model"], "test-model");
    let services = body["services"].as_array().unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0]["name"], "mock-service");
    assert_eq!(services[0]["enabled"], true);

    server.shutdown();
}

#[tokio::test]
async fn test_concurrent_requests_handle_correctly() {
    let router = full_router().await;
    let server = TestServer::start(router).await.unwrap();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let url = format!("{}/health", server.base_url());
        handles.push(tokio::spawn(async move {
            reqwest::get(&url).await.unwrap().status()
        }));
    }

    for handle in handles {
        assert_eq!(handle.await.unwrap(), 200);
    }

    server.shutdown();
}

#[tokio::test]
async fn test_404_on_unknown_path() {
    let router = full_router().await;
    let server = TestServer::start(router).await.unwrap();

    let resp = server.get("/api/v1/nonexistent").await;
    assert_eq!(resp.status(), 404);

    server.shutdown();
}
