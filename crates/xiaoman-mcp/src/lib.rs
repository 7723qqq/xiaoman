//! MCP client: server config, transports, tool serving and the one-tap catalog.

pub mod catalog;
pub mod client;
pub mod mcp_config;
pub mod provider;
pub mod transport;

pub use catalog::{
    CatalogKey, CatalogServer, CustomMcpServer, McpCatalog, McpPlatform, McpSelection,
};
pub use client::{McpCallResult, McpClient, McpTool};
pub use mcp_config::{ConfirmPolicy, McpConfig, McpServerConfig, SecretRedactor};
pub use provider::{
    DEFAULT_MAX_RESULT_CHARS, DEFAULT_RETRY_AFTER_MILLIS, McpToolProvider, ServerStatus,
    UserChannelApprovals,
};
pub use transport::{
    McpError, McpHttpTransport, McpStdioTransport, McpTransport, PROTOCOL_VERSION, command_line,
    create,
};
