use serde::{Deserialize, Serialize};

use crate::api::{self, ApiResult};

/// 模型服务摘要
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelServiceInfo {
    pub name: String,
    pub endpoint: String,
    pub default_model: String,
    pub enabled: bool,
    pub priority: u32,
}

/// 模型服务列表响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelsResponse {
    pub services: Vec<ModelServiceInfo>,
    pub default_chat_model: String,
    pub default_embedding_model: String,
    pub default_vision_model: String,
}

/// 切换默认聊天模型请求
#[derive(Debug, Serialize)]
pub struct SwitchModelRequest {
    pub model: String,
}

/// 更新配置响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfigResponse {
    pub success: bool,
    pub message: String,
}

/// 获取模型服务列表及默认模型
pub async fn fetch_models() -> ApiResult<ModelsResponse> {
    api::get("/config/models").await
}

/// 切换默认聊天模型
pub async fn switch_chat_model(model: &str) -> ApiResult<UpdateConfigResponse> {
    let request = SwitchModelRequest {
        model: model.to_string(),
    };
    api::post("/config/models/switch", &request).await
}
