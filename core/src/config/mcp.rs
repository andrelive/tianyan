//! MCP (Model Context Protocol) 服务器配置模块。
//!
//! 本模块定义 MCP 服务器在配置文件中的表示类型。
//! MCP 服务器为智能体提供额外的工具和资源。
//!
//! # 配置示例
//!
//! ```toml
//! [[mcp.servers]]
//! name = "filesystem"
//! command = "npx"
//! args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
//! enabled = false
//! description = "Secure file operations"
//! ```

use crate::common::error::TianyanError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP 服务器配置。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// MCP 服务器列表。
    #[serde(default)]
    pub servers: Vec<McpServerEntry>,
}

/// 单个 MCP 服务器定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// 服务器唯一名称。
    pub name: String,
    /// 启动服务器进程的命令。
    pub command: String,
    /// 传递给命令的参数。
    #[serde(default)]
    pub args: Vec<String>,
    /// 服务器进程的环境变量。
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// 是否启用此服务器（默认 true）。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 可读描述。
    #[serde(default)]
    pub description: Option<String>,
    /// 传输方式（G7）：`"stdio"`（默认，本地进程）| `"http"`（远程
    /// streamable HTTP 服务器）。`http` 时必须配置 `url`。
    #[serde(default)]
    pub transport: Option<String>,
    /// 远程服务器地址（transport = "http" 时必填；http/https 绝对地址）。
    #[serde(default)]
    pub url: Option<String>,
}

fn default_enabled() -> bool {
    true
}

impl McpServerEntry {
    /// 创建新的 MCP 服务器配置。
    pub fn new(name: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            env: None,
            enabled: true,
            description: None,
            transport: None,
            url: None,
        }
    }

    /// 设置远程 streamable HTTP 传输（G7）：`url` 为 http/https 绝对地址。
    pub fn with_http_transport(mut self, url: impl Into<String>) -> Self {
        self.transport = Some("http".to_string());
        self.url = Some(url.into());
        self
    }

    /// 设置环境变量。
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    /// 设置描述。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 验证 MCP 服务器配置是否有效。
    pub fn validate(&self) -> Result<(), TianyanError> {
        if self.name.is_empty() {
            return Err(TianyanError::Custom(format!(
                "'{}' 的配置值无效：{}",
                "mcp.servers.name", "MCP 服务器名称不能为空"
            )));
        }
        if self.command.is_empty() {
            let key = format!("mcp.servers['{}'].command", self.name);
            let message = format!("MCP 服务器 '{}' 的命令不能为空", self.name);
            return Err(TianyanError::Custom(format!(
                "'{}' 的配置值无效：{}",
                key, message
            )));
        }
        Ok(())
    }
}

impl McpConfig {
    /// 验证 MCP 服务器配置。
    pub fn validate(&self) -> Result<(), TianyanError> {
        for server in &self.servers {
            server.validate()?;
        }
        Ok(())
    }

    /// 返回已启用的服务器列表。
    pub fn enabled_servers(&self) -> Vec<&McpServerEntry> {
        self.servers.iter().filter(|s| s.enabled).collect()
    }

    /// 返回已禁用的服务器列表。
    pub fn disabled_servers(&self) -> Vec<&McpServerEntry> {
        self.servers.iter().filter(|s| !s.enabled).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = McpConfig::default();
        assert!(config.servers.is_empty());
        assert!(config.enabled_servers().is_empty());
        assert!(config.disabled_servers().is_empty());
    }

    #[test]
    fn test_valid_server_entry() {
        let entry = McpServerEntry {
            name: "filesystem".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
            ],
            env: None,
            enabled: true,
            description: Some("File operations".to_string()),
            transport: None,
            url: None,
        };
        assert!(entry.validate().is_ok());
    }

    #[test]
    fn test_empty_name_fails_validation() {
        let entry = McpServerEntry {
            name: "".to_string(),
            command: "npx".to_string(),
            args: vec![],
            env: None,
            enabled: true,
            description: None,
            transport: None,
            url: None,
        };
        let result = entry.validate();
        assert!(result.is_err());
        let message = match result {
            Err(e) => e.to_string(),
            Ok(()) => String::new(),
        };
        assert!(message.contains("名称不能为空"));
    }

    #[test]
    fn test_empty_command_fails_validation() {
        let entry = McpServerEntry {
            name: "test".to_string(),
            command: "".to_string(),
            args: vec![],
            env: None,
            enabled: true,
            description: None,
            transport: None,
            url: None,
        };
        let result = entry.validate();
        assert!(result.is_err());
        let message = match result {
            Err(e) => e.to_string(),
            Ok(()) => String::new(),
        };
        assert!(message.contains("命令不能为空"));
    }

    #[test]
    fn test_enabled_disabled_servers() {
        let config = McpConfig {
            servers: vec![
                McpServerEntry {
                    name: "server1".to_string(),
                    command: "cmd1".to_string(),
                    args: vec![],
                    env: None,
                    enabled: true,
                    description: None,
                    transport: None,
                    url: None,
                },
                McpServerEntry {
                    name: "server2".to_string(),
                    command: "cmd2".to_string(),
                    args: vec![],
                    env: None,
                    enabled: false,
                    description: None,
                    transport: None,
                    url: None,
                },
            ],
        };
        assert_eq!(config.enabled_servers().len(), 1);
        assert_eq!(config.disabled_servers().len(), 1);
        assert_eq!(config.enabled_servers()[0].name, "server1");
        assert_eq!(config.disabled_servers()[0].name, "server2");
    }

    #[test]
    fn test_serialization_round_trip() {
        let config = McpConfig {
            servers: vec![McpServerEntry {
                name: "filesystem".to_string(),
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-filesystem".to_string(),
                ],
                env: Some(HashMap::from([(
                    "ALLOWED_PATH".to_string(),
                    "/tmp".to_string(),
                )])),
                enabled: false,
                description: Some("Safe file access".to_string()),
                transport: None,
                url: None,
            }],
        };
        let toml_str = match toml::to_string_pretty(&config) {
            Ok(s) => s,
            Err(e) => panic!("McpConfig 序列化失败: {e}"),
        };
        let parsed: McpConfig = match toml::from_str(&toml_str) {
            Ok(c) => c,
            Err(e) => panic!("McpConfig 反序列化失败: {e}"),
        };
        assert_eq!(parsed.servers.len(), 1);
        assert_eq!(parsed.servers[0].name, "filesystem");
        assert_eq!(parsed.servers[0].command, "npx");
        assert_eq!(parsed.servers[0].args.len(), 2);
        assert!(parsed.servers[0].env.is_some());
        assert!(!parsed.servers[0].enabled);
    }

    #[test]
    fn test_config_validation() {
        let config = McpConfig {
            servers: vec![McpServerEntry {
                name: "valid".to_string(),
                command: "cmd".to_string(),
                args: vec![],
                env: None,
                enabled: true,
                description: None,
                transport: None,
                url: None,
            }],
        };
        assert!(config.validate().is_ok());

        let invalid_config = McpConfig {
            servers: vec![McpServerEntry {
                name: "".to_string(),
                command: "cmd".to_string(),
                args: vec![],
                env: None,
                enabled: true,
                description: None,
                transport: None,
                url: None,
            }],
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_http_transport_roundtrip() {
        // G7：streamable HTTP 传输配置（transport=http + url）随 TOML 往返
        let config = McpConfig {
            servers: vec![McpServerEntry::new("remote", "unused", vec![])
                .with_http_transport("https://mcp.example.com/mcp")
                .with_description("远程服务器")],
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: McpConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.servers[0].transport.as_deref(), Some("http"));
        assert_eq!(
            parsed.servers[0].url.as_deref(),
            Some("https://mcp.example.com/mcp")
        );

        // 旧配置（无 transport/url 字段）兼容解析：默认 stdio
        let legacy: McpConfig = toml::from_str(
            "[[servers]]\nname = \"old\"\ncommand = \"npx\"\nargs = []\n",
        )
        .unwrap();
        assert!(legacy.servers[0].transport.is_none());
        assert!(legacy.servers[0].url.is_none());
    }
}
