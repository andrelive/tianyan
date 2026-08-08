//! Web 工具配置（web_search / web_fetch）。

use serde::{Deserialize, Serialize};

/// 默认启用。
fn default_true() -> bool {
    true
}

/// 默认超时（秒）。
fn default_timeout() -> u64 {
    15
}

/// 默认缓存 TTL（秒）。
fn default_cache_ttl() -> u64 {
    600
}

/// 默认抓取大小上限（字节，2MB）。
fn default_max_fetch_bytes() -> usize {
    2 * 1024 * 1024
}

/// Web 工具配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    /// 是否启用 web_search / web_fetch 工具。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 搜索后端：`duckduckgo`（默认，零配置）或 `searxng`（自定义端点）。
    #[serde(default = "default_backend")]
    pub search_backend: String,
    /// SearXNG 端点（`search_backend = "searxng"` 时必填），如 `https://searx.be`。
    #[serde(default)]
    pub searxng_endpoint: Option<String>,
    /// HTTP 请求超时（秒）。
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// 搜索/抓取结果缓存 TTL（秒）。
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
    /// 单次抓取响应大小上限（字节）。
    #[serde(default = "default_max_fetch_bytes")]
    pub max_fetch_bytes: usize,
}

/// 默认搜索后端。
fn default_backend() -> String {
    "duckduckgo".to_string()
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            search_backend: default_backend(),
            searxng_endpoint: None,
            timeout_secs: default_timeout(),
            cache_ttl_secs: default_cache_ttl(),
            max_fetch_bytes: default_max_fetch_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_config_defaults() {
        let cfg = WebConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.search_backend, "duckduckgo");
        assert_eq!(cfg.timeout_secs, 15);
        assert_eq!(cfg.cache_ttl_secs, 600);
        assert_eq!(cfg.max_fetch_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn test_web_config_deserialize_minimal() {
        let cfg: WebConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.enabled, "空配置应使用默认值");
        assert_eq!(cfg.search_backend, "duckduckgo");
    }

    #[test]
    fn test_web_config_roundtrip() {
        let cfg = WebConfig {
            enabled: false,
            search_backend: "searxng".to_string(),
            searxng_endpoint: Some("https://searx.be".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: WebConfig = serde_json::from_str(&json).unwrap();
        assert!(!restored.enabled);
        assert_eq!(restored.search_backend, "searxng");
        assert_eq!(
            restored.searxng_endpoint.as_deref(),
            Some("https://searx.be")
        );
    }
}
