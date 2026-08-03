//! 端到端系统测试 — 完整启动 → API 调用 → 验证流程

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
                    let providers: Vec<serde_json::Value> = c
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
                    let models: Vec<serde_json::Value> = c
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
                            "chat": c.models.preferences.chat,
                            "embedding": c.models.preferences.embedding,
                            "vision": c.models.preferences.vision,
                        },
                    }))
                }
            })
        })
}

#[tokio::test]
async fn test_e2e_full_router_health_and_config() {
    let router = full_router().await;
    let server = TestServer::start(router).await.unwrap();

    // 健康检查
    let resp = server.get("/health").await;
    assert_eq!(resp.status(), 200);

    // 配置端点返回有效 JSON
    let resp = server.get("/api/v1/config").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("config").is_some(), "响应应包含 config 字段");

    server.shutdown();
}
