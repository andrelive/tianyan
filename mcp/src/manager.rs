//! Manages multiple MCP client connections.
//!
//! [`McpClientManager`] provides a registry of named [`McpClient`] instances,
//! allowing callers to connect, disconnect, and interact with multiple MCP
//! servers through a single interface.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::client::McpClient;
use crate::error::{McpError, McpResult};
use crate::ToolInfo;

/// Manages a collection of named MCP server connections.
///
/// Each server is identified by a unique name. Thread-safe: cloning shares
/// the underlying connection map via `Arc`.
#[derive(Debug, Clone)]
pub struct McpClientManager {
    /// Map of server name to connected client.
    clients: Arc<RwLock<HashMap<String, Arc<McpClient>>>>,
}

impl Default for McpClientManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpClientManager {
    /// Create a new empty manager.
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to a new MCP server and register it.
    ///
    /// If a server with the same name already exists, it will be disconnected
    /// and replaced.
    pub async fn connect_server(
        &mut self,
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
    ) -> McpResult<()> {
        let name: String = name.into();

        // Disconnect existing connection with this name, if any.
        if self.is_connected(&name).await {
            self.disconnect_server(&name).await?;
        }

        let client = McpClient::connect(name.clone(), command, args, env).await?;
        let client = Arc::new(client);

        let mut guard = self.clients.write().await;
        guard.insert(name.clone(), client);

        tracing::info!(server = %name, "MCP server registered in manager");
        Ok(())
    }

    /// Disconnect and remove a server by name.
    pub async fn disconnect_server(&mut self, name: &str) -> McpResult<()> {
        let mut guard = self.clients.write().await;
        if let Some(client) = guard.remove(name) {
            drop(guard);
            // Actually disconnect the client (shuts down subprocess)
            let _ = client.disconnect().await;
            tracing::info!(server = %name, "MCP server unregistered from manager");
            Ok(())
        } else {
            Err(McpError::ConnectionFailed(
                name.to_string(),
                "Server not found in manager".to_string(),
            ))
        }
    }

    /// Get a reference to a connected client by server name.
    pub async fn get_client(&self, name: &str) -> Option<Arc<McpClient>> {
        let guard = self.clients.read().await;
        guard.get(name).cloned()
    }

    /// Call a tool on a named server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> McpResult<String> {
        let client = self.get_client(server_name).await.ok_or_else(|| {
            McpError::ConnectionFailed(server_name.to_string(), "Server not connected".to_string())
        })?;

        client.call_tool(tool_name, arguments).await
    }

    /// List tools available on a named server.
    pub async fn list_tools(&self, server_name: &str) -> McpResult<Vec<ToolInfo>> {
        let client = self.get_client(server_name).await.ok_or_else(|| {
            McpError::ConnectionFailed(server_name.to_string(), "Server not connected".to_string())
        })?;

        client.list_tools().await
    }

    /// Check if a server is connected.
    pub async fn is_connected(&self, name: &str) -> bool {
        let guard = self.clients.read().await;
        guard.contains_key(name)
    }

    /// Returns the number of connected servers.
    pub async fn connected_count(&self) -> usize {
        let guard = self.clients.read().await;
        guard.len()
    }

    /// Returns a list of all connected server names.
    pub async fn connected_servers(&self) -> Vec<String> {
        let guard = self.clients.read().await;
        guard.keys().cloned().collect()
    }

    /// Disconnect all servers.
    pub async fn disconnect_all(&mut self) {
        let names: Vec<String> = {
            let guard = self.clients.read().await;
            guard.keys().cloned().collect()
        };

        for name in &names {
            let _ = self.disconnect_server(name).await;
        }

        tracing::info!("All MCP servers disconnected");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_new_is_empty() {
        let manager = McpClientManager::new();
        assert_eq!(futures::executor::block_on(manager.connected_count()), 0);
    }

    #[test]
    fn test_manager_default_is_empty() {
        let manager = McpClientManager::default();
        assert_eq!(futures::executor::block_on(manager.connected_count()), 0);
    }

    #[test]
    fn test_is_connected_returns_false_for_unknown() {
        let manager = McpClientManager::new();
        let connected = futures::executor::block_on(manager.is_connected("nonexistent"));
        assert!(!connected);
    }

    #[test]
    fn test_disconnect_unknown_server_returns_error() {
        let mut manager = McpClientManager::new();
        let result = futures::executor::block_on(manager.disconnect_server("ghost"));
        assert!(result.is_err());
    }

    #[test]
    fn test_connected_servers_empty() {
        let manager = McpClientManager::new();
        let servers = futures::executor::block_on(manager.connected_servers());
        assert!(servers.is_empty());
    }
}
