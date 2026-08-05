//! MCP 工具桥接：把配置的 MCP 服务器工具注册进 Agent 工具注册表。
//!
//! 架构（ADR-003 组件工具化）：server 层同时依赖 core 与 mcp crate，
//! 将每个 MCP 工具适配为 core 的 [`DynamicToolExecutor`]，使配置的 MCP
//! 工具对 LLM 可见并可执行。此前 MCP 服务器仅可配置与测试连接，
//! 其工具从未进入智能体工具循环。
//!
//! 生命周期：MCP 客户端跨 agent reload 保持连接一致（[`McpToolManager::sync`]），
//! 配置热更新时断开已移除的服务器、连接新增的；连接失败仅告警（fail fast，不重试）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::{Result, TianyanError};
use tianyan::config::McpServerEntry;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan_mcp::{McpClient, ToolInfo};

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
pub struct McpToolBridge {
    server_name: String,
    tool: ToolInfo,
    client: Arc<McpClient>,
}

impl McpToolBridge {
    /// 创建工具桥接。
    pub fn new(server_name: impl Into<String>, tool: ToolInfo, client: Arc<McpClient>) -> Self {
        Self {
            server_name: server_name.into(),
            tool,
            client,
        }
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
        match self.client.call_tool(&self.tool.name, args).await {
            Ok(text) => Ok(serde_json::Value::String(text)),
            Err(e) => Err(TianyanError::Custom(format!(
                "tool: MCP {}:{} 执行失败：{}",
                self.server_name, self.tool.name, e
            ))),
        }
    }
}

/// MCP 客户端生命周期管理器：跨 agent reload 保持连接与配置一致。
pub struct McpToolManager {
    clients: Mutex<HashMap<String, Arc<McpClient>>>,
}

impl Default for McpToolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpToolManager {
    /// 创建管理器。
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// 与配置对齐：断开已移除/禁用的服务器，连接新增/启用的。
    ///
    /// 连接失败仅告警并跳过（fail fast，不重试）。
    pub async fn sync(&self, servers: &[McpServerEntry]) {
        let mut clients = self.clients.lock().await;
        let enabled: Vec<&str> = servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.name.as_str())
            .collect();

        // 断开配置中已不存在或已禁用的服务器
        let stale: Vec<String> = clients
            .keys()
            .filter(|k| !enabled.contains(&k.as_str()))
            .cloned()
            .collect();
        for name in stale {
            tracing::info!(server = %name, "MCP 服务器已从配置移除，断开连接");
            if let Some(client) = clients.remove(&name) {
                if let Err(e) = client.disconnect().await {
                    tracing::warn!(server = %name, error = %e, "MCP 断开失败");
                }
            }
        }

        // 连接新增/启用的服务器
        for server in servers.iter().filter(|s| s.enabled) {
            if clients.contains_key(&server.name) {
                continue;
            }
            match McpClient::connect(
                server.name.clone(),
                server.command.clone(),
                server.args.clone(),
                server.env.clone(),
            )
            .await
            {
                Ok(client) => {
                    let tool_count = client.list_tools().await.map(|t| t.len()).unwrap_or(0);
                    tracing::info!(
                        server = %server.name,
                        tools = tool_count,
                        "MCP 服务器已连接"
                    );
                    clients.insert(server.name.clone(), Arc::new(client));
                }
                Err(e) => {
                    tracing::warn!(
                        server = %server.name,
                        error = %e,
                        "MCP 服务器连接失败，跳过其工具"
                    );
                }
            }
        }
    }

    /// 从当前连接的服务器生成全部工具桥接。
    pub async fn bridges(&self) -> Vec<Arc<dyn DynamicToolExecutor>> {
        let clients = self.clients.lock().await;
        let mut bridges: Vec<Arc<dyn DynamicToolExecutor>> = Vec::new();
        for (name, client) in clients.iter() {
            match client.list_tools().await {
                Ok(tools) => {
                    for tool in tools {
                        bridges.push(Arc::new(McpToolBridge::new(
                            name.clone(),
                            tool,
                            client.clone(),
                        )));
                    }
                }
                Err(e) => tracing::warn!(server = %name, error = %e, "读取 MCP 工具列表失败"),
            }
        }
        bridges
    }

    /// 断开全部连接（应用关闭时调用）。
    pub async fn shutdown(&self) {
        let clients = std::mem::take(&mut *self.clients.lock().await);
        for (name, client) in clients {
            if let Err(e) = client.disconnect().await {
                tracing::warn!(server = %name, error = %e, "MCP 断开失败");
            }
        }
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
}
