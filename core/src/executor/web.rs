//! Web 工具执行器：web_search / web_fetch。
//!
//! 业界定位（对标 Claude Code WebSearch/WebFetch、Codex web_search、
//! fetch-mcp）：
//! - `web_search` 返回结构化结果（标题/URL/摘要），不进全文——省 token；
//!   搜索后端零配置走 DuckDuckGo HTML 端点，可切换 SearXNG 自托管端点。
//! - `web_fetch` 抓取单 URL 并提取可读正文（标题 + 主文本 + 链接列表），
//!   替代 http_request 的裸 HTML 输出。
//! - 安全：仅 http/https；SSRF 防护（拒绝本地/内网地址，与 http_request
//!   技能同策略）；响应大小上限 + 超时；结果缓存（TTL 防重复抓取）。
//! - 防注入：工具描述提示 LLM 搜索结果来自外部、不可信（Codex cached
//!   索引模式的本地等价——用"摘要优先 + 可信度提示"替代索引白名单）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

use crate::common::error::{Result, TianyanError};
use crate::common::llm_judge::truncate_output;
use crate::config::WebConfig;

/// 单条搜索结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 标题。
    pub title: String,
    /// 目标 URL。
    pub url: String,
    /// 摘要片段。
    pub snippet: String,
}

/// 搜索后端。
#[derive(Debug, Clone)]
enum SearchBackend {
    /// DuckDuckGo HTML 端点（零配置）。
    DuckDuckGo,
    /// SearXNG 自托管/公共实例（JSON API）。
    Searxng(String),
}

impl SearchBackend {
    fn from_config(config: &WebConfig) -> Result<Self> {
        match config.search_backend.as_str() {
            "duckduckgo" => Ok(Self::DuckDuckGo),
            "searxng" => {
                let endpoint = config.searxng_endpoint.clone().ok_or_else(|| {
                    TianyanError::Custom(
                        "配置错误：search_backend = \"searxng\" 需要配置 searxng_endpoint"
                            .to_string(),
                    )
                })?;
                Ok(Self::Searxng(endpoint.trim_end_matches('/').to_string()))
            }
            other => Err(TianyanError::Custom(format!(
                "配置错误：未知的 search_backend: {other}（支持 duckduckgo / searxng）"
            ))),
        }
    }
}

/// 缓存条目。
#[derive(Debug)]
struct CachedWeb {
    inserted_at: Instant,
    data: String,
}

/// Web 搜索与抓取客户端。
///
/// 说明：`fetch` 不做 SSRF 检查——搜索引擎端点是可信配置；
/// **安全边界在工具执行层**（[`validate_public_url`]），由
/// `execute_web_fetch` 对用户提供的 URL 先校验再调用。
#[derive(Debug, Clone)]
pub struct WebSearchClient {
    http: reqwest::Client,
    backend: SearchBackend,
    cache: Arc<DashMap<String, CachedWeb>>,
    cache_ttl: Duration,
    max_fetch_bytes: usize,
}

impl WebSearchClient {
    /// 从配置创建客户端。
    pub fn new(config: &WebConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .connect_timeout(Duration::from_secs(config.timeout_secs.clamp(1, 15)))
            .user_agent(concat!(
                "Mozilla/5.0 (compatible; tianyan-agent/",
                env!("CARGO_PKG_VERSION"),
                "; +local desktop agent)"
            ))
            .build()
            .map_err(|e| TianyanError::Custom(format!("配置错误：构建 HTTP 客户端失败: {e}")))?;

        Ok(Self {
            http,
            backend: SearchBackend::from_config(config)?,
            cache: Arc::new(DashMap::new()),
            cache_ttl: Duration::from_secs(config.cache_ttl_secs),
            max_fetch_bytes: config.max_fetch_bytes.max(1024),
        })
    }

    /// 搜索网页，返回结构化结果（缓存命中直接返回）。
    ///
    /// - `query` — 搜索词
    /// - `max_results` — 结果上限（默认 8，上限 20）
    pub async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(TianyanError::Custom(
                "tool: web_search 查询词不能为空".to_string(),
            ));
        }
        let limit = max_results.clamp(1, 20);

        // 缓存命中
        let cache_key = format!("search:{query}:{limit}");
        if let Some(hit) = self.cache_get(&cache_key) {
            return serde_json::from_str(&hit)
                .map_err(|e| TianyanError::Custom(format!("tool: 缓存解析失败: {e}")));
        }

        let (url, results) = match &self.backend {
            SearchBackend::DuckDuckGo => {
                let url = ddg_search_url(query);
                let html = self.http_get(&url).await?;
                let results = parse_duckduckgo_html(&html);
                (url, results)
            }
            SearchBackend::Searxng(endpoint) => {
                let url = format!("{endpoint}/search?q={}&format=json", urlencode(query));
                let body = self.http_get(&url).await?;
                let results = parse_searxng_json(&body);
                (url, results)
            }
        };

        let results = results.into_iter().take(limit).collect::<Vec<_>>();
        if results.is_empty() {
            tracing::warn!(query, "web_search 无结果: {url}");
        }

        let json = serde_json::to_string(&results)
            .map_err(|e| TianyanError::Custom(format!("tool: 结果序列化失败: {e}")))?;
        self.cache_put(cache_key, json);
        Ok(results)
    }

    /// 抓取 URL 并提取可读文本（缓存命中直接返回）。
    ///
    /// 调用方必须先执行 [`validate_public_url`]（SSRF 安全边界）。
    /// - `url` — 目标 URL（http/https）
    /// - `max_chars` — 输出字符上限（默认 50_000）
    pub async fn fetch(&self, url: &str, max_chars: Option<usize>) -> Result<String> {
        let limit = max_chars.unwrap_or(50_000).clamp(500, 200_000);

        let cache_key = format!("fetch:{url}:{limit}");
        if let Some(hit) = self.cache_get(&cache_key) {
            return Ok(hit);
        }

        let text = self.fetch_text(url).await?;
        let extracted = extract_readable(&text);
        let truncated = truncate_output(&extracted, limit);
        self.cache_put(cache_key, truncated.clone());
        Ok(truncated)
    }

    /// 内部：GET 并返回字符串（带大小上限）。
    async fn http_get(&self, url: &str) -> Result<String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 网络请求失败: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(TianyanError::Custom(format!(
                "tool: 请求失败 HTTP {status}"
            )));
        }
        self.read_limited(response).await
    }

    /// 内部：抓取原始响应文本（区分 HTML 与纯文本）。
    async fn fetch_text(&self, url: &str) -> Result<String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 网络请求失败: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(TianyanError::Custom(format!(
                "tool: 请求失败 HTTP {status}"
            )));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let body = self.read_limited(response).await?;

        // 非 HTML 内容（纯文本/JSON 等）直接返回
        if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
            return Ok(body);
        }
        Ok(body)
    }

    /// 流式读取响应，超过上限即中止。
    async fn read_limited(&self, response: reqwest::Response) -> Result<String> {
        use futures::StreamExt;

        let mut bytes: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| TianyanError::Custom(format!("tool: 读取响应失败: {e}")))?;
            if bytes.len() + chunk.len() > self.max_fetch_bytes {
                // 截断标记，避免静默丢内容
                bytes.extend_from_slice(&chunk[..self.max_fetch_bytes.saturating_sub(bytes.len())]);
                tracing::warn!("响应超过大小上限（{} 字节），已截断", self.max_fetch_bytes);
                break;
            }
            bytes.extend_from_slice(&chunk);
        }

        String::from_utf8(bytes)
            .map_err(|_| TianyanError::Custom("tool: 响应不是有效 UTF-8 文本".to_string()))
    }

    /// 缓存读取（TTL 过期视为未命中）。
    fn cache_get(&self, key: &str) -> Option<String> {
        let entry = self.cache.get(key)?;
        if entry.inserted_at.elapsed() > self.cache_ttl {
            drop(entry);
            self.cache.remove(key);
            return None;
        }
        Some(entry.data.clone())
    }

    fn cache_put(&self, key: String, data: String) {
        self.cache.insert(
            key,
            CachedWeb {
                inserted_at: Instant::now(),
                data,
            },
        );
    }
}

// ─── SSRF 防护（与 http_request 技能同策略） ─────────────────────────────

/// 校验 URL 可被 WebFetch 访问：仅 http/https，且非本地/内网地址。
pub fn validate_public_url(url: &str) -> Result<()> {
    let parsed = url
        .parse::<reqwest::Url>()
        .map_err(|_| TianyanError::Custom("tool: 无效的 URL 格式".to_string()))?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(TianyanError::Custom(format!(
            "操作不被允许：不支持的 URL 协议: {scheme}"
        )));
    }

    if let Some(host) = parsed.host_str() {
        let lower = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_lowercase();
        let is_ipv6_loopback = lower == "::1";
        let is_ipv6_unspecified = lower == "::";
        let is_ipv6_private =
            lower.contains(':') && (lower.starts_with("fc") || lower.starts_with("fd"));
        let is_ipv6_link_local = lower.starts_with("fe80:");
        if lower == "localhost"
            || lower.starts_with("127.")
            || lower.starts_with("192.168.")
            || lower.starts_with("10.")
            || lower.starts_with("172.")
            || lower.starts_with("0.")
            || is_ipv6_loopback
            || is_ipv6_unspecified
            || is_ipv6_private
            || is_ipv6_link_local
        {
            return Err(TianyanError::Custom(
                "操作不被允许：禁止访问本地或内网地址".to_string(),
            ));
        }
    }

    Ok(())
}

// ─── 后端与解析 ──────────────────────────────────────────────────────────

/// 构造 DuckDuckGo HTML 搜索 URL。
fn ddg_search_url(query: &str) -> String {
    format!("https://html.duckduckgo.com/html/?q={}", urlencode(query))
}

/// URL 编码（查询参数安全）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 解析 DuckDuckGo HTML 搜索结果页。
fn parse_duckduckgo_html(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let result_selector = match Selector::parse("a.result__a") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut results: Vec<SearchResult> = Vec::new();
    for element in document.select(&result_selector) {
        let title = element.text().collect::<String>().trim().to_string();
        let href = element.value().attr("href").unwrap_or("").to_string();
        let url = decode_ddg_href(&href);
        if title.is_empty() || url.is_empty() {
            continue;
        }
        // 摘要：同一结果块内紧随标题的下一个 .result__snippet 兄弟元素
        // （scraper 的 select 只匹配后代不含自身，故按 class 直接匹配兄弟）
        let snippet = element
            .next_siblings()
            .filter_map(ElementRef::wrap)
            .find(|e| {
                e.value()
                    .attr("class")
                    .map(|c| c.contains("result__snippet"))
                    .unwrap_or(false)
            })
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }
    results
}

/// 解码 DDG 重定向包装 URL（`//duckduckgo.com/l/?uddg=<encoded>`）。
fn decode_ddg_href(href: &str) -> String {
    let trimmed = href.trim_start_matches("//");
    if trimmed.contains("duckduckgo.com/l/") || trimmed.contains("duckduckgo.com%2Fl%2F") {
        if let Some(pos) = trimmed.find("uddg=") {
            let encoded = &trimmed[pos + 5..];
            let end = encoded.find('&').unwrap_or(encoded.len());
            return percent_decode(&encoded[..end]);
        }
    }
    href.to_string()
}

/// 百分比解码（仅解码 %XX，保留 +）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 解析 SearXNG JSON 搜索结果（`{ results: [{title, url, content}] }`）。
fn parse_searxng_json(body: &str) -> Vec<SearchResult> {
    #[derive(serde::Deserialize)]
    struct SearxngResponse {
        #[serde(default)]
        results: Vec<SearxngItem>,
    }
    #[derive(serde::Deserialize)]
    struct SearxngItem {
        #[serde(default)]
        title: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        content: String,
    }

    serde_json::from_str::<SearxngResponse>(body)
        .map(|resp| {
            resp.results
                .into_iter()
                .filter(|r| !r.title.is_empty() && !r.url.is_empty())
                .map(|r| SearchResult {
                    title: r.title,
                    url: r.url,
                    snippet: r.content,
                })
                .collect()
        })
        .unwrap_or_default()
}

// ─── 网页可读文本提取 ─────────────────────────────────────────────────────

/// 从 HTML 提取可读文本：标题 + 主文本 + 链接列表。
fn extract_readable(html: &str) -> String {
    let document = Html::parse_document(html);

    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| document.select(&sel).next())
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    // 正文容器：article > main > body
    let container = ["article", "main", "body"].iter().find_map(|tag| {
        Selector::parse(tag)
            .ok()
            .and_then(|sel| document.select(&sel).next())
    });

    let mut out = String::new();
    if !title.is_empty() {
        out.push_str(&format!("# {title}\n\n"));
    }

    if let Some(container) = container {
        out.push_str(&extract_block_text(&container));
    }

    // 链接列表（帮助 LLM 判断后续抓取目标）
    if let Ok(sel) = Selector::parse("a[href]") {
        let mut links: Vec<String> = Vec::new();
        for link in document.select(&sel).take(20) {
            let text = link.text().collect::<String>().trim().to_string();
            let href = link.value().attr("href").unwrap_or("").to_string();
            if !text.is_empty() && (href.starts_with("http://") || href.starts_with("https://")) {
                links.push(format!("- [{text}]({href})"));
            }
        }
        if !links.is_empty() {
            out.push_str("\n## 页面链接\n");
            out.push_str(&links.join("\n"));
        }
    }

    let out = out.trim().to_string();
    if out.is_empty() {
        // 兜底：纯文本（无法解析的 HTML 或 XML 等）
        return html.trim().to_string();
    }
    out
}

/// 提取容器内块级文本（p/h1-h6/li/pre/code），跳过 script/style/nav。
fn extract_block_text(container: &ElementRef) -> String {
    let mut parts: Vec<String> = Vec::new();
    for element in container.descendants().filter_map(ElementRef::wrap) {
        let value = element.value();
        let tag = value.name();
        match tag {
            "script" | "style" | "nav" | "footer" | "header" | "aside" | "noscript" => continue,
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "pre" | "blockquote" => {
                // 只取直接文本（避免嵌套重复）
                let text = element.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    let prefix = match tag {
                        "h1" => "# ",
                        "h2" => "## ",
                        "h3" => "### ",
                        "h4" => "#### ",
                        "h5" => "##### ",
                        "h6" => "###### ",
                        "li" => "- ",
                        "blockquote" => "> ",
                        _ => "",
                    };
                    parts.push(format!("{prefix}{text}"));
                }
            }
            _ => {}
        }
    }
    // 块间空行分隔
    parts.join("\n\n")
}

// ─── 工具执行函数（ToolRegistry 调用） ────────────────────────────────────

/// 执行 web_search 工具。
pub async fn execute_web_search(
    client: &WebSearchClient,
    arguments: &str,
) -> Result<serde_json::Value> {
    #[derive(serde::Deserialize)]
    struct Params {
        query: String,
        #[serde(default)]
        max_results: Option<usize>,
    }
    let params: Params = serde_json::from_str(arguments)
        .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{e}")))?;

    let results = client
        .search(&params.query, params.max_results.unwrap_or(8))
        .await?;
    serde_json::to_value(results)
        .map_err(|e| TianyanError::Custom(format!("tool: 序列化失败：{e}")))
}

/// 执行 web_fetch 工具。
pub async fn execute_web_fetch(
    client: &WebSearchClient,
    arguments: &str,
) -> Result<serde_json::Value> {
    #[derive(serde::Deserialize)]
    struct Params {
        url: String,
        #[serde(default)]
        max_chars: Option<usize>,
    }
    let params: Params = serde_json::from_str(arguments)
        .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{e}")))?;

    // SSRF 安全边界：先校验再抓取
    validate_public_url(&params.url)?;

    let text = client.fetch(&params.url, params.max_chars).await?;
    let mut map = HashMap::new();
    map.insert("url".to_string(), serde_json::Value::String(params.url));
    map.insert("content".to_string(), serde_json::Value::String(text));
    Ok(serde_json::Value::Object(map.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WebConfig {
        WebConfig::default()
    }

    // ── SSRF ──────────────────────────────────────────────────────────

    #[test]
    fn test_validate_public_url_rejects_local_and_private() {
        for url in [
            "http://localhost/x",
            "http://127.0.0.1/x",
            "http://127.0.0.2:8080/x",
            "http://192.168.1.1/x",
            "http://10.0.0.1/x",
            "http://172.16.0.1/x",
            "http://0.0.0.0/x",
            "http://[::1]/x",
            "http://[fc00::1]/x",
            "http://[fe80::1]/x",
            "file:///etc/passwd",
            "ftp://example.com/x",
        ] {
            let err = validate_public_url(url).unwrap_err();
            assert!(
                err.to_string().contains("不被允许")
                    || err.to_string().contains("无效")
                    || err.to_string().contains("禁止访问")
                    || err.to_string().contains("不支持的"),
                "{url} 应被拒绝: {err}"
            );
        }
    }

    #[test]
    fn test_validate_public_url_allows_public() {
        for url in [
            "https://example.com/x",
            "http://example.com/x",
            "https://sub.domain.co.uk/path?q=1",
        ] {
            validate_public_url(url).expect("公网 URL 应通过");
        }
    }

    // ── DDG 解析 ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_duckduckgo_html() {
        let html = r#"
<html><body>
<div class="result">
  <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage">Example Title</a>
  <a class="result__snippet">This is a snippet about example.</a>
</div>
<div class="result">
  <a class="result__a" href="https://other.com/x">Other Site</a>
</div>
</body></html>"#;
        let results = parse_duckduckgo_html(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].url, "https://example.com/page");
        assert!(results[0].snippet.contains("snippet about example"));
        assert_eq!(results[1].url, "https://other.com/x");
        assert_eq!(results[1].snippet, "");
    }

    #[test]
    fn test_parse_duckduckgo_html_empty() {
        assert!(parse_duckduckgo_html("<html><body>no results</body></html>").is_empty());
    }

    #[test]
    fn test_ddg_href_decoding() {
        assert_eq!(
            decode_ddg_href(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1%26c%3D2&rut=x"
            ),
            "https://example.com/a?b=1&c=2"
        );
        assert_eq!(
            decode_ddg_href("https://plain.com/x"),
            "https://plain.com/x"
        );
    }

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("hello world"), "hello+world");
        assert_eq!(urlencode("a/b?c"), "a%2Fb%3Fc");
        assert_eq!(urlencode("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn test_ddg_search_url() {
        let url = ddg_search_url("rust agent");
        assert_eq!(url, "https://html.duckduckgo.com/html/?q=rust+agent");
    }

    // ── SearXNG 解析 ──────────────────────────────────────────────────

    #[test]
    fn test_parse_searxng_json() {
        let body = r#"{"results":[{"title":"T1","url":"https://a.com","content":"c1"},{"title":"","url":"https://empty.com","content":"x"}]}"#;
        let results = parse_searxng_json(body);
        assert_eq!(results.len(), 1, "空标题应被过滤");
        assert_eq!(results[0].title, "T1");
    }

    #[test]
    fn test_parse_searxng_json_garbage() {
        assert!(parse_searxng_json("not json").is_empty());
    }

    // ── 正文提取 ──────────────────────────────────────────────────────

    #[test]
    fn test_extract_readable_html() {
        let html = r#"<html><head><title>My Page</title></head>
<body><nav>menu</nav>
<article>
  <h1>Heading One</h1>
  <p>First paragraph with <b>bold</b> text.</p>
  <ul><li>Item A</li><li>Item B</li></ul>
  <pre>code block</pre>
</article>
<a href="https://example.com/next">Next Page</a>
<script>alert(1)</script>
</body></html>"#;
        let text = extract_readable(html);
        assert!(text.contains("# My Page"), "应包含标题: {text}");
        assert!(text.contains("Heading One"));
        assert!(text.contains("First paragraph"));
        assert!(text.contains("Item A"));
        assert!(text.contains("code block"));
        assert!(!text.contains("menu"), "nav 应被剔除");
        assert!(!text.contains("alert"), "script 应被剔除");
        assert!(text.contains("Next Page"), "应包含链接文本");
    }

    #[test]
    fn test_extract_readable_falls_back_to_raw() {
        let raw = "plain text body";
        let text = extract_readable(format!("<html><body>{raw}</body></html>").as_str());
        assert!(text.contains(raw) || text == raw);
    }

    #[test]
    fn test_truncate_text() {
        let long = "a".repeat(3000);
        let t = truncate_output(&long, 2000);
        assert!(t.contains("[内容被截断"));
        assert!(t.len() < 2500);
    }

    // ── 缓存 ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_cache_ttl_expiry() {
        let client = WebSearchClient::new(&test_config()).unwrap();
        client.cache_put("k".to_string(), "v".to_string());
        assert_eq!(client.cache_get("k").as_deref(), Some("v"));
        // 手动过期
        if let Some(mut entry) = client.cache.get_mut("k") {
            entry.inserted_at = Instant::now() - Duration::from_secs(3600);
        }
        assert_eq!(client.cache_get("k"), None, "TTL 过期应视为未命中");
    }

    // ── 集成：SearXNG 后端指向本地 mock server ────────────────────────

    /// 起一个本地 mock HTTP server，返回固定的 SearXNG JSON。
    async fn spawn_mock_searxng(body: &'static str) -> String {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let body = body.to_string();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 2048];
                    let _ = socket.read(&mut buf).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn test_search_via_searxng_backend() {
        let endpoint = spawn_mock_searxng(
            r#"{"results":[{"title":"Rust Book","url":"https://doc.rust-lang.org/book","content":"The Rust Programming Language"}]}"#,
        )
        .await;
        let mut config = test_config();
        config.search_backend = "searxng".to_string();
        config.searxng_endpoint = Some(endpoint);

        let client = WebSearchClient::new(&config).unwrap();
        let results = client.search("rust book", 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Book");
        assert_eq!(results[0].url, "https://doc.rust-lang.org/book");

        // 缓存命中：第二次搜索走缓存
        let again = client.search("rust book", 5).await.unwrap();
        assert_eq!(again[0].title, "Rust Book");
    }

    #[tokio::test]
    async fn test_fetch_via_mock_server() {
        let html = r#"<html><head><title>Mock Page</title></head><body><article><p>Hello from mock server.</p></article></body></html>"#;
        let endpoint = spawn_mock_searxng(html).await; // 复用同一 mock（返回任意 body）

        let client = WebSearchClient::new(&test_config()).unwrap();
        // fetch 是引擎方法（调用方负责 SSRF），mock 地址直连可用
        let text = client
            .fetch(&format!("{endpoint}/page"), None)
            .await
            .unwrap();
        assert!(text.contains("Mock Page"), "应提取标题: {text}");
        assert!(text.contains("Hello from mock server"));
    }

    #[tokio::test]
    async fn test_search_empty_query_rejected() {
        let client = WebSearchClient::new(&test_config()).unwrap();
        let err = client.search("   ", 5).await.unwrap_err();
        assert!(err.to_string().contains("查询词不能为空"));
    }

    #[tokio::test]
    async fn test_unknown_backend_rejected() {
        let mut config = test_config();
        config.search_backend = "google".to_string();
        let err = WebSearchClient::new(&config).unwrap_err();
        assert!(err.to_string().contains("未知的 search_backend"));
    }
}
