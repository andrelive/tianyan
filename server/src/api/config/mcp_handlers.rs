//! MCP 服务器管理 HTTP 处理函数。
//!
//! 提供 MCP 服务器的增删改查与连接测试。配置由
//! [`ConfigService`](super::services::ConfigService) 持久化到 tianyan.toml。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use tracing::{error, info};

use tianyan::config::McpServerEntry;

use crate::api::config::services::ConfigService;
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 切换 MCP 服务器启停的请求体。
#[derive(Debug, Deserialize)]
pub struct ToggleMcpServerRequest {
    /// 是否启用。
    pub enabled: bool,
}

/// MCP 服务器连接测试响应。
#[derive(Debug, serde::Serialize)]
pub struct McpTestResponse {
    /// 是否连接成功。
    pub success: bool,
    /// 服务器暴露的工具数量（成功时有效）。
    pub tools: usize,
    /// 失败原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// GET /api/v1/config/mcp/servers — 列出所有 MCP 服务器。
pub async fn list_mcp_servers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<McpServerEntry>>, ApiError> {
    let service = ConfigService::new(state);
    let servers = service.list_mcp_servers().await;
    info!(count = servers.len(), "列出 MCP 服务器");
    Ok(Json(servers))
}

/// POST /api/v1/config/mcp/servers — 添加 MCP 服务器。
pub async fn add_mcp_server(
    State(state): State<Arc<AppState>>,
    Json(server): Json<McpServerEntry>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = ConfigService::new(state);
    service.add_mcp_server(server).await.map(|_| {
        Json(serde_json::json!({
            "success": true,
            "message": "MCP 服务器已添加"
        }))
    })
}

/// DELETE /api/v1/config/mcp/servers/{name} — 移除 MCP 服务器。
pub async fn remove_mcp_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = ConfigService::new(state);
    service.remove_mcp_server(&name).await.map(|_| {
        Json(serde_json::json!({
            "success": true,
            "message": format!("MCP 服务器 '{}' 已移除", name)
        }))
    })
}

/// PUT /api/v1/config/mcp/servers/{name} — 启用/禁用 MCP 服务器。
pub async fn toggle_mcp_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(request): Json<ToggleMcpServerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = ConfigService::new(state);
    service
        .toggle_mcp_server(&name, request.enabled)
        .await
        .map(|_| {
            Json(serde_json::json!({
                "success": true,
                "message": format!("MCP 服务器 '{}' 已{}", name, if request.enabled { "启用" } else { "禁用" })
            }))
        })
}

/// POST /api/v1/config/mcp/servers/{name}/test — 测试 MCP 服务器连接。
///
/// 启动服务器子进程并完成 MCP 握手，统计其暴露的工具数量。
pub async fn test_mcp_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<McpTestResponse>, ApiError> {
    let service = ConfigService::new(state);
    match service.test_mcp_server(&name).await {
        Ok(resp) => {
            info!(server = %name, success = resp.success, "测试 MCP 服务器连接");
            Ok(Json(resp))
        }
        Err(e) => {
            error!("测试 MCP 服务器 '{}' 失败: {}", name, e);
            Err(e)
        }
    }
}
