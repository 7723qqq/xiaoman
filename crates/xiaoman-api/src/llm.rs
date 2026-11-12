//! OpenAI-compatible chat: model configuration, messages, tool specs and the client trait.

use std::error::Error;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Which job a model is configured for. The chat model talks; the operator model reads screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelRole {
    #[serde(rename = "CHAT")]
    Chat,
    #[serde(rename = "OPERATOR")]
    Operator,
}

/// An OpenAI-compatible endpoint, e.g. Volcano Ark `https://ark.cn-beijing.volces.com/api/v3`,
/// Alibaba Bailian, OpenRouter, or a local vLLM/Ollama server.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub role: ModelRole,
    pub base_url: String,
    pub model: String,
    /// Never logged or written to traces.
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: i64,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub extra_headers: std::collections::HashMap<String, String>,
}

fn default_timeout_seconds() -> i64 {
    120
}

impl fmt::Debug for ModelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelConfig")
            .field("role", &self.role)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"***")
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("timeout_seconds", &self.timeout_seconds)
            .field(
                "extra_headers",
                &self.extra_headers.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ModelConfig {
    pub fn new(
        role: ModelRole,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            role,
            base_url: base_url.into(),
            model: model.into(),
            api_key: api_key.into(),
            temperature: None,
            max_tokens: None,
            timeout_seconds: default_timeout_seconds(),
            extra_headers: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRole {
    #[serde(rename = "SYSTEM")]
    System,
    #[serde(rename = "USER")]
    User,
    #[serde(rename = "ASSISTANT")]
    Assistant,
    #[serde(rename = "TOOL")]
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentPart {
    Text(String),
    ImagePng(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub parts: Vec<ContentPart>,
    /// Set on ASSISTANT messages that requested tools.
    pub tool_calls: Vec<ToolCall>,
    /// Set on TOOL messages: the id of the call this message answers.
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn new(role: ChatRole, parts: Vec<ContentPart>) -> Self {
        Self {
            role,
            parts,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(ChatRole::System, vec![ContentPart::Text(text.into())])
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(ChatRole::User, vec![ContentPart::Text(text.into())])
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(ChatRole::Assistant, vec![ContentPart::Text(text.into())])
    }

    pub fn tool(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        let mut message = Self::new(ChatRole::Tool, vec![ContentPart::Text(text.into())]);
        message.tool_call_id = Some(call_id.into());
        message
    }

    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.as_str()),
                ContentPart::ImagePng(_) => None,
            })
            .collect()
    }
}

/// A function the chat model may call. `parameters` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    /// Ask for a JSON object reply (`response_format: json_object`) when the endpoint supports it.
    pub json_mode: bool,
}

impl ChatRequest {
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            tools: Vec::new(),
            json_mode: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    pub message: String,
    pub status_code: Option<u16>,
    pub retryable: bool,
    pub provider_code: Option<String>,
}

impl LlmError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status_code: None,
            retryable: false,
            provider_code: None,
        }
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn status(mut self, status_code: u16) -> Self {
        self.status_code = Some(status_code);
        self
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status_code {
            Some(status) => write!(f, "{} (HTTP {status})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl Error for LlmError {}

#[async_trait]
pub trait LlmClient: Send + Sync {
    fn config(&self) -> &ModelConfig;

    /// Completes one chat request, after the client's own retries.
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;
}
