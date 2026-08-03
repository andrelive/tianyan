use std::sync::Arc;

use axum::{
    routing::{delete, get, post},
    Router,
};

use crate::api::config::handlers::{
    get_config, get_config_section, get_config_status, get_models, switch_model, test_connection,
    update_config,
};
use crate::api::config::mcp_handlers::{
    add_mcp_server, list_mcp_servers, remove_mcp_server, test_mcp_server, toggle_mcp_server,
};
use crate::api::config::ollama_handlers::{
    add_ollama_model, scan_ollama_models, test_ollama_connection,
};
use crate::api::config::soul_handlers::{
    get_default_soul_handler, get_soul_handler, update_soul_handler,
};
use crate::state::AppState;

/// 构建配置路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/config", get(get_config).put(update_config))
        .route("/config/status", get(get_config_status))
        .route("/config/{section}", get(get_config_section))
        .route("/config/models", get(get_models))
        .route("/config/models/switch", post(switch_model))
        .route("/config/test-connection", post(test_connection))
        // Soul（智能体人格）
        .route(
            "/config/soul",
            get(get_soul_handler).put(update_soul_handler),
        )
        .route("/config/soul/default", get(get_default_soul_handler))
        // MCP 服务器管理
        .route(
            "/config/mcp/servers",
            get(list_mcp_servers).post(add_mcp_server),
        )
        .route(
            "/config/mcp/servers/{name}",
            delete(remove_mcp_server).put(toggle_mcp_server),
        )
        .route("/config/mcp/servers/{name}/test", post(test_mcp_server))
        // Ollama 模型发现
        .route("/config/ollama/test", post(test_ollama_connection))
        .route("/config/ollama/scan", post(scan_ollama_models))
        .route("/config/ollama/add-model", post(add_ollama_model))
}
