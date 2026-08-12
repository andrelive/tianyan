//! MCP 工具桥接：把配置的 MCP 服务器工具注册进 Agent 工具注册表。
//!
//! 架构（ADR-003 组件工具化）：server 层同时依赖 core 与 mcp crate，
//! 将每个 MCP 工具适配为 core 的 [`DynamicToolExecutor`]，使配置的 MCP
//! 工具对 LLM 可见并可执行。此前 MCP 服务器仅可配置与测试连接，
//! 其工具从未进入智能体工具循环。
//!
//! 生命周期：MCP 客户端跨 agent reload 保持连接一致（[`McpToolManager::sync`]），
//! 配置热更新时断开已移除的服务器、连接新增的；连接失败仅告警（fail fast，不重试）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::{Result, TianyanError};
use tianyan::config::McpServerEntry;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan_mcp::{McpClient, McpClientManager, McpImage, ToolInfo};

/// 构造 LLM 可见的工具描述（带 `[MCP:<服务器名>]` 来源前缀）。
fn mcp_tool_description(server_name: &str, tool: &ToolInfo) -> String {
    format!(
        "[MCP:{}] {}",
        server_name,
        tool.description.clone().unwrap_or_default()
    )
}

/// 单个 MCP 工具 → 动态工具适配器。
///
/// 工具名保持 MCP 服务器原始名称（与服务器注册表一致）；
/// 描述带来源前缀，帮助 LLM 区分工具来源。
///
/// 工具返回的图片内容块（如浏览器截图）在配置了 `image_dir` 时落盘保存，
/// 并把可见路径追加到工具结果文本，使截图对 LLM 可感知、可后续访问；
/// 未配置时保持 `[Image: <mime> (<bytes>)]` 占位符降级。
pub struct McpToolBridge {
    server_name: String,
    tool: ToolInfo,
    client: Arc<McpClient>,
    image_dir: Option<PathBuf>,
}

impl McpToolBridge {
    /// 创建工具桥接（默认不落盘图片）。
    pub fn new(server_name: impl Into<String>, tool: ToolInfo, client: Arc<McpClient>) -> Self {
        Self {
            server_name: server_name.into(),
            tool,
            client,
            image_dir: None,
        }
    }

    /// 设置图片落盘目录（消费式 builder）。
    pub fn with_image_dir(mut self, dir: PathBuf) -> Self {
        self.image_dir = Some(dir);
        self
    }
}

#[async_trait]
impl DynamicToolExecutor for McpToolBridge {
    fn tool_name(&self) -> String {
        self.tool.name.clone()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::new(
            &self.tool.name,
            mcp_tool_description(&self.server_name, &self.tool),
            self.tool.input_schema.clone(),
        ))
    }

    async fn execute(&self, arguments: &str) -> Result<serde_json::Value> {
        // 参数非 JSON 对象时按 Null 处理（MCP 协议要求对象参数）。
        let args = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
        match self.client.call_tool_detailed(&self.tool.name, args).await {
            Ok(output) => {
                let mut text = output.text;
                if let Some(dir) = &self.image_dir {
                    if !output.images.is_empty() {
                        append_image_paths(&mut text, dir, &self.server_name, &output.images);
                    }
                }
                Ok(serde_json::Value::String(text))
            }
            Err(e) => Err(TianyanError::Custom(format!(
                "tool: MCP {}:{} 执行失败：{}",
                self.server_name, self.tool.name, e
            ))),
        }
    }
}

/// MIME 类型 → 文件扩展名。
fn image_ext(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

/// 把服务器名净化成安全的文件名前缀（保留字母/数字/-/_）。
fn sanitize_prefix(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 把 MCP 返回的图片块 base64 解码后落盘到 `dir`。
///
/// 返回每个图片的结果：`Ok(绝对路径)` 或 `Err(错误描述)`。单个图片失败
/// 不阻断其余图片保存；目录不存在时自动创建（幂等）。
fn persist_images(
    dir: &Path,
    prefix: &str,
    images: &[McpImage],
) -> Vec<std::result::Result<PathBuf, String>> {
    use base64::Engine;

    if let Err(e) = std::fs::create_dir_all(dir) {
        let msg = format!("无法创建图片目录 {}: {e}", dir.display());
        return images.iter().map(|_| Err(msg.clone())).collect();
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let safe_prefix = sanitize_prefix(prefix);

    images
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let file_name = format!("{safe_prefix}_{ts}_{i}.{}", image_ext(&img.mime_type));
            let path = dir.join(&file_name);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&img.data)
                .map_err(|e| format!("图片 {} base64 解码失败: {e}", img.mime_type))?;
            std::fs::write(&path, &bytes)
                .map_err(|e| format!("图片写入失败 {}: {e}", path.display()))?;
            Ok(path)
        })
        .collect()
}

/// 把图片保存结果追加到工具结果文本，使截图路径对 LLM 可见。
fn append_image_paths(text: &mut String, dir: &Path, prefix: &str, images: &[McpImage]) {
    let results = persist_images(dir, prefix, images);
    let mut saved: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for r in results {
        match r {
            Ok(p) => saved.push(p.display().to_string()),
            Err(e) => errors.push(e),
        }
    }

    if saved.is_empty() {
        text.push_str(&format!("\n[截图保存失败: {}]", errors.join("; ")));
    } else {
        text.push_str(&format!("\n[截图已保存: {}]", saved.join(", ")));
        if !errors.is_empty() {
            text.push_str(&format!("（{} 张保存失败）", errors.len()));
        }
    }
}

/// MCP 客户端生命周期管理器：跨 agent reload 保持连接与配置一致。
///
/// 连接生命周期（注册表、连接/断开/sync 语义、transport 分发）委托给
/// [`McpClientManager`]（mcp crate 唯一实现）；本类型只保留工具桥接与
/// 图片落盘（ADR-010）。
pub struct McpToolManager {
    /// 连接注册表（委托，非自建——连接逻辑只有一份）。
    manager: McpClientManager,
    /// 图片落盘目录（浏览器截图等）；`None` 时不保存图片。
    image_dir: Option<PathBuf>,
}

impl Default for McpToolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpToolManager {
    /// 创建管理器（默认不落盘 MCP 图片）。
    pub fn new() -> Self {
        Self {
            manager: McpClientManager::new(),
            image_dir: None,
        }
    }

    /// 创建管理器并指定 MCP 图片（如浏览器截图）落盘目录。
    pub fn with_image_dir(image_dir: PathBuf) -> Self {
        Self {
            manager: McpClientManager::new(),
            image_dir: Some(image_dir),
        }
    }

    /// 与配置对齐（委托 [`McpClientManager::sync`]）：断开已移除/禁用的
    /// 服务器，连接新增/启用的。连接失败仅告警（fail fast，不重试）。
    pub async fn sync(&self, servers: &[McpServerEntry]) {
        self.manager.sync(servers).await;
    }

    /// 从当前连接的服务器生成全部工具桥接。
    pub async fn bridges(&self) -> Vec<Arc<dyn DynamicToolExecutor>> {
        let mut bridges: Vec<Arc<dyn DynamicToolExecutor>> = Vec::new();
        for name in self.manager.connected_servers().await {
            let Some(client) = self.manager.get_client(&name).await else {
                continue;
            };
            match client.list_tools().await {
                Ok(tools) => {
                    for tool in tools {
                        let mut bridge = McpToolBridge::new(name.clone(), tool, client.clone());
                        if let Some(dir) = &self.image_dir {
                            bridge = bridge.with_image_dir(dir.clone());
                        }
                        bridges.push(Arc::new(bridge));
                    }
                }
                Err(e) => tracing::warn!(server = %name, error = %e, "读取 MCP 工具列表失败"),
            }
        }
        bridges
    }

    /// 断开全部连接（应用关闭时调用）。
    pub async fn shutdown(&self) {
        self.manager.disconnect_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tianyan::config::McpServerEntry;

    fn entry(name: &str, command: &str, enabled: bool) -> McpServerEntry {
        McpServerEntry {
            name: name.to_string(),
            command: command.to_string(),
            args: vec![],
            env: None,
            enabled,
            description: None,
            transport: None,
            url: None,
        }
    }

    #[tokio::test]
    async fn test_sync_with_empty_config_disconnects_all() {
        let manager = McpToolManager::new();
        // 无配置：不应报错，bridges 为空
        manager.sync(&[]).await;
        assert!(manager.bridges().await.is_empty());
    }

    #[tokio::test]
    async fn test_sync_skips_failed_connection() {
        let manager = McpToolManager::new();
        // 空命令 → spawn 立即失败 → 告警跳过，不 panic
        manager.sync(&[entry("broken", "", true)]).await;
        assert!(
            manager.bridges().await.is_empty(),
            "连接失败的服务器不应产生桥接"
        );
    }

    #[tokio::test]
    async fn test_sync_disabled_server_skipped() {
        let manager = McpToolManager::new();
        manager.sync(&[entry("off", "some-cmd", false)]).await;
        assert!(manager.bridges().await.is_empty(), "禁用的服务器不应连接");
    }

    #[test]
    fn test_tool_description_has_server_prefix() {
        let tool = ToolInfo {
            name: "search".to_string(),
            description: Some("Search the web".to_string()),
            input_schema: serde_json::json!({ "type": "object" }),
        };
        let desc = mcp_tool_description("server-x", &tool);
        assert_eq!(desc, "[MCP:server-x] Search the web");
    }

    #[test]
    fn test_tool_description_empty_description_ok() {
        let tool = ToolInfo {
            name: "bare".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let desc = mcp_tool_description("srv", &tool);
        assert_eq!(desc, "[MCP:srv] ");
    }

    fn png_image() -> McpImage {
        // 1x1 透明 PNG 的 base64
        McpImage {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==".to_string(),
            mime_type: "image/png".to_string(),
        }
    }

    #[test]
    fn test_image_ext_known_mimes() {
        assert_eq!(image_ext("image/png"), "png");
        assert_eq!(image_ext("image/jpeg"), "jpg");
        assert_eq!(image_ext("image/webp"), "webp");
        assert_eq!(image_ext("image/svg+xml"), "svg");
    }

    #[test]
    fn test_image_ext_unknown_falls_back_to_bin() {
        assert_eq!(image_ext("application/octet-stream"), "bin");
        assert_eq!(image_ext(""), "bin");
    }

    #[test]
    fn test_persist_images_writes_decoded_files() {
        let dir = tempfile::tempdir().expect("临时目录创建失败");
        let images = vec![png_image()];

        let results = persist_images(dir.path(), "playwright", &images);

        assert_eq!(results.len(), 1);
        let path = results[0].as_ref().expect("图片应保存成功");
        assert!(path.is_file(), "图片文件应已写入磁盘");
        assert_eq!(path.extension().unwrap(), "png");
        // 内容应为解码后的 PNG 二进制（非 base64 文本）
        let bytes = std::fs::read(path).unwrap();
        assert!(!bytes.is_empty(), "解码结果不应为空");
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n", "应写入真实 PNG 魔数");
    }

    #[test]
    fn test_persist_images_creates_missing_dir() {
        let base = tempfile::tempdir().expect("临时目录创建失败");
        let nested = base.path().join("a/b/c");
        let results = persist_images(&nested, "srv", &[png_image()]);
        assert!(results[0].is_ok(), "目录不存在时应自动创建");
        assert!(results[0].as_ref().unwrap().is_file());
    }

    #[test]
    fn test_persist_images_invalid_base64_reports_error() {
        let dir = tempfile::tempdir().expect("临时目录创建失败");
        let images = vec![McpImage {
            data: "!!!not-base64!!!".to_string(),
            mime_type: "image/png".to_string(),
        }];

        let results = persist_images(dir.path(), "srv", &images);
        assert!(results[0].is_err(), "非法 base64 应报告错误");
        assert!(results[0].as_ref().unwrap_err().contains("解码失败"));
        // 目录中不应留下垃圾文件
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_persist_images_partial_failure_keeps_good_ones() {
        let dir = tempfile::tempdir().expect("临时目录创建失败");
        let images = vec![
            png_image(),
            McpImage {
                data: "###".to_string(),
                mime_type: "image/webp".to_string(),
            },
        ];

        let results = persist_images(dir.path(), "srv", &images);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok(), "合法图片应保存");
        assert!(results[1].is_err(), "非法图片应报错");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn test_append_image_paths_all_saved() {
        let dir = tempfile::tempdir().expect("临时目录创建失败");
        let mut text = "snapshot ok".to_string();
        append_image_paths(&mut text, dir.path(), "playwright", &[png_image()]);

        assert!(text.contains("snapshot ok"), "原文本应保留");
        assert!(text.contains("[截图已保存:"), "应包含保存提示");
        assert!(text.contains(".png"), "应包含文件扩展名");
        assert!(!text.contains("失败"), "全部成功时不应出现失败字样");
    }

    #[test]
    fn test_append_image_paths_all_failed() {
        let dir = tempfile::tempdir().expect("临时目录创建失败");
        let mut text = "result".to_string();
        append_image_paths(
            &mut text,
            dir.path(),
            "srv",
            &[McpImage {
                data: "broken".to_string(),
                mime_type: "image/png".to_string(),
            }],
        );

        assert!(text.contains("[截图保存失败:"), "全部失败应报告错误");
    }

    #[test]
    fn test_sanitize_prefix_replaces_unsafe_chars() {
        assert_eq!(sanitize_prefix("playwright"), "playwright");
        assert_eq!(sanitize_prefix("my server"), "my_server");
        assert_eq!(sanitize_prefix("a/b:c"), "a_b_c");
    }
}
