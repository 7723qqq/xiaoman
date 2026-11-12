//! Tool serving: lazily discovers independent servers and mediates every tool call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use xiaoman_api::{
    ToolApprovalGateway, ToolApprovalRequest, ToolProvider, ToolResult, ToolSpec, UserChannel,
};

use crate::client::{McpClient, McpTool};
use crate::mcp_config::{ConfirmPolicy, McpConfig, McpServerConfig, SecretRedactor};
use crate::transport::{self, McpError, McpTransport};

pub const DEFAULT_MAX_RESULT_CHARS: usize = 6000;
pub const DEFAULT_RETRY_AFTER_MILLIS: i64 = 5 * 60 * 1000;

const NAME_LIMIT: usize = 64;
const DESCRIPTION_LIMIT: usize = 600;
const TRUNCATION_MARKER: &str = "\n[结果已截断]";

/// One line of [`McpToolProvider::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub title: String,
    pub connected: bool,
    pub tool_count: usize,
    pub error: Option<String>,
}

type TransportFactory =
    dyn Fn(&McpServerConfig) -> Result<Arc<dyn McpTransport>, McpError> + Send + Sync;
type Clock = dyn Fn() -> i64 + Send + Sync;

struct State {
    /// Index into [`McpConfig::servers`], so every configured server has a status line.
    server_index: usize,
    server: McpServerConfig,
    client: Option<Arc<McpClient>>,
    tools: Vec<McpTool>,
    error: Option<String>,
    last_failure_at: Option<i64>,
}

#[derive(Clone)]
struct Binding {
    state: usize,
    tool: McpTool,
    spec: ToolSpec,
}

struct Inner {
    states: Vec<State>,
    bindings: Vec<Binding>,
}

/// Serves the tools of every enabled server of [`McpConfig`].
pub struct McpToolProvider {
    config: McpConfig,
    approvals: Arc<dyn ToolApprovalGateway>,
    factory: Box<TransportFactory>,
    max_result_chars: usize,
    retry_after_millis: i64,
    clock: Box<Clock>,
    redactor: SecretRedactor,
    inner: tokio::sync::Mutex<Inner>,
    /// Every client handed out, so [`Self::close`] reaches in-flight and installed transports.
    clients: Mutex<Vec<Arc<McpClient>>>,
    closed: AtomicBool,
}

impl McpToolProvider {
    /// Servers are only reached once [`ToolProvider::specs`] runs.
    pub fn new(config: McpConfig, approvals: Arc<dyn ToolApprovalGateway>) -> Self {
        let redactor = SecretRedactor::new(&config.servers);
        let states = config
            .servers
            .iter()
            .enumerate()
            .filter(|(_, server)| server.enabled)
            .map(|(index, server)| State {
                server_index: index,
                server: server.clone(),
                client: None,
                tools: Vec::new(),
                error: None,
                last_failure_at: None,
            })
            .collect();
        Self {
            config,
            approvals,
            factory: Box::new(|server: &McpServerConfig| transport::create(server)),
            max_result_chars: DEFAULT_MAX_RESULT_CHARS,
            retry_after_millis: DEFAULT_RETRY_AFTER_MILLIS,
            clock: Box::new(xiaoman_api::now_millis),
            redactor,
            inner: tokio::sync::Mutex::new(Inner {
                states,
                bindings: Vec::new(),
            }),
            clients: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub fn with_transport_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(&McpServerConfig) -> Result<Arc<dyn McpTransport>, McpError> + Send + Sync + 'static,
    {
        self.factory = Box::new(factory);
        self
    }

    pub fn with_max_result_chars(mut self, max_result_chars: usize) -> Self {
        assert!(max_result_chars > 0, "max_result_chars must be positive");
        self.max_result_chars = max_result_chars;
        self
    }

    pub fn with_retry_after_millis(mut self, retry_after_millis: i64) -> Self {
        assert!(
            retry_after_millis >= 0,
            "retry_after_millis must not be negative"
        );
        self.retry_after_millis = retry_after_millis;
        self
    }

    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> i64 + Send + Sync + 'static,
    {
        self.clock = Box::new(clock);
        self
    }

    /// Retries failed servers immediately when `refresh` is true; connected servers stay cached.
    pub async fn status(&self, refresh: bool) -> Vec<ServerStatus> {
        self.discover(refresh).await;
        let inner = self.inner.lock().await;
        self.config
            .servers
            .iter()
            .enumerate()
            .map(|(index, server)| {
                let state = inner
                    .states
                    .iter()
                    .find(|state| state.server_index == index);
                ServerStatus {
                    name: self.redactor.redact(&server.name),
                    title: self.redactor.redact(&server.title),
                    connected: state.is_some_and(|state| state.client.is_some())
                        && !self.is_closed(),
                    tool_count: state.map_or(0, |state| state.tools.len()),
                    error: state.and_then(|state| state.error.clone()),
                }
            })
            .collect()
    }

    /// Closes every transport and refuses further discovery or calls.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let clients =
            std::mem::take(&mut *self.clients.lock().unwrap_or_else(PoisonError::into_inner));
        for client in clients {
            client.close();
        }
    }

    async fn discover(&self, refresh: bool) -> Vec<ToolSpec> {
        let mut inner = self.inner.lock().await;
        if self.is_closed() {
            return Vec::new();
        }
        let now = (self.clock)();
        let retry_after = self.retry_after_millis;
        let pending: Vec<usize> = inner
            .states
            .iter()
            .enumerate()
            .filter(|(_, state)| {
                state.client.is_none()
                    && (refresh
                        || state
                            .last_failure_at
                            .is_none_or(|last_failure| now - last_failure >= retry_after))
            })
            .map(|(index, _)| index)
            .collect();
        enum Attempt {
            Connected {
                client: Arc<McpClient>,
                tools: Vec<McpTool>,
            },
            Failed(String),
            Closed,
        }
        // Servers are independent: one slow or dead server must not delay the others.
        let attempts = pending.into_iter().map(|index| {
            let server = inner.states[index].server.clone();
            async move {
                let client = match (self.factory)(&server) {
                    Ok(transport) => Arc::new(McpClient::new(server.clone(), transport)),
                    Err(error) => return (index, Attempt::Failed(error.0)),
                };
                self.track(&client);
                if self.is_closed() {
                    client.close();
                    self.untrack(&client);
                    return (index, Attempt::Closed);
                }
                match client.list_tools().await {
                    Ok(tools) => {
                        let tools: Vec<McpTool> = tools
                            .into_iter()
                            .filter(|tool| {
                                server
                                    .tools
                                    .as_ref()
                                    .is_none_or(|allowed| allowed.contains(&tool.name))
                            })
                            .collect();
                        (index, Attempt::Connected { client, tools })
                    }
                    Err(error) => {
                        client.close();
                        self.untrack(&client);
                        (index, Attempt::Failed(error.0))
                    }
                }
            }
        });
        for (index, attempt) in futures::future::join_all(attempts).await {
            match attempt {
                Attempt::Connected { client, tools } => {
                    if self.is_closed() {
                        client.close();
                        self.untrack(&client);
                        continue;
                    }
                    let state = &mut inner.states[index];
                    state.client = Some(client);
                    state.tools = tools;
                    state.error = None;
                    state.last_failure_at = None;
                }
                Attempt::Failed(message) => self.record_failure(&mut inner.states[index], &message),
                Attempt::Closed => {}
            }
        }
        if self.is_closed() {
            return Vec::new();
        }
        // Existing names stay stable when an unavailable server later recovers.
        let mut bindings = inner.bindings.clone();
        for (state_index, state) in inner.states.iter().enumerate() {
            if state.client.is_none() {
                continue;
            }
            for tool in &state.tools {
                if bindings
                    .iter()
                    .any(|binding| binding.state == state_index && binding.tool.name == tool.name)
                {
                    continue;
                }
                let base = clean_name(
                    &self
                        .redactor
                        .redact(&format!("{}__{}", state.server.name, tool.name)),
                );
                let mut name = base.clone();
                let mut suffix = 2;
                while bindings.iter().any(|binding| binding.spec.name == name) {
                    let ending = format!("_{suffix}");
                    suffix += 1;
                    name = format!(
                        "{}{ending}",
                        take_chars(&base, NAME_LIMIT - ending.chars().count())
                    );
                }
                let description = take_chars(
                    &self.redactor.redact(&format!(
                        "[服务器 {}] {}",
                        state.server.title,
                        tool_text(tool)
                    )),
                    DESCRIPTION_LIMIT,
                )
                .to_string();
                let parameters = self.redactor.redact_json(&tool.input_schema);
                bindings.push(Binding {
                    state: state_index,
                    tool: tool.clone(),
                    spec: ToolSpec {
                        name,
                        description,
                        parameters,
                    },
                });
            }
        }
        inner.bindings = bindings;
        inner
            .bindings
            .iter()
            .map(|binding| binding.spec.clone())
            .collect()
    }

    async fn invoke(&self, task_id: &str, name: &str, arguments_json: &str) -> Option<ToolResult> {
        let (server, tool, client) = {
            let inner = self.inner.lock().await;
            let binding = inner
                .bindings
                .iter()
                .find(|binding| binding.spec.name == name)?;
            let state = &inner.states[binding.state];
            (
                state.server.clone(),
                binding.tool.clone(),
                state.client.clone(),
            )
        };
        let label = self
            .redactor
            .redact(&format!("{}·{}", server.title, tool.name));
        if self.is_closed() {
            return Some(self.failure(&label, "MCP 已关闭"));
        }
        let arguments = match parse_arguments(arguments_json) {
            Ok(arguments) => arguments,
            Err(message) => return Some(self.failure(&label, &message)),
        };
        let confirm = match server.confirm {
            ConfirmPolicy::None => false,
            ConfirmPolicy::All => true,
            ConfirmPolicy::Writes => tool.read_only != Some(true),
        };
        if confirm {
            let reason = match server.confirm {
                ConfirmPolicy::All => "该服务器的所有工具调用都需要确认",
                _ => "该工具会修改外部数据或产生费用",
            };
            let request = ToolApprovalRequest {
                task_id: task_id.to_string(),
                server: self.redactor.redact(&server.title),
                tool: self.redactor.redact(&tool.name),
                title: tool
                    .title
                    .as_deref()
                    .map(|title| self.redactor.redact(title)),
                arguments_json: self.redactor.redact(&arguments.to_string()),
                reason: reason.to_string(),
            };
            if !self.approvals.approve(request).await {
                return Some(self.failure(&label, &format!("用户没有确认，未执行 {}", tool.name)));
            }
        }
        let Some(client) = client else {
            return Some(self.failure(&label, "MCP 服务器未连接"));
        };
        match client.call_tool(&tool.name, arguments).await {
            Ok(response) => Some(self.result(response.ok, &label, &response.text)),
            Err(error) => Some(self.failure(&label, &error.0)),
        }
    }

    fn record_failure(&self, state: &mut State, message: &str) {
        state.client = None;
        state.error =
            Some(take_chars(&self.redactor.redact(message), DESCRIPTION_LIMIT).to_string());
        state.last_failure_at = Some((self.clock)());
    }

    fn track(&self, client: &Arc<McpClient>) {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(client.clone());
    }

    fn untrack(&self, client: &Arc<McpClient>) {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|existing| !Arc::ptr_eq(existing, client));
    }

    fn result(&self, ok: bool, label: &str, text: &str) -> ToolResult {
        let summary = format!("{label} {}", if ok { "成功" } else { "失败" });
        ToolResult {
            ok,
            text: self.truncate(&self.redactor.redact(text)),
            summary: take_chars(&self.redactor.redact(&summary), DESCRIPTION_LIMIT).to_string(),
        }
    }

    fn failure(&self, label: &str, message: &str) -> ToolResult {
        // Redact before JSON escaping so quoted secrets cannot leak through an error result.
        let text = json!({ "error": self.redactor.redact(message) }).to_string();
        self.result(false, label, &text)
    }

    fn truncate(&self, text: &str) -> String {
        if text.chars().count() <= self.max_result_chars {
            return text.to_string();
        }
        let marker = TRUNCATION_MARKER.chars().count();
        if self.max_result_chars >= marker {
            format!(
                "{}{TRUNCATION_MARKER}",
                take_chars(text, self.max_result_chars - marker)
            )
        } else {
            take_chars("[已截断]", self.max_result_chars).to_string()
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

impl Drop for McpToolProvider {
    fn drop(&mut self) {
        self.close();
    }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
    async fn specs(&self) -> Vec<ToolSpec> {
        self.discover(false).await
    }

    async fn call(&self, task_id: &str, name: &str, arguments_json: &str) -> Option<ToolResult> {
        self.invoke(task_id, name, arguments_json).await
    }
}

/// Asks the user through the chat channel; only an explicit affirmative approves.
pub struct UserChannelApprovals {
    users: Arc<dyn UserChannel>,
}

impl UserChannelApprovals {
    pub fn new(users: Arc<dyn UserChannel>) -> Self {
        Self { users }
    }
}

#[async_trait]
impl ToolApprovalGateway for UserChannelApprovals {
    async fn approve(&self, request: ToolApprovalRequest) -> bool {
        let arguments = if request.arguments_json.chars().count() > 1000 {
            format!("{}[参数已截断]", take_chars(&request.arguments_json, 1000))
        } else {
            request.arguments_json.clone()
        };
        let title = request.title.as_deref().unwrap_or(&request.tool);
        let question = format!(
            "要调用「{title}」（{}），参数：{arguments}。{}。回复“确认”继续，其他回复取消。",
            request.server, request.reason
        );
        let reply = self.users.ask(&request.task_id, &question, false).await;
        matches!(
            reply.trim().to_lowercase().as_str(),
            "确认" | "是" | "好" | "可以" | "y" | "yes"
        )
    }
}

fn parse_arguments(arguments_json: &str) -> Result<Value, String> {
    if arguments_json.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    match serde_json::from_str::<Value>(arguments_json) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err("工具参数必须是 JSON 对象".to_string()),
    }
}

fn tool_text(tool: &McpTool) -> String {
    [tool.title.as_deref(), tool.description.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("：")
}

/// OpenAI function-name rules; overlength names keep a SHA-256 suffix so they stay distinct.
fn clean_name(raw: &str) -> String {
    let mut clean: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        clean = "mcp_tool".to_string();
    }
    if clean.chars().count() <= NAME_LIMIT {
        return clean;
    }
    let digest = Sha256::digest(raw.as_bytes());
    let hash: String = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}_{hash}", take_chars(&clean, 55))
}

fn take_chars(text: &str, count: usize) -> &str {
    match text.char_indices().nth(count) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}
