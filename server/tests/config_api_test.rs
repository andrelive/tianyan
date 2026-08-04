//! 配置 API 集成测试 — 驱动真实 `create_app()` 路由测试运行时配置的 HTTP 端点

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use tempfile::tempdir;

use common::factory::test_tianyan_config_with_data_dir;
use common::harness::TestServer;

/// 使用独立临时数据目录构建真实应用路由（VFS + AppState 完整初始化）。
async fn make_router() -> (TestServer, tempfile::TempDir) {
    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());

    let (router, _state) = tianyan_server::create_app(config)
        .await
        .expect("create_app 失败");
    let server = TestServer::start(router).await.expect("启动测试服务器失败");
    (server, dir)
}

#[tokio::test]
async fn test_health_endpoint() {
    let (server, _dir) = make_router().await;

    let resp = server.get("/health").await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string(), "health 应包含版本号: {body}");

    server.shutdown();
}

#[tokio::test]
async fn test_config_endpoint_returns_valid_json() {
    let (server, _dir) = make_router().await;

    let resp = server.get("/api/v1/config").await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("config").is_some(), "响应应包含 config 字段");

    // 真实 handler 返回的 config 应包含可识别的模型结构
    let config = &body["config"];
    assert!(
        config.get("models").is_some() && config["models"].get("providers").is_some(),
        "config 应包含 models.providers: {config}"
    );

    server.shutdown();
}

#[tokio::test]
async fn test_models_endpoint_returns_services() {
    let (server, _dir) = make_router().await;

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
    let (server, _dir) = make_router().await;

    let resp1 = server.get("/api/v1/config").await;
    let body1: serde_json::Value = resp1.json().await.unwrap();

    let resp2 = server.get("/api/v1/config").await;
    let body2: serde_json::Value = resp2.json().await.unwrap();

    assert_eq!(body1, body2, "连续请求应返回一致的配置");

    server.shutdown();
}

#[tokio::test]
async fn test_unknown_api_route_returns_404() {
    let (server, _dir) = make_router().await;

    let resp = server.get("/api/v1/definitely-not-a-route").await;
    assert_eq!(resp.status(), 404);

    server.shutdown();
}
