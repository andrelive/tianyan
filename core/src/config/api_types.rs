//! 共享的 API 配置类型。
//!
//! 本模块定义 server 和 gui 共用的请求/响应类型。

use crate::common::error::TianyanError;
use serde::{Deserialize, Serialize};

use super::model::{ModelCapability, ModelRef};

/// 模型提供商摘要（供前端列表展示）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderInfo {
    /// 提供商名称。
    pub name: String,
    /// API 端点地址。
    pub endpoint: String,
    /// 是否启用。
    pub enabled: bool,
    /// 模型数量。
    pub model_count: usize,
}

/// 模型条目摘要（供前端列表展示）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    /// 模型名称。
    pub name: String,
    /// 所属提供商。
    pub provider: String,
    /// 能力标签列表。
    pub capabilities: Vec<ModelCapability>,
    /// 该模型支持的思考强度档位值（每个模型自己声明的，如 "low"/"high"/"max"；
    /// None = 不支持思考）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
}

/// 模型服务列表响应。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelsResponse {
    /// 提供商列表。
    pub providers: Vec<ProviderInfo>,
    /// 模型列表。
    pub models: Vec<ModelInfo>,
    /// 各能力偏好。
    pub preferences: PreferencesInfo,
}

/// 偏好信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PreferencesInfo {
    /// 聊天首选模型。
    pub chat: Option<ModelRef>,
    /// 嵌入首选模型。
    pub embedding: Option<ModelRef>,
    /// 视觉首选模型。
    pub vision: Option<ModelRef>,
}

/// 切换默认模型请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchModelRequest {
    /// 模型名称。
    pub model: String,
    /// 能力类型。
    #[serde(default)]
    pub capability: Option<ModelCapability>,
}

impl SwitchModelRequest {
    /// 验证请求参数。
    pub fn validate(&self) -> Result<(), TianyanError> {
        if self.model.trim().is_empty() {
            return Err(TianyanError::Custom(format!(
                "'{}' 的配置值无效：{}",
                "switch_model.model", "模型名不能为空"
            )));
        }
        Ok(())
    }
}

/// 更新配置响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfigResponse {
    /// 是否成功。
    pub success: bool,
    /// 响应消息。
    pub message: String,
}
