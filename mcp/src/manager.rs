//! Manages multiple MCP client connections.
//!
//! [`McpClientManager`] provides a registry of named [`McpClient`] instances,
//! allowing callers to connect, disconnect, and interact with multiple MCP
//! servers through a single interface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::client::McpClient;
use crate::error::{McpError, McpResult};
use crate::types::McpServerConfig;

/// 探活超时（T1-9）：`ping` 往返上限——超过即视为不可用（触发重连）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 可用性判定（T1-9）：**在册且探活存活**才算"已连接"。
///
/// 旧实现只判断在册（`contains_key`）——服务器崩溃后仍报"已连接"，死连接
/// 被一直复用（工具看似在、调用必失败）。
fn usable(registered: bool, alive: bool) -> bool {
    registered && alive
}

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
    ///
    /// `env: None` means inherit the parent process environment.
    pub async fn connect_server(
        &self,
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
    ) -> McpResult<()> {
        let config = McpServerConfig {
            env,
            ..McpServerConfig::new(name.into(), command, args)
        };
        self.connect_from_config(&config).await
    }

    /// 按服务器配置连接并注册（transport 分发见 [`McpClient::connect_from_config`]）。
    ///
    /// 同名已存在时先断开再替换；工具数量计入日志（连接成功的可见信号）。
    pub async fn connect_from_config(&self, config: &McpServerConfig) -> McpResult<()> {
        // 替换路径必须按"在册"判断（不能用探活：死连接探活为 false 时
        // 若跳过 disconnect，旧 client 会被直接覆盖 Drop——不 kill 子进程）
        if self.is_registered(&config.name).await {
            self.disconnect_server(&config.name).await?;
        }

        let client = McpClient::connect_from_config(config).await?;
        let tool_count = client.list_tools().await.map(|t| t.len()).unwrap_or(0);
        let client = Arc::new(client);

        self.clients
            .write()
            .await
            .insert(config.name.clone(), client);

        tracing::info!(
            server = %config.name,
            tools = tool_count,
            "MCP server registered in manager"
        );
        Ok(())
    }

    /// 与配置对齐（sync 语义）：断开已移除/禁用的服务器，连接新增/启用的。
    ///
    /// 连接失败仅告警并跳过（fail fast，不重试）。配置变化时调用
    /// （如 server 层配置热更新 / agent reload）。
    pub async fn sync(&self, servers: &[McpServerConfig]) {
        let enabled: Vec<&str> = servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.name.as_str())
            .collect();

        // 断开配置中已不存在或已禁用的服务器
        let stale: Vec<String> = {
            let guard = self.clients.read().await;
            guard
                .keys()
                .filter(|k| !enabled.contains(&k.as_str()))
                .cloned()
                .collect()
        };
        for name in stale {
            tracing::info!(server = %name, "MCP 服务器已从配置移除，断开连接");
            let _ = self.disconnect_server(&name).await;
        }

        // 连接新增/启用的服务器（已连接则保持，不做重复握手）
        for server in servers.iter().filter(|s| s.enabled) {
            // 真探活（T1-9）：在册 ≠ 可用——服务器崩溃/远端断连后仅凭注册表
            // 会一直复用死连接；ping 往返失败即走重连（断开旧的再连新的）。
            if self.probe(&server.name, PROBE_TIMEOUT).await {
                continue;
            }
            if let Err(e) = self.connect_from_config(server).await {
                tracing::warn!(
                    server = %server.name,
                    error = %e,
                    "MCP 服务器连接失败，跳过其工具"
                );
            }
        }
    }

    /// Disconnect and remove a server by name.
    pub async fn disconnect_server(&self, name: &str) -> McpResult<()> {
        let mut guard = self.clients.write().await;
        if let Some(client) = guard.remove(name) {
            drop(guard);
            // Actually disconnect the client (shuts down subprocess)
            if let Err(e) = client.disconnect().await {
                tracing::warn!(server = %name, error = %e, "MCP 断开失败（子进程可能残留）");
            }
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

    /// 注册表在册判定（**不做探活**）：连接替换路径专用。
    pub async fn is_registered(&self, name: &str) -> bool {
        self.clients.read().await.contains_key(name)
    }

    /// 服务器是否**可用**：在册 + 客户端存活（T1-9）。
    ///
    /// 廉价（无网络往返，读客户端本地状态）；真往返探活见 [`Self::probe`]。
    pub async fn is_connected(&self, name: &str) -> bool {
        match self.get_client(name).await {
            Some(client) => usable(true, client.is_alive().await),
            None => usable(false, false),
        }
    }

    /// **真探活**：向已注册服务器发 MCP `ping`（带超时）。
    ///
    /// 未注册 / 已显式断开 / 超时 / 传输失败均返回 `false`。
    pub async fn probe(&self, name: &str, timeout: Duration) -> bool {
        match self.get_client(name).await {
            Some(client) => client.probe(timeout).await.is_ok(),
            None => false,
        }
    }

    /// Returns the number of connected servers.
    pub async fn connected_count(&self) -> usize {
        self.connected_servers().await.len()
    }

    /// 返回**可用**（存活）服务器名列表。
    ///
    /// T1-9：在册但已死的连接不列出——避免桥接层为其生成"调用必失败"的工具。
    pub async fn connected_servers(&self) -> Vec<String> {
        let clients: Vec<(String, Arc<McpClient>)> = {
            let guard = self.clients.read().await;
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut out = Vec::new();
        for (name, client) in clients {
            if client.is_alive().await {
                out.push(name);
            }
        }
        out
    }

    /// Disconnect all servers.
    pub async fn disconnect_all(&self) {
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
        let manager = McpClientManager::new();
        let result = futures::executor::block_on(manager.disconnect_server("ghost"));
        assert!(result.is_err());
    }

    #[test]
    fn test_connect_from_config_http_without_url_fails() {
        // transport=http 但未配置 url：连接分发应拒绝（不发起网络请求）
        let manager = McpClientManager::new();
        let config = McpServerConfig {
            name: "remote".to_string(),
            command: String::new(),
            args: vec![],
            env: None,
            enabled: true,
            description: None,
            transport: Some("http".to_string()),
            url: None,
        };
        let result = futures::executor::block_on(manager.connect_from_config(&config));
        let Err(err) = result else {
            panic!("transport=http 且未配置 url 应失败，实际成功");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("transport=http 但未配置 url"),
            "错误应携带原因: {msg}"
        );
    }

    #[test]
    fn test_connected_servers_empty() {
        let manager = McpClientManager::new();
        let servers = futures::executor::block_on(manager.connected_servers());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_usable_requires_alive() {
        // T1-9：在册但客户端已死 ⇒ 不可用（旧实现只看注册表，会误报"已连接"）
        assert!(!usable(true, false), "在册但不可用不应视为已连接");
        assert!(usable(true, true));
        assert!(!usable(false, true), "未在册不可用");
        assert!(!usable(false, false));
    }

    #[test]
    fn test_probe_and_registered_for_unknown() {
        let manager = McpClientManager::new();
        assert!(!futures::executor::block_on(manager.is_registered("ghost")));
        assert!(!futures::executor::block_on(
            manager.probe("ghost", Duration::from_millis(50))
        ));
    }
}
