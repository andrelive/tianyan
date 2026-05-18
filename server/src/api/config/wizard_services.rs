//! 配置向导服务
//!
//! 提供配置向导的业务逻辑，包括配置验证、保存和模型连接测试。

use std::sync::Arc;

use tracing::{debug, error, info};

use crate::api::config::wizard_types::{SaveConfigRequest, SaveConfigResponse};
use crate::state::AppState;

/// 配置向导服务
pub struct ConfigWizardService;

impl ConfigWizardService {
    /// 创建新的配置向导服务
    pub fn new() -> Self {
        Self
    }

    /// 获取配置状态
    pub fn get_config_status() -> tianyan::config::ConfigStatus {
        debug!("获取配置状态");
        tianyan::config::TianyanConfig::check_config_status()
    }

    /// 保存配置
    pub async fn save_config(
        &self,
        state: Arc<AppState>,
        request: SaveConfigRequest,
    ) -> SaveConfigResponse {
        info!("保存配置向导数据");

        // 转换为 TianyanConfig 进行验证
        let tianyan_config = request.config.to_tianyan_config();

        // 验证模型配置
        if let Err(errors) = tianyan::config::validate_models_config(&tianyan_config.models) {
            let error_strings = tianyan::config::validation_errors_to_strings(errors);
            return SaveConfigResponse {
                success: false,
                message: format!("模型配置验证失败: {}", error_strings.join(", ")),
                config_path: None,
            };
        }

        // 验证存储配置
        if let Err(errors) = tianyan::config::validate_storage_config(&tianyan_config.storage) {
            let error_strings = tianyan::config::validation_errors_to_strings(errors);
            return SaveConfigResponse {
                success: false,
                message: format!("存储配置验证失败: {}", error_strings.join(", ")),
                config_path: None,
            };
        }

        // 验证智能体配置
        if let Err(errors) = tianyan::config::validate_agent_config(&tianyan_config.agent) {
            let error_strings = tianyan::config::validation_errors_to_strings(errors);
            return SaveConfigResponse {
                success: false,
                message: format!("智能体配置验证失败: {}", error_strings.join(", ")),
                config_path: None,
            };
        }

        // 确定保存路径
        let save_path = match request.save_path {
            Some(path) => path,
            None => match tianyan::config::TianyanConfig::default_config_path() {
                Some(path) => path,
                None => {
                    return SaveConfigResponse {
                        success: false,
                        message: "无法确定默认配置路径".to_string(),
                        config_path: None,
                    };
                }
            },
        };

        // 保存配置
        match tianyan_config.save_to_file(&save_path) {
            Ok(()) => {
                info!("配置已保存到: {:?}", save_path);

                // 更新内存中的配置
                let _ = state.update_config(tianyan_config).await;

                // 重新初始化 Agent
                match state.reload_agent().await {
                    Ok(()) => {
                        info!("Agent 重新初始化成功");
                        SaveConfigResponse {
                            success: true,
                            message: "配置保存成功并已生效".to_string(),
                            config_path: Some(save_path),
                        }
                    }
                    Err(e) => {
                        error!("Agent 重新初始化失败: {}", e);
                        SaveConfigResponse {
                            success: true,
                            message: format!("配置已保存但无法生效: {}。请检查配置后重试。", e),
                            config_path: Some(save_path),
                        }
                    }
                }
            }
            Err(e) => {
                error!("保存配置失败: {}", e);
                SaveConfigResponse {
                    success: false,
                    message: format!("保存配置失败: {}", e),
                    config_path: None,
                }
            }
        }
    }

    /// 测试模型连接
    pub async fn test_connection(
        &self,
        request: tianyan::config::TestConnectionRequest,
    ) -> tianyan::config::TestConnectionResponse {
        info!("测试模型连接: {}", request.endpoint);

        // 验证 URL 格式
        if !request.endpoint.starts_with("http://") && !request.endpoint.starts_with("https://") {
            return tianyan::config::TestConnectionResponse::error(
                "API 端点 URL 格式无效，必须以 http:// 或 https:// 开头",
            );
        }

        // 验证 API 密钥
        if request.api_key.is_empty() {
            return tianyan::config::TestConnectionResponse::error("API 密钥不能为空");
        }

        // 验证模型名称
        if request.model.is_empty() {
            return tianyan::config::TestConnectionResponse::error("模型名称不能为空");
        }

        // 尝试创建临时客户端并测试连接
        match Self::test_model_connection(&request.endpoint, &request.api_key, &request.model).await
        {
            Ok(models) => tianyan::config::TestConnectionResponse::success("连接成功", models),
            Err(e) => {
                error!("模型连接测试失败: {}", e);
                tianyan::config::TestConnectionResponse::error(format!("连接失败: {}", e))
            }
        }
    }

    /// 测试模型连接的内部实现
    async fn test_model_connection(
        endpoint: &str,
        api_key: &str,
        model: &str,
    ) -> anyhow::Result<Vec<String>> {
        use tianyan::model::types::ChatCompletionRequest;
        use tianyan::model::ChatService;
        use tianyan::Message;

        // 创建临时客户端
        let client = crate::core_bridge::create_model_client(endpoint, api_key)?;

        // 构建一个简单的测试请求
        let request = ChatCompletionRequest::new(
            model.to_string(),
            vec![Message::system("You are a helpful assistant.")],
        )
        .with_temperature(0.7)
        .with_max_tokens(10)
        .with_stream(false)
        .with_enable_thinking(false);

        // 尝试发送请求
        match client.chat_completion(request).await {
            Ok(_) => {
                // 连接成功，返回可用模型列表
                // 这里我们假设用户提供的模型是可用的
                // 实际应用中可以通过 API 获取可用模型列表
                Ok(vec![model.to_string()])
            }
            Err(e) => {
                // 检查是否是认证错误
                let error_msg = e.to_string();
                if error_msg.contains("401") || error_msg.contains("unauthorized") {
                    Err(anyhow::anyhow!("API 密钥无效或已过期"))
                } else if error_msg.contains("404") {
                    Err(anyhow::anyhow!("模型不存在，请检查模型名称"))
                } else if error_msg.contains("timeout") {
                    Err(anyhow::anyhow!("连接超时，请检查网络或 API 端点"))
                } else {
                    Err(anyhow::anyhow!("连接失败: {}", error_msg))
                }
            }
        }
    }
}

impl Default for ConfigWizardService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wizard_service_creation() {
        let _service = ConfigWizardService::new();
    }
}
