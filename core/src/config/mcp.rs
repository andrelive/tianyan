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
}

fn default_enabled() -> bool {
    true
}

impl McpServerEntry {
    /// 验证 MCP 服务器配置是否有效。
    pub fn validate(&self) -> Result<(), TianyanError> {
        if self.name.is_empty() {
            return Err(TianyanError::Custom(format!(
                "'{}' 的配置值无效：{}",
                "mcp.servers.name", "MCP 服务器名称不能为空"
            )));
        }
        if self.command.is_empty() {
            return Err(TianyanError::Custom(format!(
                "'{}' 的配置值无效：{}",
                format!("mcp.servers['{}'].command", self.name),
                format!("MCP 服务器 '{}' 的命令不能为空", self.name)
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
        };
        let result = entry.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("名称不能为空"));
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
        };
        let result = entry.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("命令不能为空"));
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
                },
                McpServerEntry {
                    name: "server2".to_string(),
                    command: "cmd2".to_string(),
                    args: vec![],
                    env: None,
                    enabled: false,
                    description: None,
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
            }],
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: McpConfig = toml::from_str(&toml_str).unwrap();
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
            }],
        };
        assert!(invalid_config.validate().is_err());
    }
}
