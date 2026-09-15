use async_openai::types::chat::FinishReason;
use async_openai::{config::OpenAIConfig, Client};

use crate::common::error::{Result, TianyanError};
use crate::config::{ProviderConfig, ThinkingField};
use crate::model::retry::RetryPolicy;

/// 基于 async-openai 的模型服务客户端。
///
/// 每个客户端实例绑定一个提供商，可同时服务 chat/embedding/vision 三种能力。
/// 模型选择由调用方通过 model 参数决定，客户端本身不持有模型信息。
#[derive(Debug, Clone)]
pub struct AsyncOpenAIClient {
    pub(crate) service_name: String,
    pub(crate) client: Client<OpenAIConfig>,
    /// 原始 HTTP 客户端（手写流式解析用：async-openai 0.34 不解析
    /// DeepSeek 思考模型的 `reasoning_content` 增量字段）。
    pub(crate) http: reqwest::Client,
    /// 提供商 base URL（如 `https://api.openai.com/v1`）。
    pub(crate) base_url: String,
    /// API Key（可能为空，如本地服务）。
    pub(crate) api_key: String,
    /// 额外请求头（ProviderConfig.headers）。
    pub(crate) headers: std::collections::HashMap<String, String>,
    /// 请求重试策略（指数退避 + 最大重试次数；内置全局默认，对全部 provider 生效）。
    pub(crate) retry_policy: RetryPolicy,
    /// 流式读取空闲超时：相邻两个 SSE 块之间的最长静默间隔。
    /// 语义为"空闲"而非"总时长"——长思维链生成可持续数分钟，总时长
    /// 限制会把活跃长流在中途掐断（0.2.3 会话静默中断根因之一）。
    pub(crate) read_idle: std::time::Duration,
    /// 传输层思考方言：历史 assistant 消息的思考内容用哪个 wire 字段名。
    ///
    /// 构造时按「显式配置 > endpoint/名称嗅探 > 默认」解析一次并缓存；
    /// 请求路径上不再重复判断。字段名不匹配时服务端静默丢弃思考内容，
    /// 请求仍成功——发错不会报错，只会静默损失上下文。
    pub(crate) thinking_field: ThinkingField,
}

impl AsyncOpenAIClient {
    /// 从 ProviderConfig 创建新客户端。
    pub fn from_provider(config: &ProviderConfig) -> Result<Self> {
        let base_url = config
            .get_endpoint()
            .map_err(|e| TianyanError::config(format!("获取 endpoint 失败: {}", e)))?;

        let mut oa_config = OpenAIConfig::default().with_api_base(&base_url);

        let api_key = config.resolve_api_key();
        if !api_key.is_empty() {
            oa_config = oa_config.with_api_key(&api_key);
        }

        let http_client =
            crate::common::http::build_http_client(crate::common::http::HttpClientSpec {
                // 读空闲超时（字节间无数据的最长间隔）——非总请求时长：
                // 流式思维链+长正文可持续数分钟，总时长限制会把长生成
                // 在中途掐断（0.2.3 会话静默中断的根因，见 ADR-023 后续排查）
                timeout: std::time::Duration::from_secs(config.timeout),
                connect_timeout: std::time::Duration::from_secs(30),
                user_agent: None,
                // 模型 API 端点来自用户配置（可信），保持默认重定向跟随
                redirect_policy: None,
            })?;

        let client = Client::with_config(oa_config).with_http_client(http_client.clone());

        Ok(Self {
            service_name: config.name.clone(),
            client,
            http: http_client,
            base_url,
            api_key,
            headers: config.headers.clone(),
            retry_policy: RetryPolicy::default(),
            read_idle: std::time::Duration::from_secs(config.timeout),
            thinking_field: config.resolve_thinking_field(),
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
            thinking_field: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    fn provider(name: &str, endpoint: &str, api_key: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            models: vec![],
            timeout: 30,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
        }
    }

    #[test]
    fn test_from_provider_constructs_client() {
        let config = provider("openai", "https://api.openai.com/v1", Some("sk-test"));
        let client = AsyncOpenAIClient::from_provider(&config).expect("构造客户端");
        assert_eq!(client.service_name(), "openai");
    }

    #[test]
    fn test_from_provider_without_api_key() {
        // 本地服务（Ollama 等）无密钥也应可构造
        let config = provider("local", "http://localhost:11434/v1", None);
        let client = AsyncOpenAIClient::from_provider(&config).expect("无密钥构造客户端");
        assert_eq!(client.service_name(), "local");
    }

    /// 客户端在构造时就把方言解析完毕（请求路径不再判断）：
    /// ollama 端点/名称 → reasoning；其余 → reasoning_content。
    #[test]
    fn test_from_provider_resolves_thinking_field() {
        let ollama_cloud = provider("ollama", "https://ollama.com/v1", Some("k"));
        assert_eq!(
            AsyncOpenAIClient::from_provider(&ollama_cloud)
                .unwrap()
                .thinking_field,
            ThinkingField::Ollama
        );

        let local = provider("local", "http://127.0.0.1:11434/v1", None);
        assert_eq!(
            AsyncOpenAIClient::from_provider(&local)
                .unwrap()
                .thinking_field,
            ThinkingField::Ollama
        );

        let deepseek = provider("deepseek", "https://api.deepseek.com/v1", Some("k"));
        assert_eq!(
            AsyncOpenAIClient::from_provider(&deepseek)
                .unwrap()
                .thinking_field,
            ThinkingField::ReasoningContent
        );
    }

    /// 显式配置覆盖嗅探（自建代理/嗅探不到的网关逃生门）。
    #[test]
    fn test_explicit_thinking_field_overrides_sniff() {
        // 自建网关（嗅探不到）显式声明为 ollama 方言
        let mut custom = provider("my-gateway", "https://gw.corp.example/v1", Some("k"));
        custom.thinking_field = Some(ThinkingField::Ollama);
        assert_eq!(
            AsyncOpenAIClient::from_provider(&custom)
                .unwrap()
                .thinking_field,
            ThinkingField::Ollama
        );

        // ollama 端点被显式改回 DeepSeek 方言
        let mut ollama = provider("ollama", "https://ollama.com/v1", Some("k"));
        ollama.thinking_field = Some(ThinkingField::ReasoningContent);
        assert_eq!(
            AsyncOpenAIClient::from_provider(&ollama)
                .unwrap()
                .thinking_field,
            ThinkingField::ReasoningContent
        );
    }

    #[test]
    fn test_from_provider_rejects_empty_endpoint() {
        let config = provider("bad", "", Some("sk-test"));
        assert!(
            AsyncOpenAIClient::from_provider(&config).is_err(),
            "空 endpoint 应报错"
        );
    }

    #[test]
    fn test_from_provider_rejects_non_http_endpoint() {
        let config = provider("bad", "api.openai.com/v1", Some("sk-test"));
        assert!(
            AsyncOpenAIClient::from_provider(&config).is_err(),
            "非 http(s) endpoint 应报错"
        );
    }

    #[test]
    fn test_finish_reason_str_mapping() {
        assert_eq!(
            AsyncOpenAIClient::finish_reason_str(&FinishReason::Stop),
            "stop"
        );
        assert_eq!(
            AsyncOpenAIClient::finish_reason_str(&FinishReason::Length),
            "length"
        );
        assert_eq!(
            AsyncOpenAIClient::finish_reason_str(&FinishReason::ToolCalls),
            "tool_calls"
        );
        assert_eq!(
            AsyncOpenAIClient::finish_reason_str(&FinishReason::ContentFilter),
            "content_filter"
        );
        assert_eq!(
            AsyncOpenAIClient::finish_reason_str(&FinishReason::FunctionCall),
            "function_call"
        );
    }
}
