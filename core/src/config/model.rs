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

// ─── 传输层思考方言 ───

/// 传输层"思考"方言。
///
/// 决定历史 assistant 消息的思考内容在 HTTP wire 上用哪个字段名表达。
/// 各 OpenAI 兼容实现的字段名并不统一：DeepSeek 官方与 OpenAI 生态用
/// `reasoning_content`，ollama 的 OpenAI 兼容层用 `reasoning`。发错字段名
/// 时服务端会**静默丢弃**（Go 的 `encoding/json` 忽略未知字段），请求仍然
/// 成功，但思考内容根本没进 prompt——上下文与多轮一致性悄悄受损。
///
/// 本类型只描述"字段名"这一件事，与思考强度档位（模型维度）正交。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThinkingField {
    /// DeepSeek / OpenAI 生态：assistant 消息上的 `reasoning_content`。
    #[default]
    #[serde(rename = "reasoning_content")]
    ReasoningContent,
    /// ollama OpenAI 兼容层：assistant 消息上的 `reasoning`。
    #[serde(rename = "reasoning")]
    Ollama,
}

impl ThinkingField {
    /// 该方言在 wire 上使用的字段名。
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::ReasoningContent => "reasoning_content",
            Self::Ollama => "reasoning",
        }
    }

    /// 按 endpoint 与提供商名称嗅探方言（仅在未显式配置时使用）。
    ///
    /// 只认已知的 ollama 域名与默认端口，以及名称里含 `ollama` 的提供商；
    /// 其余一律回落到 `reasoning_content`。**刻意不猜测自建域名/代理**——
    /// 猜错时用户可用 `thinking_field` 显式覆盖，而不是让嗅探去赌。
    pub fn sniff(endpoint: &str, provider_name: &str) -> Self {
        let ep = endpoint.to_ascii_lowercase();
        let name = provider_name.to_ascii_lowercase();
        if ep.contains("ollama") || ep.contains(":11434") || name.contains("ollama") {
            Self::Ollama
        } else {
            Self::ReasoningContent
        }
    }
}

// ─── Wire 方言（各「OpenAI 兼容」实现的细节差异，单点描述）───

/// 缓存命中 token 的 wire 字段来源。
///
/// 各 OpenAI 兼容实现的标识不统一：DeepSeek 官方用顶层 `prompt_cache_hit_tokens`、
/// DashScope 用顶层 `input_cache_hit_tokens`、OpenAI 标准用
/// `prompt_tokens_details.cached_tokens`。
///
/// 失败性质为**可见**（探不到即 0、无歧义），因此消费侧保留全字段探测链兜底：
/// 本枚举只决定**首选**字段（更精确），未命中时仍回落探测（更鲁棒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CacheField {
    /// OpenAI 标准：`prompt_tokens_details.cached_tokens`。
    #[default]
    #[serde(rename = "prompt_tokens_details.cached_tokens")]
    OpenAiDetails,
    /// DeepSeek 官方：顶层 `prompt_cache_hit_tokens`。
    #[serde(rename = "prompt_cache_hit_tokens")]
    DeepSeekTop,
    /// DashScope（阿里云百炼）：顶层 `input_cache_hit_tokens`。
    #[serde(rename = "input_cache_hit_tokens")]
    DashScopeTop,
}

impl CacheField {
    /// wire 字段路径（顶层字段为单元素，嵌套字段按层级展开）。
    pub fn wire_path(self) -> &'static [&'static str] {
        match self {
            Self::OpenAiDetails => &["prompt_tokens_details", "cached_tokens"],
            Self::DeepSeekTop => &["prompt_cache_hit_tokens"],
            Self::DashScopeTop => &["input_cache_hit_tokens"],
        }
    }
}

/// 思考强度参数的 wire 形态。
///
/// 失败性质为**静默**（服务端忽略不认识的参数，请求仍成功但思考过程不受控），
/// 因此缺省只能靠模型名嗅探，且必须可显式覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingParam {
    /// OpenAI 标准：`reasoning_effort`（档位值原样透传）。
    #[serde(rename = "reasoning_effort")]
    ReasoningEffort,
    /// DashScope Qwen 思考族原生：`thinking_budget`（档位 → 预算映射）。
    #[serde(rename = "thinking_budget")]
    ThinkingBudget,
}

impl ThinkingParam {
    /// 缺省嗅探：Qwen 思考族（模型名含 `qwen`，大小写不敏感）用 DashScope 原生
    /// `thinking_budget`，其余 OpenAI 兼容族用 `reasoning_effort`。
    ///
    /// 该差异是**模型级**而非 provider 级（同一 DashScope 下 Qwen 与其它模型不同），
    /// 故嗅探输入是模型名。
    pub fn sniff(model: &str) -> Self {
        if model.to_lowercase().contains("qwen") {
            Self::ThinkingBudget
        } else {
            Self::ReasoningEffort
        }
    }
}

/// 嵌入响应 usage 的字段形状。
///
/// 失败性质为**可见**（缺必填字段直接反序列化失败并上抛），消费侧一律宽容解析：
/// 缺 `prompt_tokens` 时以 `total_tokens` 兜底（嵌入请求没有 completion 侧，
/// 二者语义重合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EmbeddingUsageShape {
    /// 标准：`prompt_tokens` + `total_tokens`。
    #[default]
    #[serde(rename = "prompt_and_total")]
    PromptAndTotal,
    /// 仅 `total_tokens`（DashScope `text-embedding-v4` 实测）。
    #[serde(rename = "total_only")]
    TotalOnly,
}

/// provider 方言预设名。
///
/// 新增服务商的**数据接入面**：命中已知预设 = 加一行映射，不改代码；
/// 长尾 / 自建网关用 [`DialectPreset::Custom`] + 字段级覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DialectPreset {
    /// OpenAI 官方与行为一致的自建网关（全字段默认）。缺省值。
    #[default]
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// DeepSeek 官方。
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// DashScope（阿里云百炼）兼容模式。
    #[serde(rename = "dashscope")]
    DashScope,
    /// ollama（含其 OpenAI 兼容层）。
    #[serde(rename = "ollama")]
    Ollama,
    /// 自定义：不猜任何方言，全部由字段级覆盖决定。
    #[serde(rename = "custom")]
    Custom,
}

impl DialectPreset {
    /// 按端点 / 提供商名称嗅探预设（只认高置信特征）。
    ///
    /// 沿用 `ThinkingField::sniff` 的既有纪律：**刻意不猜测自建域名 / 代理**——
    /// 猜错时用户可显式指定 `dialect` 覆盖，而不是让嗅探去赌。
    pub fn sniff(endpoint: &str, provider_name: &str) -> Self {
        let ep = endpoint.to_ascii_lowercase();
        let name = provider_name.to_ascii_lowercase();
        if ep.contains("ollama") || ep.contains(":11434") || name.contains("ollama") {
            Self::Ollama
        } else if ep.contains("dashscope")
            || ep.contains("aliyuncs")
            || name.contains("dashscope")
            || name.contains("bailian")
        {
            Self::DashScope
        } else if ep.contains("deepseek") || name.contains("deepseek") {
            Self::DeepSeek
        } else {
            Self::OpenAiCompatible
        }
    }
}

/// provider wire 方言：各「OpenAI 兼容」实现细节差异的**单点描述**。
///
/// 构造时由 [`ProviderConfig::resolve_dialect`] 解析一次并缓存在客户端，
/// 请求 / 响应路径上不再做任何判定（与 [`ThinkingField`] 既有形态同构）。
///
/// **设计边界**：只描述**声明式 wire 差异**（字段名 / 参数名 / 字段存在性）。
/// 语义推断类差异（上游错误文本分类、流式空响应识别）**不入本表**——它们是
/// 启发式推断，数据化会退化为「让用户描述错误文本格式」。
///
/// **协议级差异另论**：非 OpenAI 协议（Anthropic Messages / Gemini 等）的差异
/// 不可由本表吸收，应按 `model/provider/mod.rs` 既有约定拆分子目录独立实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDialect {
    /// 历史思考内容的 wire 字段（静默失败 → 必须显式可配）。
    pub thinking_field: ThinkingField,
    /// 缓存命中 token 的首选字段（可见失败 → 允许探测兜底）。
    pub cache_field: CacheField,
    /// 思考参数形态；`None` = 按模型名嗅探（见 [`ThinkingParam::sniff`]）。
    pub thinking_param: Option<ThinkingParam>,
    /// 嵌入响应 usage 形状（可见失败 → 宽容解析）。
    pub embedding_usage: EmbeddingUsageShape,
}

impl ProviderDialect {
    /// 预设 → 方言参数表（新增服务商在此加一行即可）。
    pub fn for_preset(preset: DialectPreset) -> Self {
        match preset {
            DialectPreset::OpenAiCompatible | DialectPreset::Custom => Self {
                thinking_field: ThinkingField::ReasoningContent,
                cache_field: CacheField::OpenAiDetails,
                thinking_param: None,
                embedding_usage: EmbeddingUsageShape::PromptAndTotal,
            },
            DialectPreset::DeepSeek => Self {
                thinking_field: ThinkingField::ReasoningContent,
                cache_field: CacheField::DeepSeekTop,
                thinking_param: None,
                embedding_usage: EmbeddingUsageShape::PromptAndTotal,
            },
            DialectPreset::DashScope => Self {
                thinking_field: ThinkingField::ReasoningContent,
                cache_field: CacheField::DashScopeTop,
                thinking_param: None,
                embedding_usage: EmbeddingUsageShape::TotalOnly,
            },
            DialectPreset::Ollama => Self {
                thinking_field: ThinkingField::Ollama,
                cache_field: CacheField::OpenAiDetails,
                thinking_param: None,
                embedding_usage: EmbeddingUsageShape::PromptAndTotal,
            },
        }
    }

    /// 思考参数方言：显式声明 > 模型名嗅探（模型级差异，输入是模型名）。
    pub fn thinking_param_for(self, model: &str) -> ThinkingParam {
        self.thinking_param
            .unwrap_or_else(|| ThinkingParam::sniff(model))
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
    /// 传输层思考字段名（可选）。
    ///
    /// 缺省时按 endpoint / 名称嗅探（见 [`ThinkingField::sniff`]），再回落
    /// 到 `reasoning_content`。自建代理或嗅探不到的网关用此字段显式指定。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_field: Option<ThinkingField>,
    /// wire 方言预设（可选）。
    ///
    /// 缺省按 endpoint / 名称嗅探（[`DialectPreset::sniff`]）；显式指定用于
    /// 嗅探不到的网关（自定义域名 / 中转）——**这是新增服务商的主接入面**：
    /// 命中已知预设即在 preset 表加一行映射，长尾显式指定或走各自的字段覆盖，
    /// 二者都不改请求路径代码。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dialect: Option<DialectPreset>,
}

impl ProviderConfig {
    /// 解析传输层思考字段名：显式配置 > 方言单点（预设 / 嗅探）。
    ///
    /// 保留本方法作为单字段便捷入口（字段名是静默失败项，历史上先落地）；
    /// 其余方言项一律经 [`ProviderConfig::resolve_dialect`] 取用。
    pub fn resolve_thinking_field(&self) -> ThinkingField {
        self.resolve_dialect().thinking_field
    }

    /// 解析该提供商的 wire 方言（差异判定**单点**）：显式配置 > 预设 > 嗅探 > 默认。
    ///
    /// 在客户端构造时调用一次并缓存，请求 / 响应路径上不再做判定。
    /// 只覆盖声明式 wire 差异；语义推断类差异（错误分类等）不在此列。
    pub fn resolve_dialect(&self) -> ProviderDialect {
        let preset = self
            .dialect
            .unwrap_or_else(|| DialectPreset::sniff(&self.endpoint, &self.name));
        let mut dialect = ProviderDialect::for_preset(preset);
        // 显式字段级覆盖优先于预设 / 嗅探：`thinking_field` 是既有配置面
        // （字段名发错即静默丢上下文），单点化不得让它失效。
        if let Some(field) = self.thinking_field {
            dialect.thinking_field = field;
        }
        dialect
    }
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
    /// 值由厂商/用户自由定义；None = 不支持思考，对话中不显示思考选择）。
    /// "off" 为内置语义：不附加思考参数。
    #[serde(default)]
    pub reasoning_efforts: Option<Vec<String>>,
    /// 思考强度**参数形态**覆盖（可选）。
    ///
    /// 该差异是**模型级**的（同一 provider 下 Qwen 族与其它模型不同），缺省按
    /// 模型名嗅探（[`ThinkingParam::sniff`]）。参数发错会被服务端**静默忽略**
    /// （请求仍成功、但思考强度不受控），故提供显式覆盖逃生门。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_param: Option<ThinkingParam>,
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

    #[test]
    fn test_thinking_field_wire_names() {
        assert_eq!(
            ThinkingField::ReasoningContent.wire_name(),
            "reasoning_content"
        );
        assert_eq!(ThinkingField::Ollama.wire_name(), "reasoning");
    }

    #[test]
    fn test_thinking_field_default_is_reasoning_content() {
        assert_eq!(ThinkingField::default(), ThinkingField::ReasoningContent);
    }

    #[test]
    fn test_thinking_field_sniff_ollama() {
        // 官方云端点、本地默认端口、名称含 ollama——三种已知形态均命中
        for (ep, name) in [
            ("https://ollama.com/v1", "my-cloud"),
            ("http://localhost:11434/v1", "local"),
            ("http://127.0.0.1:11434/v1", "local"),
            ("https://proxy.example.com/v1", "ollama"),
            ("https://OLLAMA.com/v1", "x"),
        ] {
            assert_eq!(
                ThinkingField::sniff(ep, name),
                ThinkingField::Ollama,
                "应嗅探为 Ollama: {ep} / {name}"
            );
        }
    }

    #[test]
    fn test_thinking_field_sniff_defaults_to_reasoning_content() {
        // 刻意不猜自建域名：未识别的端点回落 reasoning_content，由用户显式覆盖
        for (ep, name) in [
            ("https://api.deepseek.com/v1", "deepseek"),
            ("https://api.openai.com/v1", "openai"),
            ("https://gateway.corp.example/v1", "my-gateway"),
            ("https://opencode.ai/zen/go/v1", "opencode"),
        ] {
            assert_eq!(
                ThinkingField::sniff(ep, name),
                ThinkingField::ReasoningContent,
                "应回落 reasoning_content: {ep} / {name}"
            );
        }
    }

    #[test]
    fn test_resolve_thinking_field_precedence() {
        let base = make_provider(make_model("m", vec![ModelCapability::Chat]));

        // 1) 显式配置 > 嗅探：ollama 端点被显式改为 reasoning_content
        let explicit = ProviderConfig {
            endpoint: "https://ollama.com/v1".to_string(),
            thinking_field: Some(ThinkingField::ReasoningContent),
            ..base.clone()
        };
        assert_eq!(
            explicit.resolve_thinking_field(),
            ThinkingField::ReasoningContent,
            "显式配置必须覆盖嗅探"
        );

        // 2) 未配置时按端点嗅探
        let sniffed = ProviderConfig {
            endpoint: "https://ollama.com/v1".to_string(),
            thinking_field: None,
            dialect: None,
            ..base.clone()
        };
        assert_eq!(sniffed.resolve_thinking_field(), ThinkingField::Ollama);

        // 3) 未配置且不可识别 → 默认
        let fallback = ProviderConfig {
            endpoint: "https://api.deepseek.com/v1".to_string(),
            thinking_field: None,
            dialect: None,
            ..base
        };
        assert_eq!(
            fallback.resolve_thinking_field(),
            ThinkingField::ReasoningContent
        );
    }

    #[test]
    fn test_dialect_preset_sniff() {
        // 高置信特征命中；自建网关 / 代理刻意不猜（回落 openai_compatible）
        for (ep, name, expect) in [
            ("https://ollama.com/v1", "x", DialectPreset::Ollama),
            ("http://127.0.0.1:11434/v1", "local", DialectPreset::Ollama),
            (
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
                "x",
                DialectPreset::DashScope,
            ),
            (
                "https://relay.example.com/v1",
                "bailian",
                DialectPreset::DashScope,
            ),
            (
                "https://api.deepseek.com/v1",
                "deepseek",
                DialectPreset::DeepSeek,
            ),
            (
                "https://api.openai.com/v1",
                "openai",
                DialectPreset::OpenAiCompatible,
            ),
            (
                "https://gateway.corp.example/v1",
                "my-gateway",
                DialectPreset::OpenAiCompatible,
            ),
        ] {
            assert_eq!(DialectPreset::sniff(ep, name), expect, "{ep} / {name}");
        }
    }

    #[test]
    fn test_dialect_params_table() {
        let dashscope = ProviderDialect::for_preset(DialectPreset::DashScope);
        assert_eq!(dashscope.cache_field, CacheField::DashScopeTop);
        assert_eq!(
            dashscope.embedding_usage,
            EmbeddingUsageShape::TotalOnly,
            "DashScope 嵌入只回 total_tokens（事故方言项）"
        );
        assert_eq!(dashscope.thinking_field, ThinkingField::ReasoningContent);

        let deepseek = ProviderDialect::for_preset(DialectPreset::DeepSeek);
        assert_eq!(deepseek.cache_field, CacheField::DeepSeekTop);
        assert_eq!(
            deepseek.embedding_usage,
            EmbeddingUsageShape::PromptAndTotal
        );

        let ollama = ProviderDialect::for_preset(DialectPreset::Ollama);
        assert_eq!(ollama.thinking_field, ThinkingField::Ollama);

        let default = ProviderDialect::for_preset(DialectPreset::OpenAiCompatible);
        assert_eq!(default.cache_field, CacheField::OpenAiDetails);
        assert_eq!(default.thinking_param, None, "缺省按模型名嗅探");
    }

    #[test]
    fn test_cache_field_wire_paths() {
        assert_eq!(
            CacheField::OpenAiDetails.wire_path().join("."),
            "prompt_tokens_details.cached_tokens"
        );
        assert_eq!(
            CacheField::DeepSeekTop.wire_path().join("."),
            "prompt_cache_hit_tokens"
        );
        assert_eq!(
            CacheField::DashScopeTop.wire_path().join("."),
            "input_cache_hit_tokens"
        );
    }

    #[test]
    fn test_thinking_param_sniff() {
        assert_eq!(
            ThinkingParam::sniff("qwen3-max"),
            ThinkingParam::ThinkingBudget
        );
        assert_eq!(
            ThinkingParam::sniff("Qwen3-Max"),
            ThinkingParam::ThinkingBudget
        );
        assert_eq!(
            ThinkingParam::sniff("deepseek-v4-flash"),
            ThinkingParam::ReasoningEffort
        );
    }

    #[test]
    fn test_resolve_dialect_from_endpoint() {
        let base = make_provider(make_model("m", vec![ModelCapability::Chat]));

        let dashscope = ProviderConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            ..base.clone()
        };
        assert_eq!(
            dashscope.resolve_dialect().embedding_usage,
            EmbeddingUsageShape::TotalOnly
        );
        assert_eq!(
            dashscope.resolve_dialect().cache_field,
            CacheField::DashScopeTop
        );

        // 回归等价：思考字段名的既有解析路径不漂移（显式 > 方言单点）
        let ollama = ProviderConfig {
            endpoint: "https://ollama.com/v1".to_string(),
            thinking_field: None,
            dialect: None,
            ..base.clone()
        };
        assert_eq!(
            ollama.resolve_dialect().thinking_field,
            ThinkingField::Ollama
        );
        assert_eq!(ollama.resolve_thinking_field(), ThinkingField::Ollama);

        let explicit = ProviderConfig {
            endpoint: "https://ollama.com/v1".to_string(),
            thinking_field: Some(ThinkingField::ReasoningContent),
            ..base
        };
        assert_eq!(
            explicit.resolve_thinking_field(),
            ThinkingField::ReasoningContent,
            "显式配置必须优先于方言单点"
        );
    }

    #[test]
    fn test_dialect_and_model_thinking_param_toml_roundtrip() {
        // 配置面：provider 级 `dialect` 预设 + 模型级 `thinking_param` 覆盖
        let p: ProviderConfig = toml::from_str(
            r#"
            name = "my-gateway"
            endpoint = "https://gw.corp.example/v1"
            dialect = "dashscope"

            [[models]]
            name = "qwen3-max"
            capabilities = ["chat"]
            thinking_param = "reasoning_effort"
            "#,
        )
        .unwrap();

        assert_eq!(p.dialect, Some(DialectPreset::DashScope));
        assert_eq!(
            p.models[0].thinking_param,
            Some(ThinkingParam::ReasoningEffort)
        );
        // 显式 preset 生效（该端点本嗅探不到 dashscope）
        assert_eq!(
            p.resolve_dialect().embedding_usage,
            EmbeddingUsageShape::TotalOnly
        );
        assert_eq!(p.resolve_dialect().cache_field, CacheField::DashScopeTop);
    }

    #[test]
    fn test_dialect_defaults_are_not_serialized() {
        // 缺省项不写回配置（避免污染用户文件）
        let json = serde_json::to_string(&base_provider()).unwrap();
        assert!(!json.contains("dialect"), "{json}");

        let entry = make_model("m", vec![ModelCapability::Chat]);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("thinking_param"), "{json}");
    }

    #[test]
    fn test_thinking_field_serde_roundtrip() {
        // 配置里的写法（snake_case 字符串）必须能解析
        let p: ProviderConfig = toml::from_str(
            r#"
            name = "ollama"
            endpoint = "https://ollama.com/v1"
            thinking_field = "reasoning"

            [[models]]
            name = "m"
            capabilities = ["chat"]
            "#,
        )
        .unwrap();
        assert_eq!(p.thinking_field, Some(ThinkingField::Ollama));
        assert_eq!(p.resolve_thinking_field(), ThinkingField::Ollama);

        // 缺省时不序列化（不污染用户配置文件）
        let json = serde_json::to_string(&base_provider()).unwrap();
        assert!(!json.contains("thinking_field"));
    }

    fn base_provider() -> ProviderConfig {
        ProviderConfig {
            name: "x".to_string(),
            endpoint: "https://x.com".to_string(),
            api_key: None,
            models: vec![],
            timeout: 60,
            enabled: true,
            headers: HashMap::new(),
            thinking_field: None,
            dialect: None,
        }
    }

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
            thinking_field: None,
            dialect: None,
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
            thinking_field: None,
            dialect: None,
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
            thinking_field: None,
            dialect: None,
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
            thinking_field: None,
            dialect: None,
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
            thinking_field: None,
            dialect: None,
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
            thinking_field: None,
            dialect: None,
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
            thinking_field: None,
            dialect: None,
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
                    thinking_field: None,
                    dialect: None,
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
                    thinking_field: None,
                    dialect: None,
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
                thinking_field: None,
                dialect: None,
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
                thinking_field: None,
                dialect: None,
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
