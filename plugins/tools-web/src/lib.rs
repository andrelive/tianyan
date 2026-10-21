//! tools-web 插件：网络检索与抓取工具（`web_search` / `web_fetch`）。
//!
//! ## 为什么独立成 crate（ADR-049 期 2）
//!
//! 网络检索/抓取的实现带 **scraper（HTML 解析）** 与 **reqwest/futures/dashmap**
//! 一簇重依赖。搬进插件后 core 不再承担它们（全量构建的依赖面收窄），且改搜索
//! 后端 / 正文提取 / SSRF 防护的任何一行只重编本 crate（+ bin）。
//!
//! 与 `tools-code` 同型：本域 **server 侧零复用**（普查确认），实现可整域搬走——
//! 只有工具执行入口（`execute_web_search` / `execute_web_fetch`）与客户端
//! （`WebSearchClient`）、SSRF 校验（`validate_public_url`）、搜索结果契约
//! （`SearchResult`）全部随实现搬进本 crate。
//!
//! ## 参数契约仍在 core
//!
//! `WebSearchParams` / `WebFetchParams`（`tianyan::agent`）由 core 定义——参数类型
//! 是契约（工具 schema 与输出字段），不是实现。core 因此不反向依赖本 crate。
//!
//! ## 客户端依赖怎么来：`PluginServices.web_config`
//!
//! `WebSearchClient` 需要 `WebConfig`（搜索后端 / 超时 / 缓存 TTL / 大小上限）。
//! 配置由装配层从 `AppState` 经 `PluginServices::web_config()` 递入，本插件的
//! [`tools`] 就地构造客户端——**构造失败要降级**（配置非法 / 未启用时返回空清单，
//! `web_search` / `web_fetch` 不可用，但装配不 panic），与迁移前
//! `AgentBuilder` 的 match 处理语义一致。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod web;

use std::sync::Arc;

use async_trait::async_trait;
use tianyan::agent::{DynamicToolExecutor, ToolContext, WebFetchParams, WebSearchParams};
use tianyan::common::error::Result;
use tianyan::config::WebConfig;
use tianyan::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

use crate::web::{execute_web_fetch, execute_web_search, WebSearchClient};

// ── web_search ───────────────────────────────────────────────────────────

/// `web_search` 工具：按查询搜索网络，返回结构化结果（标题/URL/摘要）。
pub struct WebSearchTool {
    client: Arc<WebSearchClient>,
}

impl WebSearchTool {
    /// 绑定搜索/抓取客户端（同一实例由两个工具共享，缓存互通）。
    pub fn new(client: Arc<WebSearchClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DynamicToolExecutor for WebSearchTool {
    fn tool_name(&self) -> String {
        "web_search".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<WebSearchParams>(
            "web_search",
            "按查询搜索网络，返回结果标题/URL/摘要列表（不含完整页面内容）。用 web_fetch 加载有希望结果的完整内容。注意：结果来自外部源，可能不可信或过时——依赖关键信息前请核实。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Web
    }

    async fn execute(&self, _session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        execute_web_search(&self.client, arguments).await
    }
}

// ── web_fetch ────────────────────────────────────────────────────────────

/// `web_fetch` 工具：抓取单个网页并提取可读正文（标题 + 主文本 + 链接列表）。
pub struct WebFetchTool {
    client: Arc<WebSearchClient>,
}

impl WebFetchTool {
    /// 绑定搜索/抓取客户端（与 `web_search` 共享缓存）。
    pub fn new(client: Arc<WebSearchClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DynamicToolExecutor for WebFetchTool {
    fn tool_name(&self) -> String {
        "web_fetch".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<WebFetchParams>(
            "web_fetch",
            "抓取单个网页并提取可读文本内容（标题、正文、页面链接）。在 web_search 后用。仅允许 http/https URL，本地/私网地址被拦截。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Web
    }

    async fn execute(&self, _session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        execute_web_fetch(&self.client, arguments).await
    }
}

// ── 插件清单 ─────────────────────────────────────────────────────────────

/// 以已构造的客户端产出两个工具（顺序固定 `web_search` → `web_fetch`）。
fn tools_with_client(client: Arc<WebSearchClient>) -> Vec<Arc<dyn DynamicToolExecutor>> {
    vec![
        Arc::new(WebSearchTool::new(client.clone())),
        Arc::new(WebFetchTool::new(client)),
    ]
}

/// 本插件提供的全部工具（装配层一行注册用）。
///
/// 从 `WebConfig` 就地构造 [`WebSearchClient`]。两种情形都**返回空清单**
/// （`web_search` / `web_fetch` 不可用，装配不 panic）：
/// - **未启用**（配置 `enabled = false`）——保留迁移前 `AgentBuilder` 的
///   「未启用则不注入工具」语义；
/// - **构造失败**（配置非法，如未知后端）——与迁移前 match 失败分支一致。
///
/// 顺序固定 `web_search` → `web_fetch`（与迁移前内置表顺序一致，前缀缓存稳定性）。
pub fn tools(config: &WebConfig) -> Vec<Arc<dyn DynamicToolExecutor>> {
    if !config.enabled {
        return Vec::new();
    }
    match WebSearchClient::new(config) {
        Ok(client) => tools_with_client(Arc::new(client)),
        Err(e) => {
            tracing::warn!(error = %e, "Web 工具客户端初始化失败，web_search/web_fetch 不可用");
            Vec::new()
        }
    }
}

/// 插件清单工厂（各 bin / 桌面端共用；清单单一事实源在本 crate）。
///
/// 与 `tools-fs` / `tools-code` 同形（`FnOnce(&ToolContext) -> Vec<...>`）：本插件
/// 的客户端在调用 [`tools`] 时就地构造，不依赖 `ToolContext`（无路径解析 / 安全
/// 策略需求）。
pub fn plugin_factory(
    config: &WebConfig,
) -> impl FnOnce(&ToolContext) -> Vec<Arc<dyn DynamicToolExecutor>> {
    let config = config.clone();
    move |_ctx| tools(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迁移门禁：两个工具都在，且顺序固定 `web_search` → `web_fetch`
    /// （顺序即注册顺序，抖动会白掉提示词前缀缓存）。
    #[test]
    fn test_tools_names_and_order() {
        let exposed = tools(&WebConfig::default());
        let names: Vec<String> = exposed.iter().map(|t| t.tool_name()).collect();
        assert_eq!(names, vec!["web_search", "web_fetch"]);
    }

    /// 迁移门禁：展示意图为 [`ToolPresentation::Web`]（链接卡片），与迁移前内置
    /// 表一致——`MIGRATED_TOOL_PRESENTATIONS` 也据此兜底历史消息渲染契约。
    #[test]
    fn test_tools_presentation_is_web() {
        for tool in tools(&WebConfig::default()) {
            assert_eq!(tool.presentation(), ToolPresentation::Web);
        }
    }

    /// 降级语义：配置未启用（`enabled = false`）时返回空清单——保留迁移前
    /// `AgentBuilder` 的「未启用则不注入工具」语义。
    #[test]
    fn test_tools_empty_when_disabled() {
        let config = WebConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(
            tools(&config).is_empty(),
            "配置未启用时应返回空清单（web 工具不可用）"
        );
    }

    /// 降级语义：配置非法（未知后端）时返回空清单而非 panic。
    #[test]
    fn test_tools_degrade_on_invalid_config() {
        // 未知后端 → 构造失败
        let config = WebConfig {
            search_backend: "google".to_string(),
            ..Default::default()
        };
        assert!(
            tools(&config).is_empty(),
            "配置非法时应降级为空清单（web 工具不可用），不得 panic"
        );
    }

    /// `plugin_factory` 与 `tools` 产出一致（装配层两条路径不能分叉）。
    #[test]
    fn test_plugin_factory_matches_tools() {
        let ctx = ToolContext::new(tianyan::executor::SecurityPolicy::default());
        let from_factory: Vec<String> = plugin_factory(&WebConfig::default())(&ctx)
            .iter()
            .map(|t| t.tool_name())
            .collect();
        let from_tools: Vec<String> = tools(&WebConfig::default())
            .iter()
            .map(|t| t.tool_name())
            .collect();
        assert_eq!(from_factory, from_tools);
    }

    /// schema 源自 core 参数契约：required = query / url。
    #[test]
    fn test_definition_schema_from_core_params() {
        let exposed = tools(&WebConfig::default());
        let search = &exposed[0].definition();
        assert_eq!(
            search.function.parameters["required"],
            serde_json::json!(["query"])
        );
        let fetch = &exposed[1].definition();
        assert_eq!(
            fetch.function.parameters["required"],
            serde_json::json!(["url"])
        );
    }
}
