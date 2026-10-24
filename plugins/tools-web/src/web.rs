//! Web 工具执行器：web_search / web_fetch。
//!
//! 业界定位（对标 Claude Code WebSearch/WebFetch、Codex web_search、
//! fetch-mcp）：
//! - `web_search` 返回结构化结果（标题/URL/摘要），不进全文——省 token；
//!   搜索后端默认走 360（so.com，国内可达），可切换 DuckDuckGo HTML / SearXNG
//!   自托管端点 / DeepSeek 官方搜索（`/v1/tools/search`，需 API Key）。
//! - `web_fetch` 抓取单 URL 并提取可读正文（标题 + 主文本 + 链接列表），
//!   替代 http_request 的裸 HTML 输出。
//! - 安全：仅 http/https；SSRF 防护（拒绝本地/内网地址，与 http_request
//!   技能同策略）；响应大小上限 + 超时；结果缓存（TTL 防重复抓取）。
//! - 防注入：工具描述提示 LLM 搜索结果来自外部、不可信（Codex cached
//!   索引模式的本地等价——用"摘要优先 + 可信度提示"替代索引白名单）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

use tianyan::common::error::{Result, TianyanError};
use tianyan::common::llm_judge::truncate_output;
use tianyan::config::WebConfig;

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
    /// DuckDuckGo HTML 端点（零配置；部分地区网络不可达）。
    DuckDuckGo,
    /// SearXNG 自托管/公共实例（JSON API）。
    Searxng(String),
    /// 360 搜索（`so.com`，国内可达；结果页 `data-mdurl` 直出真实 URL，无需解重定向）。
    So360,
    /// DeepSeek 官方搜索接口（`/v1/tools/search`；自研引擎 + 结构化 JSON
    ///（含成段 `summary` 摘要）；需 `deepseek_api_key`，按次计费）。
    Deepseek {
        /// API Key（Bearer 鉴权）。
        api_key: String,
        /// 搜索接口基地址（无尾部 `/`；默认官方，可指向兼容网关）。
        endpoint: String,
    },
}

/// DeepSeek 官方搜索接口基地址（`deepseek_endpoint` 未配置时的默认值）。
const DEEPSEEK_SEARCH_BASE: &str = "https://api.deepseek.com";

impl SearchBackend {
    fn from_config(config: &WebConfig) -> Result<Self> {
        match config.search_backend.as_str() {
            "duckduckgo" => Ok(Self::DuckDuckGo),
            "360" => Ok(Self::So360),
            "searxng" => {
                let endpoint = config.searxng_endpoint.clone().ok_or_else(|| {
                    TianyanError::config("search_backend = \"searxng\" 需要配置 searxng_endpoint")
                })?;
                Ok(Self::Searxng(endpoint.trim_end_matches('/').to_string()))
            }
            "deepseek" => {
                let api_key = config
                    .deepseek_api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        TianyanError::config(
                            "search_backend = \"deepseek\" 需要配置 deepseek_api_key",
                        )
                    })?;
                let endpoint = config
                    .deepseek_endpoint
                    .as_deref()
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                    .map(|e| e.trim_end_matches('/').to_string())
                    .unwrap_or_else(|| DEEPSEEK_SEARCH_BASE.to_string());
                Ok(Self::Deepseek { api_key, endpoint })
            }
            "bing" => Err(TianyanError::config(
                "search_backend = \"bing\" 已移除（Bing 对自动化流量返回降级结果）——请改用 360 / duckduckgo / searxng / deepseek",
            )),
            other => Err(TianyanError::config(format!(
                "未知的 search_backend: {other}（支持 360 / duckduckgo / searxng / deepseek）"
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

/// Web 结果缓存条目上限（条数）——满员时淘汰**最旧**条目。
///
/// T2：此前缓存无上限，长驻进程内随访问条目单调增长（仅 TTL 过期才回收）。
const MAX_CACHE_ENTRIES: usize = 256;

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
        let http =
            tianyan::common::http::build_http_client(tianyan::common::http::HttpClientSpec {
                timeout: Duration::from_secs(config.timeout_secs.max(1)),
                connect_timeout: Duration::from_secs(config.timeout_secs.clamp(1, 15)),
                http1_only: false,
                user_agent: Some(
                    concat!(
                        "Mozilla/5.0 (compatible; tianyan-agent/",
                        env!("CARGO_PKG_VERSION"),
                        "; +local desktop agent)"
                    )
                    .to_string(),
                ),
                // 逐跳 SSRF 校验（T0-3）：初始 URL 由调用方 validate_public_url
                // 校验，重定向目标在每一跳发出前再校验——否则公网 URL 可以 302
                // 到内网绕过防护（reqwest 默认静默跟随，最多 10 跳）。
                redirect_policy: Some(reqwest::redirect::Policy::custom(|attempt| {
                    if validate_public_url(attempt.url().as_str()).is_err() {
                        attempt.error("禁止访问本地或内网地址")
                    } else {
                        attempt.follow()
                    }
                })),
            })?;

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
            SearchBackend::Deepseek { api_key, endpoint } => {
                let url = format!("{endpoint}/v1/tools/search");
                let results = self.deepseek_search(api_key, &url, query, limit).await?;
                (url, results)
            }
            SearchBackend::So360 => {
                let url = so360_search_url(query);
                let html = self.http_get(&url).await?;
                let results = parse_so360_html(&html);
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

    /// DeepSeek 官方搜索接口（POST JSON；Bearer 鉴权）。
    ///
    /// 契约见 <https://api-docs.deepseek.com/zh-cn/guides/search>：请求
    /// `{query, count, need_content}`（count 1~50）；结果只需标题/URL/摘要，
    /// `need_content = false`（不拉正文）。计费按请求次数（仅非空结果计费）；
    /// 错误体为 OpenAI 兼容格式（`error.message`）。
    async fn deepseek_search(
        &self,
        api_key: &str,
        url: &str,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchResult>> {
        #[derive(serde::Serialize)]
        struct RequestBody<'a> {
            query: &'a str,
            count: usize,
            need_content: bool,
        }
        let response = self
            .http
            .post(url)
            .bearer_auth(api_key)
            .json(&RequestBody {
                query,
                count,
                need_content: false,
            })
            .send()
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 网络请求失败: {e}")))?;
        let status = response.status();
        let body = self.read_limited(response).await?;
        if !status.is_success() {
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| body.chars().take(200).collect());
            return Err(TianyanError::Custom(format!(
                "tool: DeepSeek 搜索失败 HTTP {status}：{message}"
            )));
        }
        Ok(parse_deepseek_json(&body))
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

    /// 流式读取响应，超过上限即中止（字节上限可能切在多字节字符中间，
    /// 解码由 [`decode_body`] 按字符边界容错——T1-22）。
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

        decode_body(bytes)
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
        // 容量治理（T2）：满员时先淘汰最旧（按 inserted_at）再插入。
        // 先收集键再 remove/insert——避免在 DashMap 迭代借用未释放时改表（会死锁）。
        if self.cache.len() >= MAX_CACHE_ENTRIES {
            let oldest = self
                .cache
                .iter()
                .min_by_key(|e| e.value().inserted_at)
                .map(|e| e.key().clone());
            if let Some(k) = oldest {
                self.cache.remove(&k);
            }
        }
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

/// 构造 360 搜索 URL（so.com；国内可达；结果页 `data-mdurl` 直出真实 URL）。
fn so360_search_url(query: &str) -> String {
    format!("https://www.so.com/s?q={}", urlencode(query))
}

/// 解析 DeepSeek 搜索接口响应（`{results:[{title,url,snippet,summary,...}]}`）。
///
/// 摘要取 `summary`（成段摘要，信息更完整），为空回退 `snippet`；标题 /
/// URL 为空的条目过滤（与其它后端一致）。
fn parse_deepseek_json(body: &str) -> Vec<SearchResult> {
    #[derive(serde::Deserialize)]
    struct DeepseekResponse {
        #[serde(default)]
        results: Vec<DeepseekItem>,
    }
    #[derive(serde::Deserialize)]
    struct DeepseekItem {
        #[serde(default)]
        title: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        snippet: String,
        #[serde(default)]
        summary: String,
    }

    serde_json::from_str::<DeepseekResponse>(body)
        .map(|resp| {
            resp.results
                .into_iter()
                .filter(|r| !r.title.is_empty() && !r.url.is_empty())
                .map(|r| {
                    let snippet = if r.summary.trim().is_empty() {
                        r.snippet
                    } else {
                        r.summary
                    };
                    SearchResult {
                        title: r.title,
                        url: r.url,
                        snippet,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 360 搜索结果页（so.com）。
///
/// 结构（2026-10 实测）：`ul.result > li.res-list`；每条结果标题为
/// `h3.res-title > a[data-mdurl]`（data-mdurl 直出真实 URL，无需解重定向；
/// 标题文本含 `<em>` 高亮词，`text()` 自然拼接）；摘要统一取
/// `[data-result-summary]` 文本（普通型位于 `p.res-desc` 内、富摘要型位于
/// `span.res-list-summary` 内）。同 URL 去重——页面导航等位置也带
/// data-mdurl，故限定在结果条目与标题链接内选取。
fn parse_so360_html(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let item_sel = match Selector::parse("li.res-list") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let title_sel = match Selector::parse("h3.res-title > a") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let summary_sel = match Selector::parse("[data-result-summary]") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut results: Vec<SearchResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item in document.select(&item_sel) {
        let link = match item.select(&title_sel).next() {
            Some(l) => l,
            None => continue,
        };
        let url = link
            .value()
            .attr("data-mdurl")
            .unwrap_or("")
            .trim()
            .to_string();
        let title = link.text().collect::<String>().trim().to_string();
        if title.is_empty() || url.is_empty() {
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }
        let snippet = item
            .select(&summary_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }
    results
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

/// 解码响应字节为文本（T1-22：字节上限可能把多字节字符切成两半）。
///
/// - 整体合法 UTF-8 → 直接返回；
/// - **尾部不完整序列**（截断切在多字节字符中间）→ 回退到最后一个完整字符
///   （旧行为直接报错，整个 fetch 失败）；
/// - 中间存在非法序列（响应本身不是 UTF-8）→ lossy 降级保留可读部分；替换
///   字符占比 > 20% 视为二进制响应（仍报错）。
fn decode_body(bytes: Vec<u8>) -> Result<String> {
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(e) => {
            let valid_up_to = e.utf8_error().valid_up_to();
            let incomplete_tail = e.utf8_error().error_len().is_none();
            let raw = e.into_bytes();
            if incomplete_tail {
                tracing::warn!(
                    bytes = raw.len(),
                    kept = valid_up_to,
                    "响应截断落在多字节字符中间，已回退到字符边界"
                );
                return Ok(String::from_utf8_lossy(&raw[..valid_up_to]).into_owned());
            }
            let lossy = String::from_utf8_lossy(&raw);
            let total = lossy.chars().count();
            let bad = lossy.chars().filter(|c| *c == '\u{FFFD}').count();
            if total > 0 && bad * 5 > total {
                return Err(TianyanError::Custom(
                    "tool: 响应不是有效 UTF-8 文本".to_string(),
                ));
            }
            tracing::warn!(
                bytes = raw.len(),
                invalid = bad,
                "响应含非 UTF-8 字节，已降级保留可读部分"
            );
            Ok(lossy.into_owned())
        }
    }
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

    // ── DeepSeek 解析 ────────────────────────────────────────────────

    #[test]
    fn test_parse_deepseek_json() {
        // 真实响应结构（docs 示例口径）：含 summary / snippet 双摘要字段
        let body = r#"{
          "results": [
            {
              "rerank_score": 2.4,
              "title": "DeepSeek V4 模型发布",
              "url": "https://example.com/deepseek-v4",
              "snippet": "DeepSeek V4 已上线。",
              "summary": "DeepSeek 发布了 V4 模型，改进了工具调用与长上下文能力。",
              "site_name": "Example News",
              "authority": 3
            },
            {
              "title": "仅 snippet 条目",
              "url": "https://example.com/b",
              "snippet": "只有片段",
              "summary": ""
            },
            { "title": "", "url": "https://example.com/c", "snippet": "空标题应被过滤" }
          ]
        }"#;
        let results = parse_deepseek_json(body);
        assert_eq!(results.len(), 2, "空标题条目应被过滤");
        assert_eq!(results[0].title, "DeepSeek V4 模型发布");
        assert!(
            results[0].snippet.contains("发布了 V4"),
            "应优先取成段 summary"
        );
        assert_eq!(results[1].snippet, "只有片段", "summary 为空回退 snippet");
    }

    #[test]
    fn test_parse_deepseek_json_garbage() {
        assert!(parse_deepseek_json("not json").is_empty());
    }

    // ── 360 解析 ─────────────────────────────────────────────────────

    #[test]
    fn test_so360_search_url() {
        let url = so360_search_url("rust agent");
        assert_eq!(url, "https://www.so.com/s?q=rust+agent");
    }

    #[test]
    fn test_parse_so360_html() {
        // 结构取自 2026-10 真实抓取（片段裁剪）：富摘要型 + 普通型 +
        // 无 data-mdurl 条目（应过滤）。
        let html = r#"<ul class="result">
<li class="res-list" data-result-type="richsum" data-result-pos="1"><h3   class="res-title"><a href="https://www.so.com/link?m=x" data-mdurl="https://blog.csdn.net/a/1" rel="noopener" target="_blank">深入<em>Loom</em>模型：解密 C11 内存模型在 <em>Rust</em> 并发测试中的应用</a></h3><div class="res-comm-con"><p></p><span class="res-list-summary" data-result-summary><em>loom</em>::thread::yield_now(); 然后使用 <em>loom</em>::model 定义测试场景。</span></div></li>
<li class="res-list" data-result-type="normal" data-result-pos="3"><h3 class="res-title "><a href="https://www.so.com/link?m=y" data-mdurl="https://blog.csdn.net/b/2" rel="noopener" target="_blank"><em>Rust</em> 异步运行时的正确性验证</a></h3><p class="res-desc"><span class="gray g-c-gray">2026年7月20日&nbsp;-&nbsp;</span><span data-result-summary>传统的压力测试几乎无法系统性暴露这类 Bug，而 loom 的模型检测通过穷举线程交错解决。</span></p></li>
<li class="res-list" data-result-type="normal"><h3 class="res-title"><a href="https://www.so.com/link?m=z" target="_blank">没有 data-mdurl 的条目</a></h3></li>
</ul>"#;
        let results = parse_so360_html(html);
        assert_eq!(results.len(), 2, "无 data-mdurl 的条目应被过滤");
        assert_eq!(
            results[0].title,
            "深入Loom模型：解密 C11 内存模型在 Rust 并发测试中的应用"
        );
        assert_eq!(results[0].url, "https://blog.csdn.net/a/1");
        assert!(results[0].snippet.contains("loom::thread::yield_now()"));
        assert_eq!(results[1].url, "https://blog.csdn.net/b/2");
        assert!(results[1].snippet.contains("传统的压力测试"));
    }

    #[test]
    fn test_parse_so360_html_dedup() {
        // 同 URL 重复出现 → 只保留一条
        let html = r#"<ul class="result">
<li class="res-list"><h3 class="res-title"><a data-mdurl="https://example.com/a" href="x">A</a></h3></li>
<li class="res-list"><h3 class="res-title"><a data-mdurl="https://example.com/a" href="x">A 重复</a></h3></li>
</ul>"#;
        let results = parse_so360_html(html);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_parse_so360_html_garbage() {
        assert!(parse_so360_html("not html at all").is_empty());
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

    // ── 响应解码（T1-22） ─────────────────────────────────────────────

    #[test]
    fn test_decode_body_valid_utf8() {
        assert_eq!(
            decode_body("你好 world".as_bytes().to_vec()).unwrap(),
            "你好 world"
        );
    }

    #[test]
    fn test_decode_body_truncated_multibyte_tail_keeps_prefix() {
        // 字节上限切在多字节字符中间（尾部「文」只剩 2/3 字节）——不得报错
        let full = "abc中文".as_bytes().to_vec();
        let truncated = full[..full.len() - 1].to_vec();
        let text = decode_body(truncated).unwrap_or_else(|e| panic!("截断不应报错: {e}"));
        assert_eq!(text, "abc中", "应回退到最后一个完整字符");
    }

    #[test]
    fn test_decode_body_few_invalid_bytes_degrades_lossily() {
        // 文本中夹少量非法字节（如 latin-1 的 0xE9）→ 降级保留可读部分
        let mut bytes = "hello ".as_bytes().to_vec();
        bytes.push(0xE9);
        bytes.extend_from_slice(" world".as_bytes());
        let text = decode_body(bytes).expect("少量非法字节应降级而非报错");
        assert!(text.contains("hello"));
        assert!(text.contains("world"));
    }

    #[test]
    fn test_decode_body_binary_rejected() {
        // 高比例非法字节（二进制响应）→ 仍报错
        assert!(decode_body(vec![0xFFu8; 64]).is_err());
    }

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
    async fn test_cache_evicts_oldest_when_full() {
        // T2：缓存此前无上限（长驻进程内随访问条目单调增长，仅 TTL 回收）。
        let client = WebSearchClient::new(&test_config()).unwrap();
        for i in 0..(MAX_CACHE_ENTRIES + 10) {
            client.cache_put(format!("k{i}"), format!("v{i}"));
        }
        assert!(
            client.cache.len() <= MAX_CACHE_ENTRIES,
            "缓存应受上限约束（实际 {}）",
            client.cache.len()
        );
        // 早期写入的键应已被淘汰（TTL 内本应命中）
        assert!(
            client.cache_get("k0").is_none() || client.cache_get("k1").is_none(),
            "满员时应淘汰最旧条目"
        );
    }

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

    /// T1-22 端到端回归：响应超限被截断、且截断点落在多字节字符中间时，
    /// 整个 fetch 不应失败（旧行为：`String::from_utf8` 直接报错）。
    #[tokio::test]
    async fn test_fetch_tolerates_truncation_mid_multibyte_char() {
        // 6000 字节中文响应；上限 1024 → 1024 % 3 != 0（切在「中」的中间）
        let body: &'static str = Box::leak("中".repeat(2000).into_boxed_str());
        let endpoint = spawn_mock_searxng(body).await;

        let mut config = test_config();
        config.max_fetch_bytes = 1024;
        let client = WebSearchClient::new(&config).unwrap();

        let text = client
            .fetch(&endpoint, None)
            .await
            .expect("截断落在多字节字符中间不应使 fetch 失败");
        assert!(!text.is_empty(), "应保留截断前的可读内容");
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

    /// DeepSeek 后端端到端（mock 端点）：POST JSON → 解析 → 缓存。
    #[tokio::test]
    async fn test_search_via_deepseek_backend() {
        let endpoint = spawn_mock_searxng(
            r#"{"results":[{"title":"DeepSeek 搜索","url":"https://example.com/a","snippet":"片段","summary":"官方摘要"}]}"#,
        )
        .await;
        let mut config = test_config();
        config.search_backend = "deepseek".to_string();
        config.deepseek_api_key = Some("test-key".to_string());
        config.deepseek_endpoint = Some(endpoint);

        let client = WebSearchClient::new(&config).unwrap();
        let results = client.search("deepseek", 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "DeepSeek 搜索");
        assert_eq!(results[0].snippet, "官方摘要");

        // 缓存命中：第二次搜索走缓存
        let again = client.search("deepseek", 5).await.unwrap();
        assert_eq!(again[0].title, "DeepSeek 搜索");
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

    #[tokio::test]
    async fn test_deepseek_backend_requires_key() {
        let mut config = test_config();
        config.search_backend = "deepseek".to_string();
        let err = WebSearchClient::new(&config).unwrap_err();
        assert!(err.to_string().contains("deepseek_api_key"), "{err}");
    }

    /// Bing 后端已移除（对自动化流量返回降级结果）：配置残留时给出明确迁移提示。
    #[tokio::test]
    async fn test_bing_backend_removed() {
        let mut config = test_config();
        config.search_backend = "bing".to_string();
        let err = WebSearchClient::new(&config).unwrap_err();
        assert!(err.to_string().contains("已移除"), "{err}");
    }

    // ── T0-3：重定向目标的 SSRF 校验 ────────────────────────────────

    /// 起一个本地 mock HTTP server，对首个请求返回 302（Location 由参数给定）。
    async fn spawn_mock_redirect(location: String) -> String {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{addr}")
    }

    /// T0-3 主回归：初始 URL 校验通过后，公网 URL 可 302 到私网绕过
    /// SSRF 防护——重定向目标必须在每一跳发出前校验（修复前 reqwest
    /// 默认静默跟随，第二跳会被实际请求）。
    #[tokio::test]
    async fn test_fetch_rejects_redirect_to_private_address() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // 第二跳 mock：记录是否被访问——修复后（策略拦截）不得被访问。
        let hit = Arc::new(AtomicBool::new(false));
        let hit_in = hit.clone();
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = target.accept().await {
                hit_in.store(true, Ordering::SeqCst);
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret",
                    )
                    .await;
            }
        });

        let endpoint =
            spawn_mock_redirect(format!("http://127.0.0.1:{}/secret", target_addr.port())).await;
        let client = WebSearchClient::new(&test_config()).unwrap();

        let err = client
            .fetch(&format!("{endpoint}/redirect"), None)
            .await
            .expect_err("重定向到内网必须失败");

        // 关键断言：内网第二跳不得被实际请求（策略在发出前拦截）
        assert!(
            !hit.load(Ordering::SeqCst),
            "重定向到内网不得实际发出请求（SSRF 绕过）: {err}"
        );
        assert!(
            err.to_string().contains("following redirect"),
            "应被重定向策略拦下（而非尝试连接内网）: {err}"
        );
    }

    /// 回归：重定向到公网字面地址不被策略误伤（策略只拦私网/本地）。
    ///
    /// 目标用 `.invalid` 保留域名（RFC 2606 保证永不解析）——策略按 URL
    /// 字面放行，请求实际发出后 DNS 失败；断言“错误消息不是策略拦截”
    /// 即可区分（且不触达真实外网，不依赖网络环境）。
    #[tokio::test]
    async fn test_fetch_redirect_to_public_not_blocked_by_policy() {
        let endpoint = spawn_mock_redirect("http://redirect-target.invalid/next".to_string()).await;
        let client = WebSearchClient::new(&test_config()).unwrap();

        let err = client
            .fetch(&format!("{endpoint}/redirect"), None)
            .await
            .expect_err("目标域名不可解析，请求必失败");
        let msg = err.to_string();
        assert!(
            !msg.contains("禁止访问本地或内网地址"),
            "公网重定向不应被策略拦截（应为网络层失败）: {msg}"
        );
    }
}
