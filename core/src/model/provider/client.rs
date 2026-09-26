use async_openai::types::chat::FinishReason;
use async_openai::{config::OpenAIConfig, Client};

use crate::common::error::{Result, TianyanError};
use crate::config::{ProviderConfig, ProviderDialect, ThinkingParam};
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
    /// 首 token 预算（ADR-044）：请求发出 → 首个字节的最长等待——覆盖大
    /// 上下文预填充与弱网首包；与 `read_idle`（块间空闲）独立。
    pub(crate) first_token: std::time::Duration,
    /// provider wire 方言（差异判定**单点**）：思考字段名 / 缓存命中字段 /
    /// 思考参数形态 / 嵌入 usage 形状。
    ///
    /// 构造时按「显式配置 > 预设 > endpoint/名称嗅探 > 默认」解析**一次**并
    /// 缓存；请求 / 响应路径上不再做任何判定（避免同一差异散落多处嗅探）。
    /// 其中字段名类差异发错时服务端静默丢弃（请求仍成功、只静默损失上下文），
    /// 故必须单点可配。
    pub(crate) dialect: ProviderDialect,
    /// 模型级思考参数形态覆盖（来自 `ModelEntry.thinking_param`）。
    ///
    /// 该差异是**模型级**的（同一 provider 下 Qwen 思考族与其它模型的参数形态
    /// 不同），无法只在 provider 级方言里表达；构造时收集一次，请求路径零判定。
    pub(crate) model_thinking_params: std::collections::HashMap<String, ThinkingParam>,
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

        // 双预算（ADR-044）：块间空闲（`timeout`）与首 token 预算
        // （`first_token_timeout`）**分开**；传输层 read_timeout 取两者中
        // 更宽的作兜底——reqwest 的读超时同时约束「等响应头」与「字节间」，
        // 若取小值会在我们的显式判定（分两段）之前先掐断长预填充。
        // （读空闲语义：非总请求时长——流式长输出不受限，见 ADR-023 修正。）
        let read_idle = std::time::Duration::from_secs(config.timeout);
        let first_token = std::time::Duration::from_secs(config.first_token_timeout);
        let http_client =
            crate::common::http::build_http_client(crate::common::http::HttpClientSpec {
                timeout: first_token.max(read_idle),
                connect_timeout: std::time::Duration::from_secs(30),
                user_agent: None,
                // 模型 API 端点来自用户配置（可信），保持默认重定向跟随
                redirect_policy: None,
            })?;

        let client = Client::with_config(oa_config).with_http_client(http_client.clone());

        // 模型级覆盖表：仅收集显式声明项（缺省走模型名嗅探，不占表）。
        let model_thinking_params = config
            .models
            .iter()
            .filter_map(|m| m.thinking_param.map(|p| (m.name.clone(), p)))
            .collect();

        Ok(Self {
            service_name: config.name.clone(),
            client,
            http: http_client,
            base_url,
            api_key,
            headers: config.headers.clone(),
            retry_policy: RetryPolicy::default(),
            read_idle,
            first_token,
            dialect: config.resolve_dialect(),
            model_thinking_params,
        })
    }

    /// 思考参数形态解析（**单点**）：模型级显式 > provider 方言（显式 > 模型名嗅探）。
    pub(crate) fn thinking_param_for(&self, model: &str) -> ThinkingParam {
        self.model_thinking_params
            .get(model)
            .copied()
            .unwrap_or_else(|| self.dialect.thinking_param_for(model))
    }

    /// 从基本参数创建新客户端（便捷方法）。
    pub fn new(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
        first_token_timeout_secs: u64,
    ) -> Result<Self> {
        let provider = ProviderConfig {
            name: name.into(),
            endpoint: endpoint.into(),
            api_key: Some(api_key.into()),
            models: vec![],
            timeout: timeout_secs,
            first_token_timeout: first_token_timeout_secs,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
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
    use crate::config::{ModelEntry, ProviderConfig, ThinkingField, ThinkingParam};

    fn provider(name: &str, endpoint: &str, api_key: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            models: vec![],
            timeout: 30,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
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
                .dialect
                .thinking_field,
            ThinkingField::Ollama
        );

        let local = provider("local", "http://127.0.0.1:11434/v1", None);
        assert_eq!(
            AsyncOpenAIClient::from_provider(&local)
                .unwrap()
                .dialect
                .thinking_field,
            ThinkingField::Ollama
        );

        let deepseek = provider("deepseek", "https://api.deepseek.com/v1", Some("k"));
        assert_eq!(
            AsyncOpenAIClient::from_provider(&deepseek)
                .unwrap()
                .dialect
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
                .dialect
                .thinking_field,
            ThinkingField::Ollama
        );

        // ollama 端点被显式改回 DeepSeek 方言
        let mut ollama = provider("ollama", "https://ollama.com/v1", Some("k"));
        ollama.thinking_field = Some(ThinkingField::ReasoningContent);
        assert_eq!(
            AsyncOpenAIClient::from_provider(&ollama)
                .unwrap()
                .dialect
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

    #[test]
    fn test_thinking_param_precedence() {
        // 模型级显式 > provider 方言 > 模型名嗅探
        let mut config = provider(
            "dashscope",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            Some("k"),
        );
        config.models = vec![
            ModelEntry {
                name: "qwen3-max".to_string(),
                thinking_param: Some(ThinkingParam::ReasoningEffort),
                ..Default::default()
            },
            ModelEntry {
                name: "plain-model".to_string(),
                ..Default::default()
            },
        ];
        let client = AsyncOpenAIClient::from_provider(&config).unwrap();

        // 模型级显式：模型名含 qwen 但声明用 reasoning_effort → 以声明为准
        assert_eq!(
            client.thinking_param_for("qwen3-max"),
            ThinkingParam::ReasoningEffort
        );
        // 未声明 → 回落模型名嗅探
        assert_eq!(
            client.thinking_param_for("plain-model"),
            ThinkingParam::ReasoningEffort
        );
        assert_eq!(
            client.thinking_param_for("qwen3-vl"),
            ThinkingParam::ThinkingBudget
        );
    }

    #[test]
    fn test_explicit_dialect_preset_overrides_sniff() {
        // 显式 dialect 优先于端点 / 名称嗅探（自建中转的逃生门）
        let mut config = provider("my-gateway", "https://gw.corp.example/v1", Some("k"));
        assert_eq!(
            AsyncOpenAIClient::from_provider(&config)
                .unwrap()
                .dialect
                .embedding_usage,
            crate::config::EmbeddingUsageShape::PromptAndTotal,
            "未显式声明时回落默认方言"
        );

        config.dialect = Some(crate::config::DialectPreset::DashScope);
        let client = AsyncOpenAIClient::from_provider(&config).unwrap();
        assert_eq!(
            client.dialect.embedding_usage,
            crate::config::EmbeddingUsageShape::TotalOnly,
            "显式 preset 必须生效"
        );
        assert_eq!(
            client.dialect.cache_field,
            crate::config::CacheField::DashScopeTop
        );
    }
}
