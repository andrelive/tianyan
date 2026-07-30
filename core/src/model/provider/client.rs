use async_openai::types::chat::FinishReason;
use async_openai::{config::OpenAIConfig, Client};

use crate::common::error::{Result, TianyanError};
use crate::config::ProviderConfig;

/// 基于 async-openai 的模型服务客户端。
///
/// 每个客户端实例绑定一个提供商，可同时服务 chat/embedding/vision 三种能力。
/// 模型选择由调用方通过 model 参数决定，客户端本身不持有模型信息。
#[derive(Debug, Clone)]
pub struct AsyncOpenAIClient {
    pub(crate) service_name: String,
    pub(crate) client: Client<OpenAIConfig>,
}

impl AsyncOpenAIClient {
    /// 从 ProviderConfig 创建新客户端。
    pub fn from_provider(config: &ProviderConfig) -> Result<Self> {
        let base_url = config
            .get_endpoint()
            .map_err(|e| TianyanError::Custom(format!("配置错误：获取 endpoint 失败: {}", e)))?;

        let mut oa_config = OpenAIConfig::default().with_api_base(&base_url);

        let api_key = config.resolve_api_key();
        if !api_key.is_empty() {
            oa_config = oa_config.with_api_key(&api_key);
        }

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout))
            .connect_timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(5)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()
            .map_err(|e| TianyanError::Custom(format!("配置错误：构建 HTTP 客户端失败: {}", e)))?;

        let client = Client::with_config(oa_config).with_http_client(http_client);

        Ok(Self {
            service_name: config.name.clone(),
            client,
        })
    }

    /// 从基本参数创建新客户端（便捷方法）。
    pub fn new(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let provider = ProviderConfig {
            name: name.into(),
            endpoint: endpoint.into(),
            api_key: Some(api_key.into()),
            models: vec![],
            timeout: timeout_secs,
            enabled: true,
            headers: std::collections::HashMap::new(),
        };
        Self::from_provider(&provider)
    }

    /// 获取服务名称。
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub(crate) fn finish_reason_str(reason: &FinishReason) -> &'static str {
        match reason {
            FinishReason::Stop => "stop",
            FinishReason::Length => "length",
            FinishReason::ToolCalls => "tool_calls",
            FinishReason::ContentFilter => "content_filter",
            FinishReason::FunctionCall => "function_call",
        }
    }
}
