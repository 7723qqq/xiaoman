//! MCP lifecycle, pagination and tool result decoding shared by both transports.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use crate::mcp_config::{McpServerConfig, SecretRedactor};
use crate::transport::{McpError, McpTransport, PROTOCOL_VERSION};

#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCallResult {
    pub ok: bool,
    pub text: String,
}

pub struct McpClient {
    server: McpServerConfig,
    transport: Arc<dyn McpTransport>,
    version: String,
    ids: AtomicI64,
    initialized: tokio::sync::Mutex<bool>,
    redactor: SecretRedactor,
}

impl McpClient {
    pub fn new(server: McpServerConfig, transport: Arc<dyn McpTransport>) -> Self {
        let redactor = server.redactor();
        Self {
            server,
            transport,
            version: "1.0".to_string(),
            ids: AtomicI64::new(0),
            initialized: tokio::sync::Mutex::new(false),
            redactor,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn server(&self) -> &McpServerConfig {
        &self.server
    }

    pub async fn initialize(&self) -> Result<(), McpError> {
        let mut initialized = self.initialized.lock().await;
        if *initialized {
            return Ok(());
        }
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "xiaoman", "version": self.version},
            }),
        )
        .await?;
        self.safe_exchange(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        *initialized = true;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        self.initialize().await?;
        let mut tools = Vec::new();
        let mut cursors: BTreeSet<String> = BTreeSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request("tools/list", params).await?;
            if let Some(items) = result.get("tools").and_then(Value::as_array) {
                for item in items {
                    let Some(tool) = item.as_object() else {
                        continue;
                    };
                    let Some(name) = tool.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let annotations = tool.get("annotations").and_then(Value::as_object);
                    tools.push(McpTool {
                        name: name.to_string(),
                        title: tool
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| {
                                annotations
                                    .and_then(|annotations| annotations.get("title"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            }),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        input_schema: tool
                            .get("inputSchema")
                            .filter(|schema| schema.is_object())
                            .cloned()
                            .unwrap_or_else(empty_schema),
                        read_only: annotations
                            .and_then(|annotations| annotations.get("readOnlyHint"))
                            .and_then(Value::as_bool),
                    });
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string);
            match cursor {
                Some(next) if !cursors.insert(next.clone()) => {
                    return Err(McpError(format!(
                        "服务器 {} 分页游标重复",
                        self.server.name
                    )));
                }
                Some(_) => {}
                None => break,
            }
        }
        Ok(tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpCallResult, McpError> {
        self.initialize().await?;
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        let content = result
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let has_text = content
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("text"));
        let text = content
            .iter()
            .map(|item| {
                let part = item.as_object();
                match part
                    .and_then(|part| part.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("text") => part
                        .and_then(|part| part.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    Some("image") => "[图片内容]".to_string(),
                    Some("resource") | Some("resource_link") => "[资源内容]".to_string(),
                    Some("audio") => "[音频内容]".to_string(),
                    _ => "[非文本内容]".to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let structured = result
            .get("structuredContent")
            .filter(|value| !value.is_null())
            .map(Value::to_string);
        let decoded = match (has_text, structured) {
            (false, Some(structured)) => {
                let mut parts: Vec<String> = vec![text];
                parts.push(structured);
                parts
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => text,
        };
        let ok = result.get("isError").and_then(Value::as_bool) != Some(true);
        Ok(McpCallResult {
            ok,
            text: self.redactor.redact(&decoded),
        })
    }

    pub fn close(&self) {
        self.transport.close();
    }

    async fn request(&self, method: &str, params: Value) -> Result<Map<String, Value>, McpError> {
        let id = self.ids.fetch_add(1, Ordering::SeqCst) + 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = self
            .safe_exchange(message)
            .await?
            .ok_or_else(|| McpError(format!("服务器 {} 响应为空", self.server.name)))?;
        if response.get("id").and_then(Value::as_i64) != Some(id) {
            return Err(McpError(format!(
                "服务器 {} 响应 id 不匹配",
                self.server.name
            )));
        }
        if let Some(error) = response.get("error").and_then(Value::as_object) {
            let code = error.get("code").cloned().unwrap_or(Value::Null);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(McpError(self.redactor.redact(&format!(
                "服务器 {} RPC {}：{message}",
                self.server.name, code
            ))));
        }
        match response.get("result") {
            Some(Value::Object(result)) => Ok(result.clone()),
            _ => Err(McpError(format!(
                "服务器 {} 缺少 result 对象",
                self.server.name
            ))),
        }
    }

    async fn safe_exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        self.transport.exchange(message).await.map_err(|error| {
            if error.0.starts_with(&format!("服务器 {}", self.server.name)) {
                error
            } else {
                McpError(
                    self.redactor
                        .redact(&format!("服务器 {}：{}", self.server.name, error.0)),
                )
            }
        })
    }
}

fn empty_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

#[allow(dead_code)]
fn _unused(_: &Mutex<u8>) {}
