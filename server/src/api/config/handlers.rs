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

    service
        .get_config()
        .await
        .inspect_err(|e| error!("获取配置失败: {}", e))
        .map(Json)
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
        .inspect_err(|e| error!("更新配置失败: {}", e))
        .map(Json)
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
        "agent_roles" => serde_json::to_value(&config.agent_roles).unwrap_or(Value::Null),
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

    service
        .get_models()
        .await
        .inspect_err(|e| error!("获取模型列表失败: {}", e))
        .map(Json)
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
    match test_model_connection(&request.endpoint, &request.api_key, &request.model).await {
        Ok(models) => Json(TestConnectionResponse::success("连接成功", models)),
        Err(e) => Json(TestConnectionResponse::error(format!("连接失败: {}", e))),
    }
}

/// 测试模型连接：创建临时客户端并发送一个简单的聊天请求验证连接。
///
/// 错误文本按 HTTP 状态码文本分类（401/404/timeout），
/// 面向配置面板的友好提示；不属 TianyanError 语义谓词范畴。
async fn test_model_connection(
    endpoint: &str,
    api_key: &str,
    model: &str,
) -> Result<Vec<String>, String> {
    let client = tianyan::model::AsyncOpenAIClient::new("test-connection", endpoint, api_key, 30)
        .map_err(|e| format!("创建客户端失败: {}", e))?;

    let request = tianyan::model::types::ChatCompletionRequest::new(
        model.to_string(),
        vec![tianyan::Message::system("You are a helpful assistant.")],
    )
    .with_temperature(0.7)
    .with_max_tokens(10)
    .with_stream(false)
    .with_enable_thinking(false);

    use tianyan::model::ChatService;
    match client.chat_completion(request).await {
        Ok(_) => Ok(vec![model.to_string()]),
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("401") || error_msg.contains("unauthorized") {
                Err("API 密钥无效或已过期".to_string())
            } else if error_msg.contains("404") {
                Err("模型不存在，请检查模型名称".to_string())
            } else if error_msg.contains("timeout") {
                Err("连接超时，请检查网络或 API 端点".to_string())
            } else {
                Err(format!("连接失败: {}", error_msg))
            }
        }
    }
}

/// 切换默认聊天模型
pub async fn switch_model(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SwitchModelRequest>,
) -> Result<Json<UpdateConfigResponse>, ApiError> {
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e.to_string()));
    }

    info!(model = %request.model, "切换默认聊天模型");

    let service = ConfigService::new(state);

    service
        .switch_model(&request.model)
        .await
        .inspect_err(|e| error!("切换模型失败: {}", e))
        .map(Json)
}
