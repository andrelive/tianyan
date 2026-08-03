//! MCP client implementation using `rust-mcp-sdk`.
//!
//! Provides [`McpClient`] which wraps a `ClientRuntime` connection
//! to a single MCP server over stdio transport.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rust_mcp_sdk::mcp_client::client_runtime;
use rust_mcp_sdk::mcp_client::{
    ClientHandler, ClientRuntime, McpClientOptions, ToMcpClientHandler,
};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, Implementation, InitializeRequestParams,
};
use rust_mcp_sdk::{McpClient as McpClientTrait, StdioTransport, TransportOptions};
use tracing::instrument;

use crate::error::{McpError, McpResult};

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
            timeout: std::time::Duration::from_secs(30),
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
        let client_info = Implementation {
            name: "tianyan-mcp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: Some("Tianyan MCP Client".to_string()),
            icons: vec![],
            title: None,
            website_url: None,
        };

        let client_details = InitializeRequestParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info,
            meta: None,
        };

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

    /// Returns the server name.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Returns `true` if the client is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Returns the cached list of available tools.
    ///
    /// To refresh the list from the server, use [`Self::refresh_tools`].
    pub async fn list_tools(&self) -> McpResult<Vec<ToolInfo>> {
        if !self.is_connected() {
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
    #[instrument(skip(arguments), fields(server = %self.server_name, tool = %tool_name))]
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> McpResult<String> {
        if !self.is_connected() {
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

        // Extract text content from the result.
        let text = Self::extract_text_content(&result);
        Ok(text)
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

    /// Fetch the tool list from the server.
    async fn fetch_tools(client: &ClientRuntime) -> McpResult<Vec<ToolInfo>> {
        let result = client
            .request_tool_list(None::<rust_mcp_sdk::schema::PaginatedRequestParams>)
            .await
            .map_err(|e| McpError::ProtocolViolation(format!("Failed to list tools: {e}")))?;

        Ok(result.tools.into_iter().map(ToolInfo::from).collect())
    }

    /// Extract text content from a `CallToolResult`.
    fn extract_text_content(result: &rust_mcp_sdk::schema::CallToolResult) -> String {
        use rust_mcp_sdk::schema::ContentBlock;

        let mut parts: Vec<String> = Vec::new();
        for block in &result.content {
            match block {
                ContentBlock::TextContent(text) => parts.push(text.text.clone()),
                ContentBlock::ImageContent(img) => {
                    parts.push(format!("[Image: {} ({})]", img.mime_type, img.data.len()));
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
        parts.join("\n")
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
}
