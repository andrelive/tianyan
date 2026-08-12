//! Soul（智能体核心人格）管理。

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tianyan::agent::DEFAULT_SOUL;
use tianyan::common::types::{ContentLevel, TianyanUri};
use tianyan::vfs::ContentStore;

use crate::api::ApiError;
use crate::state::AppState;

/// 构造无效 soul URI 错误（统一错误前缀）。
fn invalid_soul_uri_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("无效的 soul URI: {e}"))
}

/// 更新 soul 内容的请求体。
#[derive(Debug, Deserialize)]
pub struct SoulUpdateRequest {
    /// soul 全文内容。
    pub content: String,
}

/// soul 内容响应。
#[derive(Debug, Serialize)]
pub struct SoulResponse {
    /// soul 全文内容。
    pub content: String,
}

/// GET /api/v1/config/soul
pub async fn get_soul_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SoulResponse>, ApiError> {
    let uri = TianyanUri::parse("tianyan://agent/soul").map_err(invalid_soul_uri_error)?;

    let content = state.vfs().read(&uri, ContentLevel::Detail).await?;

    Ok(Json(SoulResponse { content }))
}

/// PUT /api/v1/config/soul
pub async fn update_soul_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SoulUpdateRequest>,
) -> Result<Json<Value>, ApiError> {
    if body.content.trim().is_empty() {
        return Err(ApiError::BadRequest("内容不能为空".into()));
    }

    let uri = TianyanUri::parse("tianyan://agent/soul").map_err(invalid_soul_uri_error)?;

    state
        .vfs()
        .write(&uri, ContentLevel::Detail, &body.content)
        .await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "智能体人格已更新"
    })))
}

/// GET /api/v1/config/soul/default
pub async fn get_default_soul_handler() -> Result<Json<SoulResponse>, ApiError> {
    Ok(Json(SoulResponse {
        content: DEFAULT_SOUL.to_string(),
    }))
}
