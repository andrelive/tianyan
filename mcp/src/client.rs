//! MCP client implementation using `rust-mcp-sdk`.
//!
//! Provides [`McpClient`] which wraps a `ClientRuntime` connection
//! to a single MCP server over stdio transport.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rust_mcp_sdk::mcp_client::client_runtime;
use rust_mcp_sdk::mcp_client::{
    ClientHandler, ClientRuntime, McpClientOptions, ToMcpClientHandler,
};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, Implementation, InitializeRequestParams,
};
use rust_mcp_sdk::{
    McpClient as McpClientTrait, RequestOptions, StdioTransport, StreamableTransportOptions,
    TransportOptions,
};
use tracing::instrument;

use crate::error::{McpError, McpResult};
use crate::types::McpServerConfig;

/// Information about a tool provided by an MCP server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    /// The name of the tool.
    pub name: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema defining the tool's input parameters.
    pub input_schema: serde_json::Value,
}

impl From<rust_mcp_sdk::schema::Tool> for ToolInfo {
    fn from(tool: rust_mcp_sdk::schema::Tool) -> Self {
        Self {
            name: tool.name,
            description: tool.description.or(tool.title),
            input_schema: serde_json::to_value(tool.input_schema)
                .unwrap_or(serde_json::Value::Null),
        }
    }
}

/// MCP 工具调用结果中的图片内容块。
///
/// 数据为 base64 编码（不含 `data:` 前缀），与 MCP 协议 `type: "image"`
/// content block 一致；mime_type 如 `image/png`。
#[derive(Debug, Clone)]
pub struct McpImage {
    /// Base64 编码的图片数据。
    pub data: String,
    /// 图片 MIME 类型，如 `image/png`。
    pub mime_type: String,
}

/// MCP 工具调用的完整结构化输出：文本拼接 + 图片列表。
///
/// `text` 保留兼容的 `[Image: <mime> (<bytes>)]` 占位符（供无落盘能力时
/// 降级展示）；`images` 携带完整 base64 数据，供上层（如服务器桥接层）
/// 保存为文件后把路径暴露给 LLM。
#[derive(Debug, Clone)]
pub struct McpCallOutput {
    /// 文本内容块拼接结果。
    pub text: String,
    /// 图片内容块列表（数据完整保留，不再丢弃）。
    pub images: Vec<McpImage>,
}

/// A simple MCP client handler that logs events to tracing.
#[derive(Debug, Clone)]
struct LoggingClientHandler;

impl ClientHandler for LoggingClientHandler {}

/// A client that manages a single MCP server connection over stdio.
///
/// # Example
///
/// ```no_run
/// use tianyan_mcp::McpClient;
///
/// # async fn example() {
/// let mut client = McpClient::connect(
///     "my-server",
///     "npx",
///     vec!["-y".to_string(), "@modelcontextprotocol/server-everything".to_string()],
///     None,
/// )
/// .await
/// .unwrap();
///
/// let tools = client.list_tools().await.unwrap();
/// for tool in &tools {
///     println!("  - {}", tool.name);
/// }
/// # }
/// ```
pub struct McpClient {
    /// Human-readable server name identifier.
    server_name: String,
    /// The underlying SDK client runtime.
    client: Arc<ClientRuntime>,
    /// Cached list of tools from the server.
    tools: Vec<ToolInfo>,
    /// Whether the client is currently connected.
    connected: AtomicBool,
}

impl McpClient {
    /// Connect to an MCP server by spawning it as a subprocess.
    ///
    /// # Arguments
    ///
    /// * `server_name` - A human-readable label for this server.
    /// * `command` - The executable path or name (e.g., `"npx"`, `"node"`).
    /// * `args` - Arguments passed to the command.
    /// * `env` - Optional extra environment variables for the subprocess.
    pub async fn connect(
        server_name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
    ) -> McpResult<Self> {
        let server_name: String = server_name.into();
        let command: String = command.into();

        tracing::info!(server = %server_name, "Connecting to MCP server");

        // Build the stdio transport that spawns the server subprocess.
        let transport_options = TransportOptions {
            timeout: Duration::from_secs(30),
            ..Default::default()
        };

        let transport =
            StdioTransport::create_with_server_launch(&command, args, env, transport_options)
                .map_err(|e| {
                    McpError::connection_failed(
                        &server_name,
                        format!("Failed to create transport: {e}"),
                    )
                })?;

        // Prepare the initialize request with client metadata.
        let client_details = Self::build_client_details();

        // Create the client runtime with our handler.
        let client = client_runtime::create_client(McpClientOptions {
            client_details,
            transport,
            handler: Box::new(LoggingClientHandler).to_mcp_client_handler(),
            task_store: None,
            server_task_store: None,
            message_observer: None,
        });

        // Start the client (initializes the connection).
        client.clone().start().await.map_err(|e| {
            McpError::connection_failed(&server_name, format!("Failed to start client: {e}"))
        })?;

        // List tools on connect to populate the cache.
        let tools = Self::fetch_tools(&client).await?;

        tracing::info!(
            server = %server_name,
            tool_count = tools.len(),
            "Connected to MCP server"
        );

        Ok(Self {
            server_name,
            client,
            tools,
            connected: AtomicBool::new(true),
        })
    }

    /// Connect to a remote MCP server over streamable HTTP transport（G7）。
    ///
    /// 对齐 2026-07-28 stateless 核心规范（SDK `with_transport_options`）；
    /// 支持远程 MCP 服务器（GitHub MCP / 云服务 MCP 等生态主流形态）。
    /// `url` 必须是 `http://` 或 `https://` 绝对地址（如
    /// `https://mcp.example.com/mcp`）。
    ///
    /// # Arguments
    /// * `server_name` - A human-readable label for this server.
    /// * `url` - The streamable HTTP endpoint of the remote MCP server.
    pub async fn connect_http(
        server_name: impl Into<String>,
        url: impl Into<String>,
    ) -> McpResult<Self> {
        let server_name: String = server_name.into();
        let url: String = url.into();

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(McpError::connection_failed(
                &server_name,
                format!("streamable HTTP URL 必须为 http/https 绝对地址：{url}"),
            ));
        }

        tracing::info!(server = %server_name, url = %url, "Connecting to remote MCP server");

        let transport_options = StreamableTransportOptions {
            mcp_url: url,
            request_options: RequestOptions::default(),
        };

        let client = client_runtime::with_transport_options(
            Self::build_client_details(),
            transport_options,
            LoggingClientHandler,
            None,
            None,
            None,
        );

        // Start the client (initializes the connection).
        client.clone().start().await.map_err(|e| {
            McpError::connection_failed(&server_name, format!("Failed to start client: {e}"))
        })?;

        let tools = Self::fetch_tools(&client).await?;

        tracing::info!(
            server = %server_name,
            tool_count = tools.len(),
            "Connected to remote MCP server"
        );

        Ok(Self {
            server_name,
            client,
            tools,
            connected: AtomicBool::new(true),
        })
    }

    /// 按服务器配置建立连接（**传输分发唯一实现**）。
    ///
    /// - `transport = "http"`：streamable HTTP（远程服务器），`url` 必填；
    /// - 缺省 / `"stdio"`：本地子进程；
    /// - 未知 transport：告警并回退 stdio。
    ///
    /// 连接逻辑收敛于此——server 层的生命周期管理器与连接测试端点
    /// 均经此分发，不再各自复制 transport 判断。
    pub async fn connect_from_config(config: &McpServerConfig) -> McpResult<Self> {
        match config.transport.as_deref() {
            Some("http") => {
                let url = config.url.clone().ok_or_else(|| {
                    McpError::connection_failed(&config.name, "transport=http 但未配置 url")
                })?;
                McpClient::connect_http(config.name.clone(), url).await
            }
            Some(other) => {
                tracing::warn!(
                    server = %config.name,
                    transport = %other,
                    "未知 MCP 传输方式，回退 stdio"
                );
                McpClient::connect(
                    config.name.clone(),
                    config.command.clone(),
                    config.args.clone(),
                    config.env.clone(),
                )
                .await
            }
            None => {
                McpClient::connect(
                    config.name.clone(),
                    config.command.clone(),
                    config.args.clone(),
                    config.env.clone(),
                )
                .await
            }
        }
    }

    /// Returns the server name.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Returns `true` if the client is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 廉价存活判定：未显式断开 + SDK 连接已初始化 + 未 shutdown。
    ///
    /// 不做网络往返（**真探活**用 [`Self::probe`]）；用于高频路径
    /// （工具列表/工具调用）的前置检查。
    pub async fn is_alive(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
            && self.client.is_initialized()
            && !self.client.is_shut_down().await
    }

    /// **真探活**：向服务器发 MCP `ping` 请求（带超时）。
    ///
    /// 与 [`Self::is_connected`]（仅本地标志）不同，本方法产生一次真实
    /// 往返，能发现"对端子进程崩溃 / 远端断连"——服务器不可用时返回错误
    /// （T1-9：注册在册 ≠ 可用）。
    pub async fn probe(&self, timeout: Duration) -> McpResult<()> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(McpError::ConnectionFailed(
                self.server_name.clone(),
                "Client is not connected".to_string(),
            ));
        }
        self.client.ping(None, Some(timeout)).await.map_err(|e| {
            McpError::connection_failed(&self.server_name, format!("ping 探活失败：{e}"))
        })?;
        Ok(())
    }

    /// Returns the cached list of available tools.
    ///
    /// To refresh the list from the server, use [`Self::refresh_tools`].
    pub async fn list_tools(&self) -> McpResult<Vec<ToolInfo>> {
        if !self.is_alive().await {
            return Err(McpError::ConnectionFailed(
                self.server_name.clone(),
                "Client is not connected".to_string(),
            ));
        }
        Ok(self.tools.clone())
    }

    /// Re-fetches the tool list from the server and updates the cache.
    pub async fn refresh_tools(&mut self) -> McpResult<Vec<ToolInfo>> {
        let tools = Self::fetch_tools(&self.client).await?;
        self.tools = tools.clone();
        Ok(tools)
    }

    /// Calls a tool on the server.
    ///
    /// Returns the text content of the tool result. If the tool returns
    /// multiple content blocks, they are concatenated with newlines.
    /// Images are represented by `[Image: <mime> (<bytes>)]` placeholders
    /// (their data is dropped); use [`Self::call_tool_detailed`] to receive
    /// the full image data.
    #[instrument(skip(arguments), fields(server = %self.server_name, tool = %tool_name))]
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> McpResult<String> {
        Ok(self.call_tool_detailed(tool_name, arguments).await?.text)
    }

    /// Calls a tool on the server and returns the full structured output.
    ///
    /// Unlike [`Self::call_tool`], image content blocks are preserved in
    /// [`McpCallOutput::images`] with their base64 data intact, so callers
    /// can persist them (e.g. save screenshots to disk) instead of losing
    /// them to a text placeholder.
    #[instrument(skip(arguments), fields(server = %self.server_name, tool = %tool_name))]
    pub async fn call_tool_detailed(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> McpResult<McpCallOutput> {
        if !self.is_alive().await {
            return Err(McpError::ConnectionFailed(
                self.server_name.clone(),
                "Client is not connected".to_string(),
            ));
        }

        // Validate the tool exists in the cache.
        if !self.tools.iter().any(|t| t.name == tool_name) {
            return Err(McpError::ToolNotFound(tool_name.to_string()));
        }

        let args_map = match arguments {
            serde_json::Value::Object(map) => Some(map),
            serde_json::Value::Null => None,
            _ => {
                return Err(McpError::ProtocolViolation(format!(
                    "Tool arguments must be a JSON object, got {}",
                    arguments
                )));
            }
        };

        let params =
            CallToolRequestParams::new(tool_name).with_arguments(args_map.unwrap_or_default());

        let result = self.client.request_tool_call(params).await.map_err(|e| {
            McpError::ConnectionFailed(self.server_name.clone(), format!("Tool call failed: {e}"))
        })?;

        // Extract structured content from the result (text + images).
        Ok(Self::extract_output(&result))
    }

    /// Disconnects from the server.
    pub async fn disconnect(&self) -> McpResult<()> {
        if self.connected.load(Ordering::Relaxed) {
            self.client
                .clone()
                .shut_down()
                .await
                .map_err(|e| McpError::Transport(e.to_string()))?;
            self.connected.store(false, Ordering::Relaxed);
            tracing::info!(server = %self.server_name, "Disconnected from MCP server");
        }
        Ok(())
    }

    // ─── private helpers ───

    /// 构建 MCP initialize 请求参数（stdio 与 streamable HTTP 共用）。
    fn build_client_details() -> InitializeRequestParams {
        let client_info = Implementation {
            name: "tianyan-mcp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: Some("Tianyan MCP Client".to_string()),
            icons: vec![],
            title: None,
            website_url: None,
        };

        InitializeRequestParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info,
            meta: None,
        }
    }

    /// Fetch the tool list from the server.
    async fn fetch_tools(client: &ClientRuntime) -> McpResult<Vec<ToolInfo>> {
        let result = client
            .request_tool_list(None::<rust_mcp_sdk::schema::PaginatedRequestParams>)
            .await
            .map_err(|e| McpError::ProtocolViolation(format!("Failed to list tools: {e}")))?;

        Ok(result.tools.into_iter().map(ToolInfo::from).collect())
    }

    /// Extract structured content (text + images) from a `CallToolResult`.
    fn extract_output(result: &rust_mcp_sdk::schema::CallToolResult) -> McpCallOutput {
        use rust_mcp_sdk::schema::ContentBlock;

        let mut parts: Vec<String> = Vec::new();
        let mut images: Vec<McpImage> = Vec::new();
        for block in &result.content {
            match block {
                ContentBlock::TextContent(text) => parts.push(text.text.clone()),
                ContentBlock::ImageContent(img) => {
                    // 占位符保留在文本中（无落盘能力时的降级展示）；
                    // 完整 base64 数据进入 images 列表，供上层保存。
                    parts.push(format!("[Image: {} ({})]", img.mime_type, img.data.len()));
                    images.push(McpImage {
                        data: img.data.clone(),
                        mime_type: img.mime_type.clone(),
                    });
                }
                ContentBlock::AudioContent(audio) => {
                    parts.push(format!(
                        "[Audio: {} ({})]",
                        audio.mime_type,
                        audio.data.len()
                    ));
                }
                ContentBlock::ResourceLink(link) => {
                    parts.push(format!("[Resource link: {}]", link.uri));
                }
                ContentBlock::EmbeddedResource(resource) => {
                    parts.push("[Embedded resource]".to_string());
                    let _ = resource;
                }
            }
        }
        McpCallOutput {
            text: parts.join("\n"),
            images,
        }
    }
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server_name", &self.server_name)
            .field("connected", &self.connected)
            .field("tool_count", &self.tools.len())
            .finish()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if self.connected.load(Ordering::Relaxed) {
            tracing::warn!(
                server = %self.server_name,
                "McpClient dropped while still connected; call disconnect() explicitly"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_info_from_sdk_tool() {
        let sdk_tool = rust_mcp_sdk::schema::Tool {
            name: "test_tool".to_string(),
            description: Some("A test tool".to_string()),
            input_schema: rust_mcp_sdk::schema::ToolInputSchema::new(
                vec!["param1".to_string()],
                None,
                None,
            ),
            annotations: None,
            execution: None,
            icons: vec![],
            meta: None,
            output_schema: None,
            title: None,
        };

        let info = ToolInfo::from(sdk_tool);
        assert_eq!(info.name, "test_tool");
        assert_eq!(info.description, Some("A test tool".to_string()));
        assert!(info.input_schema.is_object());
    }

    #[test]
    fn test_tool_info_serialization() {
        let info = ToolInfo {
            name: "my_tool".to_string(),
            description: Some("Does something".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            }),
        };

        let json = match serde_json::to_value(&info) {
            Ok(v) => v,
            Err(e) => panic!("ToolInfo 序列化失败: {e}"),
        };
        assert_eq!(json["name"], "my_tool");
        assert_eq!(json["description"], "Does something");
        assert_eq!(json["input_schema"]["type"], "object");
    }

    #[test]
    fn test_mcp_error_connection_failed() {
        let err = McpError::connection_failed("test-server", "timeout");
        let msg = err.to_string();
        assert!(msg.contains("test-server"));
        assert!(msg.contains("timeout"));
        assert!(msg.contains("Connection failed"));
    }

    #[test]
    fn test_mcp_error_tool_not_found() {
        let err = McpError::ToolNotFound("missing_tool".to_string());
        assert_eq!(err.to_string(), "Tool not found: missing_tool");
    }

    #[test]
    fn test_mcp_error_server_crashed() {
        let err = McpError::server_crashed("my-server", "exit code 1");
        let msg = err.to_string();
        assert!(msg.contains("my-server"));
        assert!(msg.contains("exit code 1"));
    }

    #[test]
    fn test_mcp_error_transport() {
        let err = McpError::Transport("io error".to_string());
        assert_eq!(err.to_string(), "Transport error: io error");
    }

    #[tokio::test]
    async fn test_connect_http_rejects_non_http_url() {
        // G7：非 http/https URL 在发起连接前即被拒绝（不触网）
        let result = McpClient::connect_http("remote", "ftp://example.com/mcp").await;
        let Err(err) = result else {
            panic!("非 http URL 应被拒绝，实际成功");
        };
        assert!(
            err.to_string().contains("http"),
            "应提示 URL 必须为 http/https: {err}"
        );
    }

    /// 构造一个只含文本块的 `CallToolResult`。
    fn result_with_text(text: &str) -> rust_mcp_sdk::schema::CallToolResult {
        use rust_mcp_sdk::schema::{CallToolResult, ContentBlock, TextContent};
        CallToolResult {
            content: vec![ContentBlock::from(TextContent::new(
                text.to_string(),
                None,
                None,
            ))],
            is_error: None,
            meta: None,
            structured_content: None,
        }
    }

    #[test]
    fn test_extract_output_text_only() {
        let result = result_with_text("hello world");
        let output = McpClient::extract_output(&result);
        assert_eq!(output.text, "hello world");
        assert!(output.images.is_empty(), "纯文本结果不应有图片");
    }

    #[test]
    fn test_extract_output_with_image_preserves_data() {
        use rust_mcp_sdk::schema::{CallToolResult, ContentBlock, ImageContent, TextContent};

        let base64_data = "iVBORw0KGgoAAAANSUhEUg==";
        let result = CallToolResult {
            content: vec![
                ContentBlock::from(TextContent::new("screenshot taken".to_string(), None, None)),
                ContentBlock::from(ImageContent::new(
                    base64_data.to_string(),
                    "image/png".to_string(),
                    None,
                    None,
                )),
            ],
            is_error: None,
            meta: None,
            structured_content: None,
        };

        let output = McpClient::extract_output(&result);

        // 文本占位符保留（无落盘能力时的降级展示）
        assert!(output.text.contains("screenshot taken"));
        assert!(output.text.contains("[Image: image/png ("));

        // 完整 base64 数据不再丢弃
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].data, base64_data);
        assert_eq!(output.images[0].mime_type, "image/png");
    }

    #[test]
    fn test_extract_output_multiple_images() {
        use rust_mcp_sdk::schema::{CallToolResult, ContentBlock, ImageContent};

        let result = CallToolResult {
            content: vec![
                ContentBlock::from(ImageContent::new(
                    "AAAA".to_string(),
                    "image/png".to_string(),
                    None,
                    None,
                )),
                ContentBlock::from(ImageContent::new(
                    "BBBB".to_string(),
                    "image/jpeg".to_string(),
                    None,
                    None,
                )),
            ],
            is_error: None,
            meta: None,
            structured_content: None,
        };

        let output = McpClient::extract_output(&result);
        assert_eq!(output.images.len(), 2);
        assert_eq!(output.images[0].data, "AAAA");
        assert_eq!(output.images[1].mime_type, "image/jpeg");
        // 文本中按顺序出现两个占位符
        assert!(output.text.contains("image/png"));
        assert!(output.text.contains("image/jpeg"));
    }
}
