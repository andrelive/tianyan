//! Configuration types for MCP server connections.

use std::collections::HashMap;

/// Configuration for a single MCP server connection.
///
/// This type is serializable/deserializable for use in config files
/// (e.g., `tianyan.toml`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpServerConfig {
    /// Unique name to identify this server connection.
    pub name: String,

    /// Command to spawn the MCP server process.
    pub command: String,

    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Optional environment variables for the server process.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,

    /// Whether this server is enabled (default: true).
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Optional human-readable description of the server.
    pub description: Option<String>,
}

fn default_enabled() -> bool {
    true
}

impl McpServerConfig {
    /// Create a new `McpServerConfig`.
    pub fn new(name: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            env: None,
            enabled: true,
            description: None,
        }
    }

    /// Set environment variables for the server process.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}
