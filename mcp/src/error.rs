//! Error types for the MCP client module.

/// Errors that can occur during MCP client operations.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// Connection to an MCP server failed.
    #[error("Connection failed for server '{0}': {1}")]
    ConnectionFailed(String, String),

    /// The MCP server process crashed or exited unexpectedly.
    #[error("Server '{0}' crashed: {1}")]
    ServerCrashed(String, String),

    /// Requested tool was not found on the server.
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Protocol violation occurred.
    #[error("Protocol violation: {0}")]
    ProtocolViolation(String),

    /// Transport-level error (I/O, process spawn, etc.).
    #[error("Transport error: {0}")]
    Transport(String),
}

impl McpError {
    /// Create a `Transport` error from a boxed error.
    pub fn transport_from<E: std::error::Error + Send + Sync + 'static>(err: E) -> Self {
        McpError::Transport(err.to_string())
    }

    /// Create a `ConnectionFailed` error.
    pub fn connection_failed(server: impl Into<String>, reason: impl Into<String>) -> Self {
        McpError::ConnectionFailed(server.into(), reason.into())
    }

    /// Create a `ServerCrashed` error.
    pub fn server_crashed(server: impl Into<String>, reason: impl Into<String>) -> Self {
        McpError::ServerCrashed(server.into(), reason.into())
    }
}

/// Alias for `Result<T, McpError>`.
pub type McpResult<T> = Result<T, McpError>;
