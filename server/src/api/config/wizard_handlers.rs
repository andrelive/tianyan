//! 配置向导处理器
//!
//! 提供配置向导的 HTTP 请求处理器。

use std::sync::Arc;

use axum::{extract::State, Json};
use tracing::info;

use crate::api::config::wizard_services::ConfigWizardService;
use crate::api::config::wizard_types::SaveConfigRequest;
use crate::state::AppState;

/// 获取配置状态处理器
pub async fn get_config_status() -> Json<tianyan::config::ConfigStatus> {
    let status = ConfigWizardService::get_config_status();
    Json(status)
}

/// 保存配置处理器
pub async fn save_config(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SaveConfigRequest>,
) -> Json<crate::api::config::wizard_types::SaveConfigResponse> {
    info!("保存配置向导数据");

    let service = ConfigWizardService::new();
    let response = service.save_config(state, request).await;

    Json(response)
}

/// 测试模型连接处理器
pub async fn test_connection(
    Json(request): Json<tianyan::config::TestConnectionRequest>,
) -> Json<tianyan::config::TestConnectionResponse> {
    info!("测试模型连接: {}", request.endpoint);

    let service = ConfigWizardService::new();
    let response = service.test_connection(request).await;

    Json(response)
}
