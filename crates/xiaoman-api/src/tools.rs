//! Tools from outside the phone, for example MCP servers, offered to the chat model.

use async_trait::async_trait;

use crate::llm::ToolSpec;

/// Tools from outside the phone, offered next to the built-in memory / reminder / phone_task tools.
/// A provider owns its tool names and must never panic for a failing server: it lists fewer tools
/// or returns an error result instead.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// The tools to offer now. Names follow the OpenAI function-name rules: `[a-zA-Z0-9_-]{1,64}`.
    async fn specs(&self) -> Vec<ToolSpec>;

    /// Runs `name` when it belongs to this provider and returns the result text for the model;
    /// `None` when the provider does not own the name.
    async fn call(&self, task_id: &str, name: &str, arguments_json: &str) -> Option<ToolResult>;
}

/// `text` goes back to the chat model; `summary` is a short line for the chat UI and traces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub ok: bool,
    pub text: String,
    pub summary: String,
}

/// Confirmation before an external tool with side effects runs (booking a ride, placing an order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolApprovalRequest {
    pub task_id: String,
    /// Display name of the server, e.g. `滴滴出行`.
    pub server: String,
    /// Tool name on that server, e.g. `create_order`.
    pub tool: String,
    /// Human-readable title of the tool when the server provides one.
    pub title: Option<String>,
    pub arguments_json: String,
    /// Why confirmation is needed, shown to the user.
    pub reason: String,
}

#[async_trait]
pub trait ToolApprovalGateway: Send + Sync {
    async fn approve(&self, request: ToolApprovalRequest) -> bool;
}
