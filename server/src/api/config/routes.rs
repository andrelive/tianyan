use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::config::handlers::{
    get_config, get_config_section, get_config_status, get_models, switch_model, test_connection,
    update_config,
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
}
