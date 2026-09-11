//! 剪贴板 I/O 领域测试。
//!
//! 处理器层测试通过 `create_app` 构建完整 AppState（真实 VFS + tempdir）
//! 后以 `tower::ServiceExt::oneshot` 直击路由（跟随 workspace 领域模式）；
//! 服务层测试直接构造 [`ClipboardService`] 验证沉淀链路。
//! pending/outbox 挂载于 AppState——每个测试独立实例，无跨测试污染，
//! 无需串行化锁。

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tempfile::tempdir;
use tianyan::agent::DynamicToolExecutor;
use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use tianyan::config::{
    ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig, ProviderConfig,
    TianyanConfig,
};
use tianyan::vfs::ContentStore;
use tower::ServiceExt;

use crate::api::clipboard::services::ClipboardService;
use crate::api::clipboard::tool::ClipboardWriteTool;
use crate::api::clipboard::types::{RespondAction, RespondRequest};
use crate::create_app;

/// 构建带 mock 模型提供商的测试配置（与 workspace 领域测试同构；
/// `ModelServices::from_config` 强制要求 chat/embedding/vision 三者齐备）。
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

/// 构造 JSON POST 请求。
fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// 解析响应体为 JSON。
async fn response_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// capture：配置门控
// ---------------------------------------------------------------------------

/// 监听未启用（默认 enabled=false）：204，不做任何存储。
#[tokio::test]
async fn capture_disabled_returns_204() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/clipboard/capture",
            json!({ "text": "hello" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// 监听启用 + 非 auto_capture：返回 pending 确认信息（200）。
#[tokio::test]
async fn capture_enabled_returns_pending() {
    let dir = tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.clipboard.enabled = true;
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/clipboard/capture",
            json!({ "text": "capture-me" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], "pending");
    assert_eq!(body["pending"]["text"], "capture-me");
    assert!(body["pending"]["id"].as_str().is_some());
    assert!(!body["pending"]["captured_at"].as_str().unwrap().is_empty());
}

/// 捕获文本为空：400（监听侧已过滤，防御性校验）。
#[tokio::test]
async fn capture_empty_text_returns_400() {
    let dir = tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.clipboard.enabled = true;
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/clipboard/capture",
            json!({ "text": "   " }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// respond：沉淀链路
// ---------------------------------------------------------------------------

/// remember（显式 text）：写入 `tianyan://memory/clipboard/*`，Detail 全文 + Abstract 摘要。
#[tokio::test]
async fn respond_remember_writes_vfs_memory() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (_app, state) = create_app(config).await.unwrap();
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );

    let resp = service
        .respond(
            &state,
            RespondRequest {
                text: Some("记住这条剪贴板内容".to_string()),
                action: RespondAction::Remember,
            },
        )
        .await
        .unwrap();

    assert_eq!(resp["status"], "stored");
    let uri_str = resp["uri"].as_str().expect("应返回记忆 URI");
    let uri = TianyanUri::parse(uri_str).unwrap();
    assert_eq!(uri.namespace(), ContextNamespace::Memory);
    let path = uri.path();
    assert_eq!(path[0], "clipboard");

    let vfs = state.vfs();
    let detail = vfs
        .read(&uri, ContentLevel::Detail)
        .await
        .expect("Detail 应已写入");
    assert_eq!(detail, "记住这条剪贴板内容");
    let abstract_content = vfs
        .read(&uri, ContentLevel::Abstract)
        .await
        .expect("Abstract 应已写入");
    assert!(!abstract_content.is_empty());
    assert!(abstract_content.contains("记住这条剪贴板内容"));
}

/// ignore（无 pending 时提供显式 text）：清空 pending，不落任何存储。
#[tokio::test]
async fn respond_ignore_clears_pending() {
    let dir = tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.clipboard.enabled = true;
    let (_app, state) = create_app(config).await.unwrap();
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );

    // 先捕获 → pending 存在
    let capture = service.capture(&state, "tmp-ignore-me").await.unwrap();
    assert_eq!(capture.as_ref().unwrap().status, "pending");
    assert!(service.get_pending().await.is_some());

    let resp = service
        .respond(
            &state,
            RespondRequest {
                text: None,
                action: RespondAction::Ignore,
            },
        )
        .await
        .unwrap();
    assert_eq!(resp["status"], "ignored");
    assert!(service.get_pending().await.is_none());
}

/// respond 无 pending 且未提供 text：400。
#[tokio::test]
async fn respond_without_text_or_pending_returns_bad_request() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (_app, state) = create_app(config).await.unwrap();
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );

    let err = service
        .respond(
            &state,
            RespondRequest {
                text: None,
                action: RespondAction::Ignore,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        crate::api::shared::error::ApiError::BadRequest(_)
    ));
}

// ---------------------------------------------------------------------------
// clipboard_write 动态工具
// ---------------------------------------------------------------------------

/// 工具执行后内容进入 outbox（tauri 轮询消费）。
#[tokio::test]
async fn clipboard_write_tool_pushes_outbox() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (_app, state) = create_app(config).await.unwrap();

    let tool = ClipboardWriteTool::new(state.clipboard_outbox());
    let result = tool
        .execute("test-session", r#"{"content":"粘贴这段文本"}"#)
        .await
        .expect("工具执行应成功");
    assert_eq!(result["status"], "queued");
    assert!(result["content_length"].as_u64().unwrap() > 0);

    let items: Vec<String> = state.clipboard_outbox().lock().unwrap().clone();
    assert!(
        items.iter().any(|i| i == "粘贴这段文本"),
        "outbox 应包含工具写入的内容：{items:?}"
    );
}

/// 参数非法：返回工具错误而非 panic。
#[tokio::test]
async fn clipboard_write_tool_rejects_invalid_args() {
    let tool = ClipboardWriteTool::new(Arc::new(Mutex::new(Vec::new())));
    let result = tool.execute("test-session", r#"not-json"#).await;
    assert!(result.is_err(), "非法参数应报错");
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

/// 缺 content 字段：报错。
#[tokio::test]
async fn clipboard_write_tool_requires_content() {
    let tool = ClipboardWriteTool::new(Arc::new(Mutex::new(Vec::new())));
    let result = tool.execute("test-session", r#"{}"#).await;
    assert!(result.is_err());
}

/// 组合根接线：`create_app` 装配的 Agent 工具注册表应包含 `clipboard_write`。
///
/// 验证 state.rs 把 [`ClipboardWriteTool`] 注入 dynamic_tools（new 与
/// reload_agent 双点装配）——否则 tauri 轮询 outbox 永远等不到生产者。
#[tokio::test]
async fn clipboard_write_tool_registered_in_agent() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (_app, state) = create_app(config).await.unwrap();

    let agent = state.agent().await;
    let names: Vec<String> = agent
        .tool_definitions()
        .await
        .into_iter()
        .map(|d| d.function.name)
        .collect();
    assert!(
        names.iter().any(|n| n == "clipboard_write"),
        "Agent 工具注册表应包含 clipboard_write（state.rs 组合根接线）：{names:?}"
    );
}

// ---------------------------------------------------------------------------
// outbox 端点（tauri 轮询路径）
// ---------------------------------------------------------------------------

/// GET /api/v1/clipboard/outbox：取后清空（drain 语义，防重复写出）。
#[tokio::test]
async fn outbox_endpoint_drains_contents() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, state) = create_app(config).await.unwrap();

    // 直接经服务推入两条内容（等价于 agent 工具写入）
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );
    service.outbox_push("第一条".to_string());
    service.outbox_push("第二条".to_string());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/clipboard/outbox")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["contents"].as_array().unwrap().len(), 2);

    // 再次读取应为空（drain 语义）
    let service2 = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );
    assert!(service2.outbox_drain().is_empty());
}
