//! 剪贴板 I/O 领域类型（捕获 / 确认 / pending）。

use serde::{Deserialize, Serialize};

/// 剪贴板捕获请求（tauri 监听循环 POST）。
#[derive(Debug, Clone, Deserialize)]
pub struct CaptureRequest {
    /// 捕获到的剪贴板文本（非空；空文本由监听侧过滤）。
    pub text: String,
}

/// 捕获确认动作。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RespondAction {
    /// 沉淀到长期记忆（`tianyan://memory/clipboard/`）。
    Remember,
    /// 沉淀到知识库（`tianyan://knowledge/`）。
    Knowledge,
    /// 丢弃：清空 pending，不落任何存储。
    Ignore,
}

/// 捕获确认请求（前端确认条 POST）。
#[derive(Debug, Clone, Deserialize)]
pub struct RespondRequest {
    /// 覆盖 pending 的文本（省略时使用 pending 原文；无 pending 且未提供时 400）。
    pub text: Option<String>,
    /// 确认动作。
    pub action: RespondAction,
}

/// 待用户确认的捕获（等待前端确认条响应）。
#[derive(Debug, Clone, Serialize)]
pub struct PendingCapture {
    /// 捕获唯一标识。
    pub id: String,
    /// 剪贴板文本。
    pub text: String,
    /// 捕获时间（ISO 8601）。
    pub captured_at: String,
}

/// 捕获响应（`POST /api/v1/clipboard/capture`）。
#[derive(Debug, Clone, Serialize)]
pub struct CaptureResponse {
    /// 处理状态：`pending`（等待确认）或 `stored`（已自动沉淀）。
    pub status: String,
    /// `status=pending` 时的待确认信息。
    pub pending: Option<PendingCapture>,
    /// `status=stored` 时的沉淀目标 URI（如 `tianyan://memory/clipboard/<ts>`）。
    pub uri: Option<String>,
}
