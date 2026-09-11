// 标准库
use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use tianyan::config::api_types::ModelCatalogInfo;
use tianyan::config::{McpServerEntry, ModelsConfig, TianyanConfig};
use tianyan::model::spec::ModelSpec;

use crate::api::config::types::{ConfigResponse, ModelsResponse, UpdateConfigResponse};
use crate::api::shared::error::ApiError;
use crate::state::{resolve_model_spec, AppState};

/// 解析配置中全部模型的完整规格（key = "{provider}/{model}"），供响应展示。
///
/// 与 [`crate::state::resolve_chat_model_spec`] 共用"单模型解析"逻辑
/// （[`resolve_model_spec`]：显式 > 内置表 > 默认），不重复实现。
fn resolve_all_model_specs(models: &ModelsConfig) -> HashMap<String, ModelSpec> {
    models
        .providers
        .iter()
        .flat_map(|p| {
            p.models.iter().map(|m| {
                (
                    format!("{}/{}", p.name, m.name),
                    resolve_model_spec(&p.name, m),
                )
            })
        })
        .collect()
}

/// 解析配置中全部模型的内置目录命中（advisory：显示名 + 默认档位；
/// key = "{provider}/{model}"，未命中目录的模型不出现）。
fn resolve_all_model_catalogs(models: &ModelsConfig) -> HashMap<String, ModelCatalogInfo> {
    models
        .providers
        .iter()
        .flat_map(|p| {
            p.models.iter().filter_map(|m| {
                tianyan::model::spec::builtin_catalog(&p.name, &m.name).map(|c| {
                    (
                        format!("{}/{}", p.name, m.name),
                        ModelCatalogInfo {
                            display_name: c.display_name.map(str::to_string),
                            reasoning_efforts: c
                                .reasoning_efforts
                                .map(|efforts| efforts.iter().map(|s| s.to_string()).collect()),
                        },
                    )
                })
            })
        })
        .collect()
}

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
        Ok(ConfigResponse {
            model_specs: Some(resolve_all_model_specs(&config.models)),
            model_catalog: Some(resolve_all_model_catalogs(&config.models)),
            config,
        })
    }

    /// 更新配置 — 验证、保存到文件、热重载 Agent
    pub async fn update_config(
        &self,
        config: TianyanConfig,
    ) -> Result<UpdateConfigResponse, ApiError> {
        // 校验 + 持久化 + 热重载（统一序列 persist_and_reload）
        self.persist_and_reload(config).await?;

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
                p.models.iter().map(|m| {
                    // 每个模型自己的思考档位：只来自模型配置（显示 = 配置，无内置注入）
                    // 内置目录命中（advisory：显示名 + 默认档位，仅展示）
                    let catalog =
                        tianyan::model::spec::builtin_catalog(&p.name, &m.name).map(|c| {
                            ModelCatalogInfo {
                                display_name: c.display_name.map(str::to_string),
                                reasoning_efforts: c
                                    .reasoning_efforts
                                    .map(|efforts| efforts.iter().map(|s| s.to_string()).collect()),
                            }
                        });
                    ModelInfo {
                        name: m.name.clone(),
                        provider: p.name.clone(),
                        capabilities: m.capabilities.clone(),
                        reasoning_efforts: m.reasoning_efforts.clone(),
                        context_length: m.context_length,
                        max_output_tokens: m.max_output_tokens,
                        catalog,
                    }
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

        // 校验 + 持久化 + 热重载（统一序列 persist_and_reload）
        self.persist_and_reload(config).await?;

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

        // ? 传播：保存失败（IO/序列化）映射为内部错误（无语义前缀，From 默认 Internal）
        config.save_to_file(&path)?;

        info!(path = %path.display(), "配置已保存到: {}", path.display());
        Ok(())
    }

    /// 校验、持久化并热重载配置。
    async fn persist_and_reload(&self, config: TianyanConfig) -> Result<(), ApiError> {
        config.validate().map_err(ApiError::Config)?;
        self.persist_config(&config).await?;
        self.state
            .update_config(config)
            .await
            // ? 传播：From<TianyanError> 语义谓词映射（热重载失败非 500 专属，
            // not_found/invalid_input 等保持原分类，ADR-014）
            ?;
        Ok(())
    }

    // ─── MCP 服务器管理 ───

    /// 列出所有 MCP 服务器。
    pub async fn list_mcp_servers(&self) -> Vec<McpServerEntry> {
        self.state.config().read().await.clone().mcp.servers
    }

    /// 添加 MCP 服务器（校验 + 持久化 + 热重载）。
    pub async fn add_mcp_server(&self, server: McpServerEntry) -> Result<(), ApiError> {
        server
            .validate()
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;

        let mut config = self.state.config().read().await.clone();
        if config.mcp.servers.iter().any(|s| s.name == server.name) {
            return Err(ApiError::BadRequest(format!(
                "MCP 服务器 '{}' 已存在",
                server.name
            )));
        }
        config.mcp.servers.push(server);
        self.persist_and_reload(config).await
    }

    /// 移除 MCP 服务器。
    pub async fn remove_mcp_server(&self, name: &str) -> Result<(), ApiError> {
        let mut config = self.state.config().read().await.clone();
        let before = config.mcp.servers.len();
        config.mcp.servers.retain(|s| s.name != name);
        if config.mcp.servers.len() == before {
            return Err(ApiError::NotFound(format!("MCP 服务器 '{}' 未找到", name)));
        }
        self.persist_and_reload(config).await
    }

    /// 启用/禁用 MCP 服务器。
    pub async fn toggle_mcp_server(&self, name: &str, enabled: bool) -> Result<(), ApiError> {
        let mut config = self.state.config().read().await.clone();
        let server = config
            .mcp
            .servers
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| ApiError::NotFound(format!("MCP 服务器 '{}' 未找到", name)))?;
        server.enabled = enabled;
        self.persist_and_reload(config).await
    }

    /// 测试 MCP 服务器连接：启动子进程、完成 MCP 握手、统计工具数量。
    pub async fn test_mcp_server(
        &self,
        name: &str,
    ) -> Result<crate::api::config::mcp_handlers::McpTestResponse, ApiError> {
        let config = self.state.config().read().await.clone();
        let entry = config
            .mcp
            .servers
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| ApiError::NotFound(format!("MCP 服务器 '{}' 未找到", name)))?;

        // G7：传输分发唯一实现（mcp crate）——http 走 streamable HTTP，
        // 缺省/stdio 走子进程；url 缺失等错误经 McpError 返回
        let test_result = tianyan_mcp::McpClient::connect_from_config(entry).await;

        match test_result {
            Ok(client) => {
                let tools = client
                    .list_tools()
                    .await
                    .map_err(|e| ApiError::Internal(format!("获取工具列表失败: {}", e)))?
                    .len();
                let _ = client.disconnect().await;
                info!(server = %name, tools, "MCP 服务器连接测试成功");
                Ok(crate::api::config::mcp_handlers::McpTestResponse {
                    success: true,
                    tools,
                    error: None,
                })
            }
            Err(e) => {
                warn!(server = %name, error = %e, "MCP 服务器连接测试失败");
                Ok(crate::api::config::mcp_handlers::McpTestResponse {
                    success: false,
                    tools: 0,
                    error: Some(e.to_string()),
                })
            }
        }
    }

    // ─── Provider 模型注册 ───

    /// 将模型注册进模型配置（按名称查找或创建 provider）。
    pub async fn add_provider_model(
        &self,
        provider_name: &str,
        endpoint: &str,
        model_name: &str,
        capabilities: &[String],
    ) -> Result<(), ApiError> {
        use tianyan::config::ModelCapability;

        let mut config = self.state.config().read().await.clone();

        // 查找或创建指定名称的 provider
        let provider = match config
            .models
            .providers
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(provider_name))
        {
            Some(p) => p,
            None => {
                config
                    .models
                    .providers
                    .push(tianyan::config::ProviderConfig {
                        name: provider_name.to_string(),
                        endpoint: endpoint.to_string(),
                        api_key: None,
                        models: vec![],
                        timeout: 60,
                        enabled: true,
                        headers: Default::default(),
                        thinking_field: None,
                    });
                config
                    .models
                    .providers
                    .last_mut()
                    .ok_or_else(|| ApiError::Internal("创建 Provider 提供商失败".to_string()))?
            }
        };

        // 同步最新端点
        provider.endpoint = endpoint.to_string();

        if provider.models.iter().any(|m| m.name == model_name) {
            return Err(ApiError::BadRequest(format!(
                "模型 '{}' 已存在于 {} 提供商中",
                model_name, provider_name
            )));
        }

        // 映射能力标签，未知标签忽略；空则默认 chat
        let mut mapped: Vec<ModelCapability> = capabilities
            .iter()
            .filter_map(|c| match c.as_str() {
                "chat" => Some(ModelCapability::Chat),
                "vision" => Some(ModelCapability::Vision),
                "text-embedding" => Some(ModelCapability::TextEmbedding),
                "multimodal-embedding" => Some(ModelCapability::MultimodalEmbedding),
                _ => None,
            })
            .collect();
        if mapped.is_empty() {
            mapped.push(ModelCapability::Chat);
        }

        provider.models.push(tianyan::config::ModelEntry {
            name: model_name.to_string(),
            capabilities: mapped,
            ..Default::default()
        });

        self.persist_and_reload(config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tianyan::config::{ModelCapability, ModelEntry, ModelsConfig, ProviderConfig};
    use tianyan::model::spec::ModelSpec;

    fn provider_with(name: &str, models: Vec<ModelEntry>) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            endpoint: "https://example.com/v1".to_string(),
            api_key: None,
            models,
            timeout: 60,
            enabled: true,
            headers: Default::default(),
            thinking_field: None,
        }
    }

    fn chat_entry(name: &str) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            capabilities: vec![ModelCapability::Chat],
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_all_model_catalogs_maps_only_matches() {
        let models = ModelsConfig {
            providers: vec![
                provider_with("deepseek", vec![chat_entry("deepseek-v4-flash")]),
                provider_with("openai", vec![chat_entry("custom-chat")]),
            ],
            ..Default::default()
        };
        let catalogs = resolve_all_model_catalogs(&models);
        // deepseek-v4-flash 命中目录（显示名 + 默认档位）；custom-chat 未命中不出现
        assert_eq!(catalogs.len(), 1);
        let hit = &catalogs["deepseek/deepseek-v4-flash"];
        assert_eq!(hit.display_name.as_deref(), Some("DeepSeek V4 Flash"));
        assert_eq!(
            hit.reasoning_efforts.as_deref(),
            Some(
                &[
                    String::from("low"),
                    String::from("high"),
                    String::from("max")
                ][..]
            )
        );
    }

    #[test]
    fn test_resolve_all_model_specs_maps_every_model() {
        let explicit_entry = ModelEntry {
            context_length: Some(64_000),
            max_output_tokens: Some(4_000),
            max_input_tokens: Some(60_000),
            ..chat_entry("custom-chat")
        };
        let models = ModelsConfig {
            providers: vec![
                provider_with("deepseek", vec![chat_entry("deepseek-v4-flash")]),
                provider_with("openai", vec![explicit_entry]),
                provider_with("ollama", vec![chat_entry("my-local-model")]),
            ],
            ..Default::default()
        };
        let specs = resolve_all_model_specs(&models);
        assert_eq!(specs.len(), 3);
        // 内置表命中（无显式字段）
        assert_eq!(
            specs["deepseek/deepseek-v4-flash"].context_length,
            1_000_000
        );
        assert_eq!(
            specs["deepseek/deepseek-v4-flash"].max_output_tokens,
            32_000
        );
        // 显式字段优先
        assert_eq!(specs["openai/custom-chat"].context_length, 64_000);
        assert_eq!(specs["openai/custom-chat"].max_output_tokens, 4_000);
        assert_eq!(specs["openai/custom-chat"].max_input_tokens, 60_000);
        // 未知模型 → 全局默认 32_768/8_192
        assert_eq!(specs["ollama/my-local-model"], ModelSpec::default());
    }
}
