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
