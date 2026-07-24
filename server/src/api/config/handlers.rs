use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::Value;
use tracing::{error, info, warn};

use crate::api::config::services::ConfigService;
use crate::api::config::types::{
    ConfigResponse, ModelsResponse, SwitchModelRequest, UpdateConfigRequest, UpdateConfigResponse,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 获取配置处理器
pub async fn get_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConfigResponse>, ApiError> {
    info!("获取运行时配置");

    let service = ConfigService::new(state);

    service.get_config().await.map(Json).map_err(|e| {
        error!("获取配置失败: {}", e);
        ApiError::Internal(format!("获取配置失败: {}", e))
    })
}

/// 更新配置处理器
pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<UpdateConfigResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!("更新运行时配置");

    let service = ConfigService::new(state);

    service
        .update_config(request.config)
        .await
        .map(Json)
        .map_err(|e| {
            error!("更新配置失败: {}", e);
            ApiError::Internal(format!("更新配置失败: {}", e))
        })
}

/// 获取配置的特定节
pub async fn get_config_section(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(section): axum::extract::Path<String>,
) -> Result<Json<Value>, ApiError> {
    if section.trim().is_empty() {
        return Err(ApiError::BadRequest("配置节名称不能为空".to_string()));
    }

    info!("获取配置节: {}", section);

    let config = state.config().read().await.clone();
    let value = match section.as_str() {
        "agent" => serde_json::to_value(&config.agent).unwrap_or(Value::Null),
        "models" => serde_json::to_value(&config.models).unwrap_or(Value::Null),
        "storage" => serde_json::to_value(&config.storage).unwrap_or(Value::Null),
        "logging" => serde_json::to_value(&config.logging).unwrap_or(Value::Null),
        "security" => serde_json::to_value(&config.security).unwrap_or(Value::Null),
        "memory" => serde_json::to_value(&config.memory).unwrap_or(Value::Null),
        "retrieval" => serde_json::to_value(&config.retrieval).unwrap_or(Value::Null),
        _ => {
            warn!("未知配置节: {}", section);
            Value::Null
        }
    };

    Ok(Json(value))
}

/// 获取模型服务列表及默认模型
pub async fn get_models(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ModelsResponse>, ApiError> {
    info!("获取模型服务列表");

    let service = ConfigService::new(state);

    service.get_models().await.map(Json).map_err(|e| {
        error!("获取模型列表失败: {}", e);
        ApiError::Internal(format!("获取模型列表失败: {}", e))
    })
}

/// GET /api/config/status — simple bootstrap check for frontend.
pub async fn get_config_status() -> Json<Value> {
    let configured = tianyan::config::TianyanConfig::config_exists();
    Json(serde_json::json!({ "configured": configured }))
}

/// POST /api/config/test-connection — test a model service connection.
pub async fn test_connection(
    Json(request): Json<tianyan::config::TestConnectionRequest>,
) -> Json<tianyan::config::TestConnectionResponse> {
    use tianyan::config::TestConnectionResponse;
    if !request.endpoint.starts_with("http://") && !request.endpoint.starts_with("https://") {
        return Json(TestConnectionResponse::error(
            "API 端点 URL 格式无效，必须以 http:// 或 https:// 开头",
        ));
    }
    if request.api_key.is_empty() {
        return Json(TestConnectionResponse::error("API 密钥不能为空"));
    }
    if request.model.is_empty() {
        return Json(TestConnectionResponse::error("模型名称不能为空"));
    }
    // Delegate to core_bridge for actual connection test
    match crate::core_bridge::test_model_connection(
        &request.endpoint,
        &request.api_key,
        &request.model,
    )
    .await
    {
        Ok(models) => Json(TestConnectionResponse::success("连接成功", models)),
        Err(e) => Json(TestConnectionResponse::error(format!("连接失败: {}", e))),
    }
}

/// 切换默认聊天模型
pub async fn switch_model(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SwitchModelRequest>,
) -> Result<Json<UpdateConfigResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!(model = %request.model, "切换默认聊天模型");

    let service = ConfigService::new(state);

    service
        .switch_model(&request.model)
        .await
        .map(Json)
        .map_err(|e| {
            error!("切换模型失败: {}", e);
            ApiError::Internal(format!("切换模型失败: {}", e))
        })
}
