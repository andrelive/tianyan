//! Webhook 接入视图：`POST /api/v1/events`。
//!
//! 校验配置 `events.webhook_token`（非空时必须匹配 header `X-Tianyan-Token`，
//! 不匹配返回 401；为空则放行——仅本机可访问时安全），通过后把事件发布到
//! 共享事件总线，由事件消费处理器按规则处理。

use std::sync::Arc;

use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use tianyan::events::{Event, EventBus};

use crate::api::shared::error::ApiError;

/// webhook 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookRequest {
    /// webhook 名称（规则 pattern `webhook:<名称>` 匹配）。
    pub name: String,
    /// webhook 载荷（任意 JSON；缺省为空对象）。
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// 模块级事件总线（集成者启动时设置；axum 0.8 路由 state 类型限制，
/// 采用与 webhook 令牌一致的模块级共享模式）。
static EVENT_BUS: std::sync::RwLock<Option<Arc<EventBus>>> = std::sync::RwLock::new(None);

/// 设置事件总线（装配时用 `AppState::event_bus()` 调用）。
pub fn set_event_bus(bus: Arc<EventBus>) {
    match EVENT_BUS.write() {
        Ok(mut guard) => *guard = Some(bus),
        Err(poisoned) => *poisoned.into_inner() = Some(bus),
    }
}

/// 读取当前事件总线（None = 未装配，发布失败返回服务错误）。
fn current_event_bus() -> Option<Arc<EventBus>> {
    match EVENT_BUS.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// 模块级 webhook 令牌（配置 `events.webhook_token`；集成者启动时设置）。
///
/// 用可重置的 `RwLock`（而非 `OnceLock`）承载：配置热更新后可重新设置。
static WEBHOOK_TOKEN: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// 设置 webhook 令牌（空字符串 = 不校验，放行所有请求）。
///
/// 集成者在装配时用 `config.events.webhook_token` 调用；配置热更新时重设。
pub fn set_webhook_token(token: impl Into<String>) {
    let token = token.into();
    match WEBHOOK_TOKEN.write() {
        Ok(mut guard) => *guard = Some(token),
        Err(poisoned) => *poisoned.into_inner() = Some(token),
    }
}

/// 读取当前 webhook 令牌（None = 从未设置，等同空令牌放行）。
fn current_webhook_token() -> Option<String> {
    match WEBHOOK_TOKEN.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// 接收 webhook 事件。
///
/// 令牌校验通过后发布 `Event::Webhook` 到事件总线，返回 `200 accepted`。
pub async fn receive_webhook(
    headers: HeaderMap,
    Json(body): Json<WebhookRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !validate_token(&headers) {
        return Err(ApiError::Unauthorized("无效的 X-Tianyan-Token".to_string()));
    }

    let Some(bus) = current_event_bus() else {
        return Err(ApiError::Internal(
            "事件总线未装配（events.enabled 未开启？）".to_string(),
        ));
    };
    bus.publish(Event::Webhook {
        name: body.name,
        payload: body.payload,
    });

    Ok(Json(serde_json::json!({ "status": "accepted" })))
}

/// 校验请求令牌：未配置令牌（空）时放行；配置后必须与 `X-Tianyan-Token` 匹配。
fn validate_token(headers: &HeaderMap) -> bool {
    let Some(expected) = current_webhook_token() else {
        return true;
    };
    if expected.is_empty() {
        return true;
    }
    let Some(actual) = headers.get("x-tianyan-token").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    constant_time_eq(expected.as_bytes(), actual.as_bytes())
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
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt;

    /// 串行化 webhook 测试（模块级令牌/总线为共享状态）。
    static TOKEN_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 直接调用 webhook handler（避免 Router state 装配依赖）。
    async fn invoke_webhook(token: Option<&str>) -> Result<Json<serde_json::Value>, ApiError> {
        let mut headers = HeaderMap::new();
        if let Some(t) = token {
            headers.insert("x-tianyan-token", t.parse().expect("令牌应为 ASCII"));
        }
        receive_webhook(
            headers,
            Json(WebhookRequest {
                name: "deploy".to_string(),
                payload: serde_json::json!({"branch": "main"}),
            }),
        )
        .await
    }

    /// 串行化 webhook 测试（模块级令牌/总线为共享状态）：测试体全程持锁，
    /// 锁不参与 handler 逻辑，跨 await 持有无死锁风险。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_webhook_accepts_without_token() {
        let _guard = TOKEN_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        set_webhook_token("");
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe();
        set_event_bus(bus);

        let resp = invoke_webhook(None).await.expect("无令牌应放行");
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

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_webhook_token_validation() {
        let _guard = TOKEN_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        set_webhook_token("s3cret");
        set_event_bus(Arc::new(EventBus::new()));

        // 无令牌 → 401
        assert!(invoke_webhook(None).await.is_err());
        // 错误令牌 → 401
        assert!(invoke_webhook(Some("wrong")).await.is_err());
        // 正确令牌 → 200
        let resp = invoke_webhook(Some("s3cret"))
            .await
            .expect("正确令牌应放行");
        assert_eq!(resp.0["status"], "accepted");
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }
}
