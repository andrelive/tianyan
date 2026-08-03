//! # Tianyan MCP Client
//!
//! A client library for connecting to MCP (Model Context Protocol) servers
//! via stdio transport. Provides connection management, tool listing,
//! and tool calling capabilities.
//!
//! ## Architecture
//!
//! - [`McpClient`] — Wraps a single MCP server connection.
//! - [`McpClientManager`] — Manages multiple named connections.
//! - [`McpError`] — Typed errors for all MCP operations.
//! - [`McpServerConfig`] — Serializable configuration for server connections.
//! - [`ToolInfo`] — Describes a tool provided by an MCP server.
//!
//! ## Feature flags
//!
//! This crate uses `rust-mcp-sdk` with `["client", "stdio"]` features enabled.
//!
//! ## Example
//!
//! ```no_run
//! use tianyan_mcp::{McpClient, McpClientManager};
//!
//! # async fn example() {
//! // Single client example
//! let mut client = McpClient::connect(
//!     "echo-server",
//!     "npx",
//!     vec!["-y".into(), "@modelcontextprotocol/server-everything".into()],
//!     None,
//! )
//! .await
//! .unwrap();
//!
//! let tools = client.list_tools().await.unwrap();
//! println!("Available tools: {}", tools.len());
//!
//! // Manager example
//! let mut manager = McpClientManager::new();
//! manager
//!     .connect_server(
//!         "filesystem",
//!         "npx",
//!         vec![
//!             "-y".into(),
//!             "@modelcontextprotocol/server-filesystem".into(),
//!             "/tmp".into(),
//!         ],
//!         None,
//!     )
//!     .await
//!     .unwrap();
//! # }
//! ```

pub mod client;
pub mod error;
pub mod manager;
pub mod types;

pub use client::{McpClient, ToolInfo};
pub use error::{McpError, McpResult};
pub use manager::McpClientManager;
pub use types::McpServerConfig;
