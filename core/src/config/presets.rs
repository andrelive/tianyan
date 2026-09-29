//! Provider 预置表（ADR-046）：内置 provider 的"出厂默认配置"。
//!
//! 一条预置 = 一组默认值（endpoint / 方言 / UA / 动态头 / 是否需 key /
//! 环境变量提示 / 描述）。与 [`super::model::ProviderConfig`] 同构——
//! "内置预置 = 出厂默认配置，用户配置 = 覆盖"（合并语义：显式 > 预置 > 默认）。
//!
//! 匹配：显式 `preset` 字段 > `name` 精确匹配 > 简称别名（[`PRESET_ALIASES`]，
//! 均大小写不敏感）。**新增服务商 = 表里加一行**（不改请求路径代码）。

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

/// 内置预置表（ADR-046：ollama-cloud / opencode-go / deepseek）。
///
/// 修订（2026-09-29）：原 `ollama`（本地）与 `opencode`（Zen）条目移出——
/// Zen 与 Go 连接参数一致（仅模型标识不同）；本地 Ollama 无连接修饰需求，
/// 直接手写 name / endpoint 即可。两个简称保留为命中别名（[`PRESET_ALIASES`]）。
pub static PROVIDER_PRESETS: &[ProviderPreset] = &[
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

/// `name` / `preset` 命中别名：服务商简称 → 预置 id。
///
/// 别名不新增预置条目（不出现在预置选择器、无独立默认值），仅参与命中——
/// 用户惯用简称与预置 id 对齐：
/// - `opencode` → `opencode-go`（Zen 与 Go 连接参数一致，仅模型标识不同）；
/// - `ollama` → `ollama-cloud`（本地 Ollama 无连接修饰需求，不再内置）。
pub const PRESET_ALIASES: &[(&str, &str)] =
    &[("opencode", "opencode-go"), ("ollama", "ollama-cloud")];

/// 按 id 精确查找（大小写不敏感）。
fn find_by_id(id: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
}

/// 按 id / 名称 / 别名查找预置（大小写不敏感；空白输入 / 未命中返回 None）。
pub fn find_preset(key: &str) -> Option<&'static ProviderPreset> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if let Some(p) = find_by_id(key) {
        return Some(p);
    }
    PRESET_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(key))
        .and_then(|(_, id)| find_by_id(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_preset_case_insensitive() {
        assert!(find_preset("opencode-go").is_some());
        assert!(find_preset("OPENCODE-GO").is_some());
        assert!(find_preset("ollama-cloud").is_some());
        assert!(find_preset("  deepseek  ").is_some());
        assert!(find_preset("unknown-provider").is_none());
        assert!(find_preset("").is_none());
    }

    #[test]
    fn test_aliases_resolve_to_expected_presets() {
        // 用户惯用简称 → 预置（opencode → Go；ollama → Cloud）
        assert_eq!(find_preset("opencode").unwrap().id, "opencode-go");
        assert_eq!(find_preset("OpenCode").unwrap().id, "opencode-go");
        assert_eq!(find_preset("ollama").unwrap().id, "ollama-cloud");
        // 别名目标必须真实存在，且别名不得遮蔽同名真实 id
        for (alias, id) in PRESET_ALIASES {
            let p = find_preset(alias).unwrap_or_else(|| panic!("别名 {alias} 应可解析"));
            assert_eq!(p.id, *id, "别名 {alias} 目标");
            assert!(find_by_id(id).is_some(), "别名目标 {id} 必须存在");
        }
    }

    #[test]
    fn test_opencode_presets_carry_session_binding() {
        // OpenCode 网关（go；含简称别名 opencode）携带 x-opencode-session 会话头绑定（ADR-046）
        for key in ["opencode-go", "opencode"] {
            let preset = find_preset(key).expect("预置存在");
            assert_eq!(
                preset.dynamic_headers,
                &[("x-opencode-session", HeaderSource::SessionId)],
                "{key} 应携带会话头绑定"
            );
        }
    }

    #[test]
    fn test_ollama_cloud_preset_dialect_and_local_removed() {
        // ollama-cloud（含简称别名 ollama）：Ollama 方言 + 云端端点
        for key in ["ollama-cloud", "ollama"] {
            let preset = find_preset(key).expect("预置存在");
            assert_eq!(preset.dialect, DialectPreset::Ollama, "{key} 方言");
            assert_eq!(preset.endpoint, "https://ollama.com/v1", "{key} 端点");
        }
        // 本地 Ollama 不再内置（无连接修饰需求；需要时手写 name / endpoint）
        assert!(
            PROVIDER_PRESETS
                .iter()
                .all(|p| !p.endpoint.contains("11434")),
            "不应再内置本地 Ollama 端点"
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
