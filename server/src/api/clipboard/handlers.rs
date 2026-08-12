//! 剪贴板 I/O 处理器。
//!
//! - `POST /clipboard/capture` — tauri 监听循环上报；未启用 204，否则 200
//! - `POST /clipboard/respond` — 前端确认条：remember / knowledge / ignore
//! - `GET  /clipboard/pending` — 读取待确认捕获（前端轮询或 tauri emit 双保险）
//! - `GET  /clipboard/outbox`  — 取出 agent `clipboard_write` 排队内容（tauri 轮询）

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::clipboard::services::ClipboardService;
use crate::api::clipboard::types::{CaptureRequest, RespondRequest};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 捕获上报：未启用监听时 204；auto_capture 直接沉淀；否则存 pending 返回确认信息。
pub async fn capture_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CaptureRequest>,
) -> Result<Response, ApiError> {
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );
    match service.capture(&state, &request.text).await? {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some(resp) => Ok(Json(resp).into_response()),
    }
}

/// 确认沉淀：remember → 记忆；knowledge → 知识库；ignore → 清 pending。
pub async fn respond_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RespondRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );
    Ok(Json(service.respond(&state, request).await?))
}

/// 读取最新待确认捕获（无 pending 时 `pending: null`）。
pub async fn pending_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );
    let pending = service.get_pending().await;
    Ok(Json(serde_json::json!({ "pending": pending })))
}

/// 取出 outbox 全部内容（取后清空；tauri 每 500ms 轮询）。
pub async fn outbox_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = ClipboardService::new(
        state.vfs(),
        state.clipboard_outbox(),
        state.clipboard_pending(),
    );
    let contents = service.outbox_drain();
    Ok(Json(serde_json::json!({ "contents": contents })))
}
