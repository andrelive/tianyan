// 标准库
use std::sync::Arc;

use tracing::info;

use tianyan::config::TianyanConfig;

use crate::api::config::types::{ConfigResponse, ModelsResponse, UpdateConfigResponse};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 配置服务，管理应用配置的读取、保存与热重载
pub struct ConfigService {
    state: Arc<AppState>,
}

impl ConfigService {
    /// 创建新的配置服务
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// 获取当前配置
    pub async fn get_config(&self) -> Result<ConfigResponse, ApiError> {
        let config = self.state.config().read().await.clone();
        Ok(ConfigResponse { config })
    }

    /// 更新配置 — 验证、保存到文件、热重载 Agent
    pub async fn update_config(
        &self,
        config: TianyanConfig,
    ) -> Result<UpdateConfigResponse, ApiError> {
        config.validate().map_err(ApiError::Config)?;

        self.persist_config(&config).await?;

        self.state
            .update_config(config)
            .await
            .map_err(|e| ApiError::Internal(format!("热重载失败: {}", e)))?;

        info!("配置已更新并重载");
        Ok(UpdateConfigResponse {
            success: true,
            message: "配置已更新".to_string(),
        })
    }

    /// 获取模型服务列表
    pub async fn get_models(&self) -> Result<ModelsResponse, ApiError> {
        use tianyan::config::api_types::{ModelInfo, PreferencesInfo, ProviderInfo};

        let config = self.state.config().read().await.clone();

        let providers = config
            .models
            .providers
            .iter()
            .map(|p| ProviderInfo {
                name: p.name.clone(),
                endpoint: p.endpoint.clone(),
                enabled: p.enabled,
                model_count: p.models.len(),
            })
            .collect();

        let models = config
            .models
            .providers
            .iter()
            .flat_map(|p| {
                p.models.iter().map(|m| ModelInfo {
                    name: m.name.clone(),
                    provider: p.name.clone(),
                    capabilities: m.capabilities.clone(),
                })
            })
            .collect();

        let preferences = PreferencesInfo {
            chat: config.models.preferences.chat.clone(),
            embedding: config.models.preferences.embedding.clone(),
            vision: config.models.preferences.vision.clone(),
        };

        Ok(ModelsResponse {
            providers,
            models,
            preferences,
        })
    }

    /// 切换默认聊天模型
    pub async fn switch_model(&self, model: &str) -> Result<UpdateConfigResponse, ApiError> {
        let mut config = self.state.config().read().await.clone();

        // 查找哪个 provider 拥有此模型
        let model_ref = config
            .models
            .providers
            .iter()
            .filter(|p| p.enabled)
            .find_map(|p| {
                p.models
                    .iter()
                    .find(|m| m.name == model)
                    .map(|_m| tianyan::config::ModelRef {
                        provider: p.name.clone(),
                        model: model.to_string(),
                    })
            })
            .ok_or_else(|| {
                ApiError::BadRequest(format!("模型 '{}' 未在任何已启用的提供商中注册", model))
            })?;

        config.models.preferences.chat = Some(model_ref);

        config.validate().map_err(ApiError::Config)?;

        self.persist_config(&config).await?;

        self.state
            .update_config(config)
            .await
            .map_err(|e| ApiError::Internal(format!("热重载失败: {}", e)))?;

        info!(model = model, "默认聊天模型已切换");
        Ok(UpdateConfigResponse {
            success: true,
            message: format!("已切换到模型：{}", model),
        })
    }
    async fn persist_config(&self, config: &TianyanConfig) -> Result<(), ApiError> {
        let path = TianyanConfig::find_config_file()
            .or_else(TianyanConfig::default_config_path)
            .ok_or_else(|| ApiError::Internal("无法确定配置文件路径".to_string()))?;

        config
            .save_to_file(&path)
            .map_err(|e| ApiError::Internal(format!("保存配置文件失败: {}", e)))?;

        info!(path = %path.display(), "配置已保存到: {}", path.display());
        Ok(())
    }
}
