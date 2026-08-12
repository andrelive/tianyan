//! Webhook 接入视图：`POST /api/v1/events`。
//!
//! 校验配置 `events.webhook_token`（非空时必须匹配 header `X-Tianyan-Token`，
//! 不匹配返回 401；为空则放行——仅本机可访问时安全），通过后把事件发布到
//! 共享事件总线，由事件消费处理器按规则处理。
//!
//! 总线与令牌经 `AppState` 直读（路由为常规 `Router<Arc<AppState>>`）——
//! 不再使用模块级静态：配置热更新即时生效（旧静态在热更新后会过期），
//! 且消除与 AppState 的双份状态。

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use tianyan::events::Event;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// webhook 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookRequest {
    /// webhook 名称（规则 pattern `webhook:<名称>` 匹配）。
    pub name: String,
    /// webhook 载荷（任意 JSON；缺省为空对象）。
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// 接收 webhook 事件。
///
/// 令牌校验通过后发布 `Event::Webhook` 到事件总线，返回 `200 accepted`。
pub async fn receive_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WebhookRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let config = state.config().read().await.clone();
    if !config.events.enabled {
        return Err(ApiError::Internal(
            "事件总线未装配（events.enabled 未开启？）".to_string(),
        ));
    }
    if !validate_token(&config.events.webhook_token, &headers) {
        return Err(ApiError::Unauthorized("无效的 X-Tianyan-Token".to_string()));
    }

    state.event_bus().publish(Event::Webhook {
        name: body.name,
        payload: body.payload,
    });

    Ok(Json(serde_json::json!({ "status": "accepted" })))
}

/// 校验请求令牌：未配置令牌（空）时放行；配置后必须与 `X-Tianyan-Token` 匹配。
fn validate_token(expected_token: &str, headers: &HeaderMap) -> bool {
    if expected_token.is_empty() {
        return true;
    }
    let Some(actual) = headers.get("x-tianyan-token").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    constant_time_eq(expected_token.as_bytes(), actual.as_bytes())
}

/// 常量时间比较（避免令牌校验的时序侧信道）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use tempfile::tempdir;
    use tianyan::config::{
        ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig, ProviderConfig,
        TianyanConfig,
    };

    use crate::create_app;

    /// 构建带 mock 模型提供商的测试配置（`ModelServices::from_config`
    /// 强制要求 chat/embedding/vision 三者齐备；模式同 clipboard/workspace 测试）。
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
                    },
                    ModelEntry {
                        name: "embed".to_string(),
                        capabilities: vec![ModelCapability::TextEmbedding],
                    },
                    ModelEntry {
                        name: "vision".to_string(),
                        capabilities: vec![ModelCapability::Vision],
                    },
                ],
                timeout: 30,
                enabled: true,
                headers: std::collections::HashMap::new(),
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

    /// 构造启用事件驱动的 AppState（返回事件订阅，供断言发布）。
    async fn setup_state(
        token: &str,
        enabled: bool,
    ) -> (
        tempfile::TempDir,
        Arc<AppState>,
        tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) {
        let dir = tempdir().unwrap();
        let mut config = test_config(dir.path());
        config.events.enabled = enabled;
        config.events.webhook_token = token.to_string();
        let (_app, state) = create_app(config).await.unwrap();
        let rx = state.event_bus().subscribe();
        (dir, state, rx)
    }

    /// 直接调用 webhook handler（携带 State）。
    async fn invoke_webhook(
        state: &AppState,
        token: Option<&str>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let mut headers = HeaderMap::new();
        if let Some(t) = token {
            headers.insert("x-tianyan-token", t.parse().expect("令牌应为 ASCII"));
        }
        receive_webhook(
            State(Arc::new(state.clone())),
            headers,
            Json(WebhookRequest {
                name: "deploy".to_string(),
                payload: serde_json::json!({"branch": "main"}),
            }),
        )
        .await
    }

    #[tokio::test]
    async fn test_webhook_accepts_without_token() {
        let (_dir, state, mut rx) = setup_state("", true).await;

        let resp = invoke_webhook(&state, None).await.expect("无令牌应放行");
        assert_eq!(resp.0["status"], "accepted");

        let event = rx.recv().await.expect("应发布 Webhook 事件");
        match event {
            Event::Webhook { name, payload } => {
                assert_eq!(name, "deploy");
                assert_eq!(payload, serde_json::json!({"branch": "main"}));
            }
            other => panic!("应为 Webhook 事件：{other:?}"),
        }
    }

    #[tokio::test]
    async fn test_webhook_token_validation() {
        let (_dir, state, _rx) = setup_state("s3cret", true).await;

        // 无令牌 → 401
        assert!(invoke_webhook(&state, None).await.is_err());
        // 错误令牌 → 401
        assert!(invoke_webhook(&state, Some("wrong")).await.is_err());
        // 正确令牌 → 200
        let resp = invoke_webhook(&state, Some("s3cret"))
            .await
            .expect("正确令牌应放行");
        assert_eq!(resp.0["status"], "accepted");
    }

    #[tokio::test]
    async fn test_webhook_disabled_returns_internal() {
        // events.enabled = false：总线无消费者，拒绝并提示（保持旧静态
        // "未装配"语义——路由常挂，禁用态应显式拒绝而非静默吞掉）
        let (_dir, state, _rx) = setup_state("", false).await;
        let err = invoke_webhook(&state, None)
            .await
            .expect_err("禁用态应拒绝");
        assert!(matches!(err, ApiError::Internal(_)));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }
}
