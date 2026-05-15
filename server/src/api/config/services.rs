// 标准库
use std::sync::Arc;

use tracing::{debug, info};

use tianyan::config::TianyanConfig;

use crate::api::config::types::{
    ConfigResponse, ModelServiceInfo, ModelsResponse, UpdateConfigResponse,
};
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
    pub async fn get_config(&self) -> anyhow::Result<ConfigResponse> {
        let config = self.state.config().read().await.clone();
        Ok(ConfigResponse { config })
    }

    /// 更新配置 — 验证、保存到文件、热重载 Agent
    pub async fn update_config(
        &self,
        config: TianyanConfig,
    ) -> anyhow::Result<UpdateConfigResponse> {
        config.validate().map_err(|e| anyhow::anyhow!("{}", e))?;

        self.persist_config(&config).await?;

        self.state
            .update_config(config)
            .await
            .map_err(|e| anyhow::anyhow!("热重载失败: {}", e))?;

        info!("配置已更新并重载");
        Ok(UpdateConfigResponse {
            success: true,
            message: "配置已更新".to_string(),
        })
    }

    /// 获取模型服务列表及默认模型
    pub async fn get_models(&self) -> anyhow::Result<ModelsResponse> {
        let config = self.state.config().read().await.clone();

        let services: Vec<ModelServiceInfo> = config
            .models
            .services
            .iter()
            .map(|s| ModelServiceInfo {
                name: s.name.clone(),
                endpoint: s.endpoint.clone(),
                default_model: s.default_model.clone(),
                enabled: s.enabled,
                priority: s.priority,
            })
            .collect();

        Ok(ModelsResponse {
            services,
            default_chat_model: config.models.default_chat_model.clone(),
            default_embedding_model: config.models.default_embedding_model.clone(),
            default_vision_model: config.models.default_vision_model.clone(),
        })
    }

    /// 切换默认聊天模型
    pub async fn switch_model(&self, model: &str) -> anyhow::Result<UpdateConfigResponse> {
        let mut config = self.state.config().read().await.clone();

        let valid_models: Vec<String> = config
            .models
            .services
            .iter()
            .filter(|s| s.enabled)
            .flat_map(|s| s.models.clone())
            .collect();

        if !valid_models.contains(&model.to_string()) {
            return Err(anyhow::anyhow!(
                "模型 '{}' 未在任何已启用的服务中注册",
                model
            ));
        }

        config.models.default_chat_model = model.to_string();
        debug!(model = model, "切换默认聊天模型");

        config.validate().map_err(|e| anyhow::anyhow!("{}", e))?;

        self.persist_config(&config).await?;

        self.state
            .update_config(config)
            .await
            .map_err(|e| anyhow::anyhow!("热重载失败: {}", e))?;

        info!(model = model, "默认聊天模型已切换");
        Ok(UpdateConfigResponse {
            success: true,
            message: format!("已切换到模型：{}", model),
        })
    }

    /// 将配置持久化到文件
    async fn persist_config(&self, config: &TianyanConfig) -> anyhow::Result<()> {
        let path = TianyanConfig::find_config_file()
            .or_else(TianyanConfig::default_config_path)
            .ok_or_else(|| anyhow::anyhow!("无法确定配置文件路径"))?;

        config
            .save_to_file(&path)
            .map_err(|e| anyhow::anyhow!("保存配置文件失败: {}", e))?;

        info!(path = %path.display(), "配置已保存到: {}", path.display());
        Ok(())
    }
}
