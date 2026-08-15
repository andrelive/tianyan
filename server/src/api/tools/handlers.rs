use std::sync::Arc;

use axum::{extract::State, Json};
use tracing::info;

use crate::api::shared::error::ApiError;
use crate::api::tools::types::{ListToolsResponse, ToolInfo};
use crate::state::AppState;

/// 列出全部系统工具（内置 + 动态注册，如 MCP 桥接）。
///
/// 工具目录展示：与 LLM 收到的 tools 列表同源（AgentCoordinator::tool_definitions）。
pub async fn list_tools(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListToolsResponse>, ApiError> {
    info!("列出全部工具");

    let agent = state.agent().await;
    let definitions = agent.tool_definitions().await;

    let tools: Vec<ToolInfo> = definitions
        .into_iter()
        .map(|d| ToolInfo {
            name: d.function.name,
            description: d.function.description,
            parameters: d.function.parameters,
        })
        .collect();
    let total = tools.len();

    Ok(Json(ListToolsResponse { tools, total }))
}
