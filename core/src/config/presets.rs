//! Provider 预置表（ADR-046）：内置 provider 的"出厂默认配置"。
//!
//! 一条预置 = 一组默认值（endpoint / 方言 / UA / 动态头 / 是否需 key /
//! 环境变量提示 / 描述）。与 [`super::model::ProviderConfig`] 同构——
//! "内置预置 = 出厂默认配置，用户配置 = 覆盖"（合并语义：显式 > 预置 > 默认）。
//!
//! 匹配：显式 `preset` 字段 > `name` 精确匹配（均大小写不敏感）。
//! **新增服务商 = 表里加一行**（不改请求路径代码）。

use super::model::{DialectPreset, HeaderSource};

/// Provider 预置条目。
///
/// `dynamic_headers` 为静态绑定表（header 名 + 受控变量源）；空表示无动态头。
pub struct ProviderPreset {
    /// 预置 id（与配置 `name` / `preset` 字段匹配，大小写不敏感）。
    pub id: &'static str,
    /// 显示名（UI 预置选择器用）。
    pub display_name: &'static str,
    /// 默认端点（用户显式配置 endpoint 时以用户值为准）。
    pub endpoint: &'static str,
    /// wire 方言预设。
    pub dialect: DialectPreset,
    /// 专属 User-Agent（None = 全局默认 `tianyan/{VERSION}`）。
    pub user_agent: Option<&'static str>,
    /// 动态请求头绑定（header 名 → 受控变量源）。
    pub dynamic_headers: &'static [(&'static str, HeaderSource)],
    /// 是否需要 API key（UI 提示用；本地服务为 false）。
    pub requires_api_key: bool,
    /// API key 环境变量提示（UI 文案用，如 `OPENCODE_API_KEY`）。
    pub api_key_env_hint: Option<&'static str>,
    /// 一句话描述（UI 预置选择器用）。
    pub description: &'static str,
}

/// 内置预置表（ADR-046 首批：ollama / opencode / deepseek）。
pub static PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "ollama",
        display_name: "Ollama（本地）",
        endpoint: "http://localhost:11434/v1",
        dialect: DialectPreset::Ollama,
        user_agent: None,
        dynamic_headers: &[],
        requires_api_key: false,
        api_key_env_hint: None,
        description: "本地运行，无需 API Key",
    },
    ProviderPreset {
        id: "ollama-cloud",
        display_name: "Ollama Cloud",
        endpoint: "https://ollama.com/v1",
        dialect: DialectPreset::Ollama,
        user_agent: None,
        dynamic_headers: &[],
        requires_api_key: true,
        api_key_env_hint: Some("OLLAMA_API_KEY"),
        description: "Ollama 官方云端",
    },
    ProviderPreset {
        id: "opencode",
        display_name: "OpenCode Zen",
        endpoint: "https://opencode.ai/zen/v1",
        dialect: DialectPreset::OpenAiCompatible,
        user_agent: None,
        dynamic_headers: &[("x-opencode-session", HeaderSource::SessionId)],
        requires_api_key: true,
        api_key_env_hint: Some("OPENCODE_API_KEY"),
        description: "OpenCode 官方模型网关",
    },
    ProviderPreset {
        id: "opencode-go",
        display_name: "OpenCode Go",
        endpoint: "https://opencode.ai/zen/go/v1",
        dialect: DialectPreset::OpenAiCompatible,
        user_agent: None,
        dynamic_headers: &[("x-opencode-session", HeaderSource::SessionId)],
        requires_api_key: true,
        api_key_env_hint: Some("OPENCODE_API_KEY"),
        description: "OpenCode 订阅（开源编码模型）",
    },
    ProviderPreset {
        id: "deepseek",
        display_name: "DeepSeek",
        endpoint: "https://api.deepseek.com/v1",
        dialect: DialectPreset::DeepSeek,
        user_agent: None,
        dynamic_headers: &[],
        requires_api_key: true,
        api_key_env_hint: Some("DEEPSEEK_API_KEY"),
        description: "DeepSeek 官方 API",
    },
];

/// 按 id / 名称查找预置（大小写不敏感，精确匹配；空白输入 / 未命中返回 None）。
pub fn find_preset(key: &str) -> Option<&'static ProviderPreset> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_preset_case_insensitive() {
        assert!(find_preset("opencode").is_some());
        assert!(find_preset("OpenCode").is_some());
        assert!(find_preset("OPENCODE-GO").is_some());
        assert!(find_preset("  deepseek  ").is_some());
        assert!(find_preset("unknown-provider").is_none());
        assert!(find_preset("").is_none());
    }

    #[test]
    fn test_opencode_presets_carry_session_binding() {
        // opencode 系（zen / go）均携带 x-opencode-session 会话头绑定（ADR-046）
        for id in ["opencode", "opencode-go"] {
            let preset = find_preset(id).expect("预置存在");
            assert_eq!(
                preset.dynamic_headers,
                &[("x-opencode-session", HeaderSource::SessionId)],
                "{id} 应携带会话头绑定"
            );
        }
    }

    #[test]
    fn test_ollama_presets_dialect() {
        for id in ["ollama", "ollama-cloud"] {
            let preset = find_preset(id).expect("预置存在");
            assert_eq!(preset.dialect, DialectPreset::Ollama, "{id} 方言");
        }
        assert_eq!(
            find_preset("ollama").unwrap().endpoint,
            "http://localhost:11434/v1"
        );
        assert_eq!(
            find_preset("ollama-cloud").unwrap().endpoint,
            "https://ollama.com/v1"
        );
    }

    #[test]
    fn test_deepseek_preset_dialect() {
        let preset = find_preset("deepseek").expect("预置存在");
        assert_eq!(preset.dialect, DialectPreset::DeepSeek);
        assert_eq!(preset.endpoint, "https://api.deepseek.com/v1");
    }

    #[test]
    fn test_preset_ids_unique() {
        let mut ids: Vec<&str> = PROVIDER_PRESETS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len, "预置 id 不得重复");
    }
}
