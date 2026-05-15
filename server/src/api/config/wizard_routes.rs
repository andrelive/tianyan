//! 配置向导路由
//!
//! 提供配置向导的 API 路由定义。

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::config::wizard_handlers::{get_config_status, save_config, test_connection};
use crate::state::AppState;

/// 创建配置向导路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/config/status", get(get_config_status))
        .route("/config/wizard", post(save_config))
        .route("/config/test-connection", post(test_connection))
}
