//! 模型配置模块。
//!
//! 本模块定义模型提供商、模型条目和能力偏好的统一配置类型。
//!
//! # 配置结构
//!
//! ```toml
//! [[models.providers]]
//! name = "openai"
//! endpoint = "https://api.openai.com/v1"
//! api_key = "${OPENAI_API_KEY}"
//!
//! [[models.providers.models]]
//! name = "deepseek-v4-flash"
//! capabilities = ["chat"]
//! # 思考强度档位（每个模型自己的；缺省查内置模型表，不支持思考的模型不配置）
//! reasoning_efforts = ["off", "low", "medium", "high"]
//!
//! [[models.providers.models]]
//! name = "text-embedding-3-small"
//! capabilities = ["text-embedding"]
//!
//! [models.preferences]
//! chat = { provider = "openai", model = "gpt-4" }
//! embedding = { provider = "openai", model = "text-embedding-3-small" }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── 模型能力标签 ───

/// 模型能力标签。
///
/// 每个模型可以拥有 1-N 个能力标签，表示该模型支持的功能类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelCapability {
    /// 推理 / 对话补全
    Chat,
    /// 多模态理解（VLM — 图片→文本描述）
    Vision,
    /// 文本嵌入（文本→向量）
    TextEmbedding,
    /// 多模态嵌入（图片→向量，用于视觉相似性搜索）
    MultimodalEmbedding,
}

impl std::fmt::Display for ModelCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Chat => write!(f, "chat"),
            Self::Vision => write!(f, "vision"),
            Self::TextEmbedding => write!(f, "text-embedding"),
            Self::MultimodalEmbedding => write!(f, "multimodal-embedding"),
        }
    }
}

// ─── 模型提供商 ───

/// 模型提供商配置。
///
/// 每个提供商拥有独立的连接信息（endpoint + api_key + timeout），
/// 其下可注册多个模型，每个模型声明自己的能力标签。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// 提供商名称（如 "openai"、"deepseek"）。
    pub name: String,
    /// API 端点 URL。
    pub endpoint: String,
    /// API 密钥（支持 `${ENV_VAR}` 格式引用环境变量）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub api_key: Option<String>,
    /// 此提供商下的模型列表。
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    /// 请求超时时间（秒），默认 60。
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// 是否启用此提供商，默认 true。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 自定义请求头。
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl ProviderConfig {
    /// 解析 API 密钥（支持 `${ENV_VAR}` 格式）。
    pub fn resolve_api_key(&self) -> String {
        let raw = self.api_key.as_deref().unwrap_or("");
        if raw.starts_with("${") && raw.ends_with('}') {
            let var_name = &raw[2..raw.len() - 1];
            std::env::var(var_name).unwrap_or_else(|_| raw.to_string())
        } else {
            raw.to_string()
        }
    }

    /// 获取有效的 API 端点 URL。
    pub fn get_endpoint(&self) -> Result<String, String> {
        if self.endpoint.is_empty() {
            Err("API 端点 URL 不能为空".to_string())
        } else if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            Err(format!(
                "API 端点 URL 格式无效：{}，必须以 http:// 或 https:// 开头",
                self.endpoint
            ))
        } else {
            Ok(self.endpoint.clone())
        }
    }

    /// 验证提供商配置是否有效。
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("提供商名称不能为空".to_string());
        }
        self.get_endpoint()?;
        if self.models.is_empty() {
            return Err(format!("提供商 '{}' 至少需要一个模型", self.name));
        }
        for (i, model) in self.models.iter().enumerate() {
            if model.name.is_empty() {
                return Err(format!(
                    "提供商 '{}' 的第 {} 个模型名称为空",
                    self.name,
                    i + 1
                ));
            }
            if model.capabilities.is_empty() {
                return Err(format!(
                    "模型 '{}' (提供商 '{}') 至少需要一个能力标签",
                    model.name, self.name
                ));
            }
            for (field, value) in [
                ("context_length", model.context_length),
                ("max_output_tokens", model.max_output_tokens),
                ("max_input_tokens", model.max_input_tokens),
            ] {
                if value == Some(0) {
                    return Err(format!(
                        "模型 '{}' (提供商 '{}') 的 {} 必须大于 0",
                        model.name, self.name, field
                    ));
                }
            }
        }
        if self.timeout == 0 {
            return Err("超时时间必须大于 0".to_string());
        }
        if self.timeout > 3600 {
            return Err("超时时间不能超过 3600 秒".to_string());
        }
        Ok(())
    }
}

// ─── 模型条目 ───

/// 模型条目。
///
/// 描述一个模型实例的名称和能力标签。
/// 能力标签决定该模型可用于哪种任务（推理、视觉、嵌入等）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelEntry {
    /// 模型名称（如 "gpt-4"、"text-embedding-3-small"）。
    pub name: String,
    /// 能力标签列表。
    #[serde(default)]
    pub capabilities: Vec<ModelCapability>,
    /// 上下文总窗口（token 数，输入+输出共享）。缺省查内置规格表。
    #[serde(default)]
    pub context_length: Option<usize>,
    /// 单次生成最大输出 token 数。缺省查内置规格表。
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
    /// 单次嵌入输入上限（仅 embedding 模型生效）。缺省查内置规格表。
    #[serde(default)]
    pub max_input_tokens: Option<usize>,
    /// 该模型支持的思考强度档位值（每个模型自己声明的档位集，如 ["low","high","max"]，
    /// 值由厂商/用户自由定义；None 时查内置模型表，内置表也没有则视为不支持思考）。
    /// "off" 为内置语义：不附加思考参数。
    #[serde(default)]
    pub reasoning_efforts: Option<Vec<String>>,
}

// ─── 模型偏好 ───

/// 模型偏好配置。
///
/// 按能力维度指定首选模型。
/// 未配置的能力将自动匹配第一个符合条件的模型。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelPreferences {
    /// 推理 / 对话首选模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<ModelRef>,
    /// 文本嵌入首选模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<ModelRef>,
    /// 视觉分析首选模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision: Option<ModelRef>,
}

/// 模型引用。
///
/// 通过提供商名称 + 模型名称唯一定位一个模型实例。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRef {
    /// 提供商名称。
    pub provider: String,
    /// 模型名称。
    pub model: String,
}

// ─── 顶层配置 ───

/// 模型配置。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    /// 模型提供商列表。
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// 能力偏好（可选，不填则自动匹配）。
    #[serde(default)]
    pub preferences: ModelPreferences,
}

impl ModelsConfig {
    /// 验证模型配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.providers.is_empty() {
            return Err("至少需要配置一个模型提供商".to_string());
        }
        for provider in &self.providers {
            provider.validate()?;
        }
        Ok(())
    }

    /// 按能力查找模型条目。
    ///
    /// 优先使用 `preferences` 中指定的模型，未指定则自动匹配
    /// 第一个拥有该能力的已启用模型。
    pub fn resolve(&self, capability: ModelCapability) -> Option<ModelRef> {
        // 1. 检查偏好
        let pref = match capability {
            ModelCapability::Chat => &self.preferences.chat,
            ModelCapability::Vision => &self.preferences.vision,
            ModelCapability::TextEmbedding | ModelCapability::MultimodalEmbedding => {
                &self.preferences.embedding
            }
        };
        if let Some(r) = pref {
            // 验证引用的 provider 存在且模型有对应能力
            if self.verify_model_ref(r, capability) {
                return Some(r.clone());
            }
            tracing::warn!(
                provider = %r.provider,
                model = %r.model,
                capability = %capability,
                "preferences 指定的模型不存在或能力不匹配，回退到自动选择"
            );
        }

        // 2. 自动匹配
        self.auto_resolve(capability)
    }

    /// 自动匹配第一个符合条件的模型。
    fn auto_resolve(&self, capability: ModelCapability) -> Option<ModelRef> {
        for provider in &self.providers {
            if !provider.enabled {
                continue;
            }
            for model in &provider.models {
                if model.capabilities.contains(&capability) {
                    return Some(ModelRef {
                        provider: provider.name.clone(),
                        model: model.name.clone(),
                    });
                }
            }
        }
        None
    }

    /// 验证 ModelRef 是否有效。
    fn verify_model_ref(&self, r: &ModelRef, capability: ModelCapability) -> bool {
        for provider in &self.providers {
            if provider.name == r.provider && provider.enabled {
                for model in &provider.models {
                    if model.name == r.model && model.capabilities.contains(&capability) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

// ─── 获取 provider 连接信息 ───

/// 按名称查找 ProviderConfig。
pub fn find_provider<'a>(
    providers: &'a [ProviderConfig],
    name: &str,
) -> Option<&'a ProviderConfig> {
    providers.iter().find(|p| p.name == name && p.enabled)
}

// ─── 默认值 ───

fn default_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

// ─── 测试 ───

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(name: &str, caps: Vec<ModelCapability>) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            capabilities: caps,
            ..Default::default()
        }
    }

    fn make_provider(entry: ModelEntry) -> ProviderConfig {
        ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: None,
            models: vec![entry],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        }
    }

    #[test]
    fn test_provider_validation() {
        let p = ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            models: vec![make_model("gpt-4", vec![ModelCapability::Chat])],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn test_provider_validation_empty_name() {
        let p = ProviderConfig {
            name: "".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: None,
            models: vec![make_model("gpt-4", vec![ModelCapability::Chat])],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_provider_validation_no_models() {
        let p = ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: None,
            models: vec![],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_provider_validation_invalid_url() {
        let p = ProviderConfig {
            name: "test".to_string(),
            endpoint: "ftp://bad.example.com".to_string(),
            api_key: None,
            models: vec![make_model("gpt-4", vec![ModelCapability::Chat])],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_resolve_api_key_env_var() {
        std::env::set_var("TIANYAN_TEST_KEY2", "env-key-value");
        let p = ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: Some("${TIANYAN_TEST_KEY2}".to_string()),
            models: vec![],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        };
        assert_eq!(p.resolve_api_key(), "env-key-value");
        std::env::remove_var("TIANYAN_TEST_KEY2");
    }

    #[test]
    fn test_resolve_api_key_plain() {
        let p = ProviderConfig {
            name: "test".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: Some("sk-plain".to_string()),
            models: vec![],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
        };
        assert_eq!(p.resolve_api_key(), "sk-plain");
    }

    #[test]
    fn test_resolve_with_preferences() {
        let config = ModelsConfig {
            providers: vec![
                ProviderConfig {
                    name: "openai".to_string(),
                    endpoint: "https://api.openai.com/v1".to_string(),
                    api_key: Some("sk-o".to_string()),
                    timeout: 60,
                    enabled: true,
                    headers: HashMap::new(),
                    models: vec![
                        make_model("gpt-4", vec![ModelCapability::Chat]),
                        make_model(
                            "text-embedding-3-small",
                            vec![ModelCapability::TextEmbedding],
                        ),
                    ],
                },
                ProviderConfig {
                    name: "deepseek".to_string(),
                    endpoint: "https://api.deepseek.com/v1".to_string(),
                    api_key: Some("sk-d".to_string()),
                    timeout: 60,
                    enabled: true,
                    headers: HashMap::new(),
                    models: vec![make_model("deepseek-chat", vec![ModelCapability::Chat])],
                },
            ],
            preferences: ModelPreferences {
                chat: Some(ModelRef {
                    provider: "deepseek".to_string(),
                    model: "deepseek-chat".to_string(),
                }),
                embedding: Some(ModelRef {
                    provider: "openai".to_string(),
                    model: "text-embedding-3-small".to_string(),
                }),
                vision: None,
            },
        };

        let chat = config.resolve(ModelCapability::Chat).unwrap();
        assert_eq!(chat.provider, "deepseek");
        assert_eq!(chat.model, "deepseek-chat");

        let emb = config.resolve(ModelCapability::TextEmbedding).unwrap();
        assert_eq!(emb.provider, "openai");
        assert_eq!(emb.model, "text-embedding-3-small");
    }

    #[test]
    fn test_auto_resolve_fallback() {
        let config = ModelsConfig {
            providers: vec![ProviderConfig {
                name: "openai".to_string(),
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: Some("sk-o".to_string()),
                timeout: 60,
                enabled: true,
                headers: HashMap::new(),
                models: vec![
                    make_model(
                        "gpt-4",
                        vec![ModelCapability::Chat, ModelCapability::Vision],
                    ),
                    make_model(
                        "text-embedding-3-small",
                        vec![ModelCapability::TextEmbedding],
                    ),
                ],
            }],
            preferences: ModelPreferences::default(),
        };

        let chat = config.resolve(ModelCapability::Chat).unwrap();
        assert_eq!(chat.model, "gpt-4");

        let vision = config.resolve(ModelCapability::Vision).unwrap();
        assert_eq!(vision.model, "gpt-4");

        let emb = config.resolve(ModelCapability::TextEmbedding).unwrap();
        assert_eq!(emb.model, "text-embedding-3-small");
    }

    #[test]
    fn test_resolve_embedding_preference_covers_both() {
        // embedding preference covers both TextEmbedding and MultimodalEmbedding
        let config = ModelsConfig {
            providers: vec![ProviderConfig {
                name: "o".to_string(),
                endpoint: "https://x.com".to_string(),
                api_key: Some("k".to_string()),
                timeout: 60,
                enabled: true,
                headers: HashMap::new(),
                models: vec![make_model(
                    "te3",
                    vec![
                        ModelCapability::TextEmbedding,
                        ModelCapability::MultimodalEmbedding,
                    ],
                )],
            }],
            preferences: ModelPreferences {
                embedding: Some(ModelRef {
                    provider: "o".to_string(),
                    model: "te3".to_string(),
                }),
                ..Default::default()
            },
        };

        assert!(config.resolve(ModelCapability::TextEmbedding).is_some());
        assert!(config
            .resolve(ModelCapability::MultimodalEmbedding)
            .is_some());
    }

    #[test]
    fn test_model_capability_serde() {
        let caps = vec![
            ModelCapability::Chat,
            ModelCapability::Vision,
            ModelCapability::TextEmbedding,
        ];
        let json = serde_json::to_string(&caps).unwrap();
        assert_eq!(json, r#"["chat","vision","text-embedding"]"#);

        let parsed: Vec<ModelCapability> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, caps);
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
[[providers]]
name = "openai"
endpoint = "https://api.openai.com/v1"
api_key = "sk-test"

[[providers.models]]
name = "gpt-4"
capabilities = ["chat", "vision"]

[[providers.models]]
name = "text-embedding-3-small"
capabilities = ["text-embedding"]

[preferences]
chat = { provider = "openai", model = "gpt-4" }
embedding = { provider = "openai", model = "text-embedding-3-small" }
"#;

        let config: ModelsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "openai");
        assert_eq!(config.providers[0].models.len(), 2);
        assert_eq!(config.providers[0].models[0].name, "gpt-4");
        assert_eq!(
            config.providers[0].models[0].capabilities,
            vec![ModelCapability::Chat, ModelCapability::Vision]
        );
        assert!(config.preferences.chat.is_some());
    }

    #[test]
    fn test_model_entry_old_config_compat() {
        // 无新字段的旧配置（TOML + JSON）必须反序列化成功，新字段为 None
        let toml_str = r#"
name = "gpt-4"
capabilities = ["chat"]
"#;
        let entry: ModelEntry = toml::from_str(toml_str).unwrap();
        assert_eq!(entry.name, "gpt-4");
        assert_eq!(entry.capabilities, vec![ModelCapability::Chat]);
        assert!(entry.context_length.is_none());
        assert!(entry.max_output_tokens.is_none());
        assert!(entry.max_input_tokens.is_none());

        let json = r#"{"name":"gpt-4","capabilities":["chat"]}"#;
        let entry: ModelEntry = serde_json::from_str(json).unwrap();
        assert!(entry.context_length.is_none());
        assert!(entry.max_output_tokens.is_none());
        assert!(entry.max_input_tokens.is_none());
    }

    #[test]
    fn test_model_entry_new_fields_roundtrip() {
        let toml_str = r#"
name = "gpt-4"
capabilities = ["chat"]
context_length = 128000
max_output_tokens = 8192
max_input_tokens = 8000
"#;
        let entry: ModelEntry = toml::from_str(toml_str).unwrap();
        assert_eq!(entry.context_length, Some(128000));
        assert_eq!(entry.max_output_tokens, Some(8192));
        assert_eq!(entry.max_input_tokens, Some(8000));

        let back: ModelEntry = toml::from_str(&toml::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back.context_length, Some(128000));
        assert_eq!(back.max_output_tokens, Some(8192));
        assert_eq!(back.max_input_tokens, Some(8000));
    }

    #[test]
    fn test_provider_validation_zero_spec_fields() {
        // 三字段任一显式 Some(0) → 校验失败，错误信息指明具体字段
        let mut entry = make_model("gpt-4", vec![ModelCapability::Chat]);

        entry.context_length = Some(0);
        let err = make_provider(entry.clone()).validate().unwrap_err();
        assert!(err.contains("context_length"), "err: {err}");

        entry.context_length = None;
        entry.max_output_tokens = Some(0);
        let err = make_provider(entry.clone()).validate().unwrap_err();
        assert!(err.contains("max_output_tokens"), "err: {err}");

        entry.max_output_tokens = None;
        entry.max_input_tokens = Some(0);
        let err = make_provider(entry).validate().unwrap_err();
        assert!(err.contains("max_input_tokens"), "err: {err}");
    }
}
