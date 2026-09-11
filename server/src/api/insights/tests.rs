//! 内部状态视图领域测试（G2：此前零测试域补盲）。
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

/// GET /api/v1/memory → 200 + 空记忆列表（新实例无提取记忆）。
#[tokio::test]
async fn list_memories_returns_empty_on_fresh_instance() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/memory")
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
    assert_eq!(body["total"].as_u64().unwrap(), 0);
    assert!(body["memories"].as_array().unwrap().is_empty());
}

/// GET /api/v1/memory → 递归展开子目录：仅返回叶子条目 + relative_path。
#[tokio::test]
async fn list_memories_expands_subdirectories_to_leaves() {
    use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
    use tianyan::vfs::ContentStore;

    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, state) = create_app(config).await.unwrap();

    let vfs = state.vfs();
    // 嵌套写入：浅层 facts/ 与深层 cases/failed_tasks/（父目录由 VFS 自动创建）
    let shallow = TianyanUri::new(
        ContextNamespace::Memory,
        vec!["facts".to_string(), "older-fact".to_string()],
    );
    vfs.write(&shallow, ContentLevel::Detail, "旧事实")
        .await
        .unwrap();
    let deep = TianyanUri::new(
        ContextNamespace::Memory,
        vec![
            "cases".to_string(),
            "failed_tasks".to_string(),
            "newer-case".to_string(),
        ],
    );
    vfs.write(&deep, ContentLevel::Detail, "新案例")
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/memory")
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

    let memories = body["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 2, "目录不返回，仅叶子条目: {memories:?}");
    assert!(memories.iter().all(|m| m["is_directory"] == false));

    let paths: Vec<&str> = memories
        .iter()
        .map(|m| m["relative_path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"facts/older-fact"), "浅层叶子: {paths:?}");
    assert!(
        paths.contains(&"cases/failed_tasks/newer-case"),
        "深层叶子: {paths:?}"
    );
}

/// GET /api/v1/stats → 200 + 统计摘要结构（技能调用/文档访问/搜索热度）。
#[tokio::test]
async fn get_stats_returns_summary_shape() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/stats")
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
    assert!(body.is_object(), "stats 应为对象: {body}");
}

/// GET /api/v1/scheduler/status → 200 + 未装配调度器返回空状态（running=false）。
#[tokio::test]
async fn scheduler_status_returns_not_running_without_scheduler() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scheduler/status")
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
    assert!(!body["running"].as_bool().unwrap());
    assert!(body["tasks"].as_array().unwrap().is_empty());
}

/// GET /api/v1/usage/stats → 200 + 用量统计结构（空日志返回零值）。
#[tokio::test]
async fn usage_stats_returns_shape_on_empty_log() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/usage/stats")
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
    assert!(body.is_object(), "usage/stats 应为对象: {body}");
}

/// GET /api/v1/approval/status → 200 + 审批快照结构。
#[tokio::test]
async fn approval_status_returns_snapshot() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/approval/status")
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
    assert!(body.is_object(), "approval/status 应为对象: {body}");
}
