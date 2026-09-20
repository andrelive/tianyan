//! 后台任务领域测试（G2：此前零测试域补盲）。
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

use crate::api::tasks::handlers::build_snapshot_payload;
use crate::create_app;
use tianyan::common::types::{MessageRole, Part, PartTime, StructuredMessage};

/// 构造一条 `role=tool` 的工具结果消息（跨消息合并用例复用）。
fn tool_result_message(
    id: &str,
    call_id: &str,
    content: &str,
    error: Option<&str>,
) -> StructuredMessage {
    StructuredMessage {
        id: id.to_string(),
        parent_id: None,
        role: MessageRole::Tool,
        parts: vec![Part::ToolResult {
            tool_call_id: call_id.to_string(),
            content: content.to_string(),
            error: error.map(|e| e.to_string()),
            time: PartTime {
                start: 1_000,
                end: 1_120,
            },
        }],
        tokens: Default::default(),
        cost: 0.0,
        model_id: None,
        time: Default::default(),
        session_id: "s1".to_string(),
        finish: None,
        compression_marker: false,
    }
}

/// 构造一条携带工具调用的 assistant 消息。
fn assistant_with_tool_call(call_id: &str, name: &str) -> StructuredMessage {
    let mut assistant = StructuredMessage::assistant("s1", "");
    assistant.parts = vec![
        Part::Text {
            text: "好，我看看".to_string(),
            time: PartTime::default(),
        },
        Part::ToolCall {
            id: call_id.to_string(),
            name: name.to_string(),
            arguments: r#"{"path":"a.txt"}"#.to_string(),
            time: PartTime::default(),
        },
    ];
    assistant
}

/// T0-7（主回归）：订阅快照必须携带工具卡片执行结果。
///
/// 判别力：修复前快照走 `from_structured_light`（`tool_calls: None`），
/// 工具卡片在前端丢 result/duration/error → 渲染成永久"运行中"转圈。
#[test]
fn snapshot_payload_keeps_tool_call_results() {
    let payload = build_snapshot_payload(
        "s1",
        vec![
            (0, StructuredMessage::user("s1", "读一下 a.txt")),
            (1, assistant_with_tool_call("call_1", "read_file")),
            (
                2,
                tool_result_message("msg_tool_1", "call_1", "文件内容", None),
            ),
        ],
        7,
        false,
    );

    assert_eq!(payload["type"], "snapshot");
    assert_eq!(payload["cursor"], 7);
    let messages = payload["messages"].as_array().expect("messages 必须是数组");
    assert_eq!(
        messages.len(),
        2,
        "工具结果已并入卡片 → 原 role=tool 消息不再单独出现（避免空气泡）"
    );

    let calls = messages[1]["tool_calls"]
        .as_array()
        .expect("快照必须携带 tool_calls——否则前端历史工具卡片永久显示\"运行中\"（T0-7）");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "read_file");
    assert_eq!(calls[0]["result"], "文件内容", "结果必须随快照下发");
    assert_eq!(
        calls[0]["duration_ms"], 120,
        "耗时由 Part::ToolResult.time 差值推导"
    );
}

/// T0-7：失败原因随快照下发（前端卡片显示 ✗ 而非转圈）。
#[test]
fn snapshot_payload_surfaces_tool_error() {
    let payload = build_snapshot_payload(
        "s1",
        vec![
            (0, assistant_with_tool_call("call_1", "run_command")),
            (
                1,
                tool_result_message("msg_tool_1", "call_1", "", Some("命令失败：exit 1")),
            ),
        ],
        2,
        false,
    );

    let calls = payload["messages"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls[0]["error"], "命令失败：exit 1");
}

/// T0-7（附带修复）：快照的 Tool 角色映射为 Assistant（与历史加载同源）。
/// 旧快照路径沿用 core role → `role=tool` 直落前端（无对应渲染分支）；
/// 孤立结果（无对应调用）保留为结果-only 卡片。
#[test]
fn snapshot_payload_maps_orphan_tool_result_to_assistant() {
    let payload = build_snapshot_payload(
        "s1",
        vec![(
            0,
            tool_result_message("msg_tool_orphan", "call_missing", "孤立结果", None),
        )],
        3,
        false,
    );

    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        1,
        "孤立结果消息必须保留（否则结果彻底丢失）"
    );
    assert_eq!(
        messages[0]["role"], "assistant",
        "Tool 角色必须映射为 Assistant，与历史加载一致"
    );
    let calls = messages[0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls[0]["result"], "孤立结果");
    assert_eq!(calls[0]["presentation"], "generic");
}

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
            dialect: None,
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

/// GET /api/v1/tasks → 200 + 空任务列表（新实例无后台任务）。
#[tokio::test]
async fn list_tasks_returns_empty_on_fresh_instance() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/tasks")
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
    assert_eq!(body.as_array().unwrap().len(), 0, "新实例不应有后台任务");
}

/// POST /api/v1/tasks/{id}/cancel → 任务不存在返回 404（语义谓词保真）。
#[tokio::test]
async fn cancel_unknown_task_returns_404() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path());
    let (app, _state) = create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tasks/bt_no-such-task/cancel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
