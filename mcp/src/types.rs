//! Configuration types for MCP server connections.
//!
//! The server configuration type is defined in `tianyan-core`
//! (`tianyan::config::mcp::McpServerEntry`) as the single source of truth;
//! this crate re-exports it under the `McpServerConfig` name for API
//! compatibility.

/// Configuration for a single MCP server connection.
///
/// This type is serializable/deserializable for use in config files
/// (e.g., `tianyan.toml`).
pub use tianyan::config::mcp::McpServerEntry as McpServerConfig;
