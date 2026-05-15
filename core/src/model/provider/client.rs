use async_openai::{config::OpenAIConfig, Client};

use crate::common::error::{Result, TianyanError};
use crate::model::types::ModelConfig;

/// 基于 async-openai 的模型服务客户端。
#[derive(Debug, Clone)]
pub struct AsyncOpenAIClient {
    pub(crate) service_name: String,
    pub(crate) client: Client<OpenAIConfig>,
}

impl AsyncOpenAIClient {
    /// 创建新的客户端实例。
    pub fn new(config: ModelConfig) -> Result<Self> {
        let base_url = config
            .get_base_url()
            .map_err(|e| TianyanError::Config(format!("获取 base URL 失败: {}", e)))?;

        let mut oa_config = OpenAIConfig::default().with_api_base(&base_url);

        let api_key = config.resolve_api_key();
        if !api_key.is_empty() {
            oa_config = oa_config.with_api_key(&api_key);
        }

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout))
            .build()
            .map_err(|e| TianyanError::Config(format!("构建 HTTP 客户端失败: {}", e)))?;

        let client = Client::with_config(oa_config).with_http_client(http_client);

        Ok(Self {
            service_name: config.name,
            client,
        })
    }

    /// 获取服务名称。
    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}
