//! JSON-RPC transports: HTTP (streamable) and stdio.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::mcp_config::{McpServerConfig, SecretRedactor};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// One JSON-RPC exchange; notifications have no id and return `None`.
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError>;

    fn close(&self);
}

/// Safe exception without a raw transport cause that could disclose credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpError(pub String);

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpError {}

pub fn create(server: &McpServerConfig) -> Result<Arc<dyn McpTransport>, McpError> {
    if server.url.is_some() {
        Ok(Arc::new(McpHttpTransport::new(server.clone())))
    } else {
        Ok(Arc::new(McpStdioTransport::spawn(server.clone(), None)?))
    }
}

pub struct McpHttpTransport {
    server: McpServerConfig,
    client: reqwest::Client,
    session_id: Mutex<Option<String>>,
    initialized: AtomicBool,
    closed: AtomicBool,
    redactor: SecretRedactor,
}

impl McpHttpTransport {
    pub fn new(server: McpServerConfig) -> Self {
        let timeout = Duration::from_secs(server.timeout_seconds.max(1) as u64);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            // MCP servers are often local; the system proxy would break loopback calls.
            .no_proxy()
            .build()
            .unwrap_or_default();
        let redactor = server.redactor();
        Self {
            server,
            client,
            session_id: Mutex::new(None),
            initialized: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            redactor,
        }
    }

    fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    fn message_id(message: &Value) -> Option<&Value> {
        message.get("id").filter(|id| !id.is_null())
    }

    fn matches_id(candidate: &Value, id: &Value) -> bool {
        candidate.get("id").is_some_and(|value| value == id)
    }
}

#[async_trait]
impl McpTransport for McpHttpTransport {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError("传输已关闭".to_string()));
        }
        let url = self.server.url.clone().unwrap_or_default();
        let mut request = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(message.to_string());
        for (key, value) in &self.server.headers {
            request = request.header(key, value);
        }
        if self.is_initialized() {
            request = request.header("MCP-Protocol-Version", PROTOCOL_VERSION);
        }
        if let Some(session) = self.session_id.lock().unwrap().clone() {
            request = request.header("Mcp-Session-Id", session);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Err(McpError(
                    self.redactor
                        .redact(&format!("服务器 {} 请求失败：{error}", self.server.name)),
                ));
            }
        };

        let status = response.status();
        let session_header = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_type_is_sse = response
            .headers()
            .get_all("Content-Type")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.to_lowercase().starts_with("text/event-stream"));
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            let detail = self
                .redactor
                .redact(&body)
                .replace(&self.redactor.redact(&url), "[服务器地址]");
            let detail: String = detail.chars().take(200).collect();
            return Err(McpError(format!(
                "服务器 {} HTTP {}：{detail}",
                self.server.name,
                status.as_u16()
            )));
        }
        if let Some(session) = session_header {
            *self.session_id.lock().unwrap() = Some(session);
        }
        let Some(id) = Self::message_id(&message) else {
            return Ok(None);
        };

        let result = if content_type_is_sse {
            let found = parse_sse(&body, id);
            found
                .ok_or_else(|| McpError(format!("服务器 {} 未返回匹配的响应", self.server.name)))?
        } else {
            match serde_json::from_str::<Value>(&body) {
                Ok(Value::Object(object)) => Value::Object(object),
                _ => {
                    return Err(McpError(format!(
                        "服务器 {} 响应格式无效",
                        self.server.name
                    )));
                }
            }
        };
        if !Self::matches_id(&result, id) {
            return Err(McpError(format!(
                "服务器 {} 响应 id 不匹配",
                self.server.name
            )));
        }
        if message.get("method").and_then(Value::as_str) == Some("initialize")
            && result.get("error").is_none()
        {
            self.initialized.store(true, Ordering::SeqCst);
        }
        Ok(Some(result))
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// Finds the SSE event whose payload matches `id`; events are separated by a blank line.
fn parse_sse(body: &str, id: &Value) -> Option<Value> {
    let mut data: Vec<&str> = Vec::new();
    let try_event = |data: &mut Vec<&str>| -> Option<Value> {
        if data.is_empty() {
            return None;
        }
        let joined = data.join("\n");
        data.clear();
        match serde_json::from_str::<Value>(&joined) {
            Ok(value) if value.get("id").is_some_and(|candidate| candidate == id) => Some(value),
            _ => None,
        }
    };
    for line in body.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(found) = try_event(&mut data) {
                return Some(found);
            }
            continue;
        }
        if line == "data" {
            data.push("");
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    try_event(&mut data)
}

struct StdioIo {
    stdin: tokio::process::ChildStdin,
    incoming: mpsc::Receiver<Option<Value>>,
    child: tokio::process::Child,
}

pub struct McpStdioTransport {
    server: McpServerConfig,
    io: tokio::sync::Mutex<StdioIo>,
    closed: AtomicBool,
    redactor: SecretRedactor,
}

impl McpStdioTransport {
    /// Desktop-only transport. Stdout is drained independently so requests can time out.
    /// `inherited_environment` limits which variables of this process the child sees (`None` = all).
    pub fn spawn(
        server: McpServerConfig,
        inherited_environment: Option<&[String]>,
    ) -> Result<Self, McpError> {
        let command = server.command.clone().unwrap_or_default();
        let line = command_line(&command, &server.args);
        let mut child = tokio::process::Command::new(&line[0]);
        child.args(&line[1..]);
        child.stdin(std::process::Stdio::piped());
        child.stdout(std::process::Stdio::piped());
        child.stderr(std::process::Stdio::null());
        if let Some(keep) = inherited_environment {
            child.env_clear();
            for (key, value) in std::env::vars() {
                if keep
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(&key))
                {
                    child.env(key, value);
                }
            }
        }
        for (key, value) in &server.env {
            child.env(key, value);
        }
        let redactor = server.redactor();
        let mut child = child.spawn().map_err(|error| {
            McpError(redactor.redact(&format!("服务器 {} 启动失败：{error}", server.name)))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError("无法写入子进程".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError("无法读取子进程".to_string()))?;

        let (sender, receiver) = mpsc::channel::<Option<Value>>(256);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value.is_object()
                    && value.get("id").is_some()
                    && sender.send(Some(value)).await.is_err()
                {
                    return;
                }
            }
            let _ = sender.send(None).await;
        });

        Ok(Self {
            server,
            io: tokio::sync::Mutex::new(StdioIo {
                stdin,
                incoming: receiver,
                child,
            }),
            closed: AtomicBool::new(false),
            redactor,
        })
    }
}

#[async_trait]
impl McpTransport for McpStdioTransport {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        let mut io = self.io.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError("传输已关闭".to_string()));
        }
        let mut line = message.to_string();
        line.push('\n');
        if let Err(error) = io.stdin.write_all(line.as_bytes()).await {
            return Err(McpError(
                self.redactor
                    .redact(&format!("服务器 {} 请求失败：{error}", self.server.name)),
            ));
        }
        let _ = io.stdin.flush().await;

        let Some(id) = message.get("id").filter(|id| !id.is_null()).cloned() else {
            return Ok(None);
        };
        let timeout = Duration::from_secs(self.server.timeout_seconds.max(1) as u64);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(McpError(format!("服务器 {} 请求超时", self.server.name)));
            }
            match tokio::time::timeout(remaining, io.incoming.recv()).await {
                Err(_) => return Err(McpError(format!("服务器 {} 请求超时", self.server.name))),
                Ok(None) => {
                    return Err(McpError(format!("服务器 {} 进程已退出", self.server.name)));
                }
                Ok(Some(None)) => {
                    return Err(McpError(format!("服务器 {} 进程已退出", self.server.name)));
                }
                Ok(Some(Some(response))) => {
                    if response.get("id").is_some_and(|candidate| *candidate == id) {
                        return Ok(Some(response));
                    }
                }
            }
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut io) = self.io.try_lock() {
            let _ = io.child.start_kill();
        }
    }
}

impl Drop for McpStdioTransport {
    fn drop(&mut self) {
        let _ = self.io.get_mut().child.start_kill();
    }
}

/// Arguments `cmd.exe` passes through unchanged, so wrapping a `.cmd` launcher cannot inject commands.
fn is_plain_argument(text: &str) -> bool {
    text.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '.' | '/' | ':' | '=' | '@' | '+' | ',' | '-')
    })
}

/// On Windows, `npx` and similar Node.js launchers are `.cmd` scripts that cannot be started by name;
/// they are found through PATH/PATHEXT and run via `cmd.exe /d /c` when all arguments are plain.
pub fn command_line(command: &str, args: &[String]) -> Vec<String> {
    let windows = std::env::consts::OS == "windows";
    if !windows || command.contains('/') || command.contains('\\') || command.contains('.') {
        let mut line = vec![command.to_string()];
        line.extend(args.iter().cloned());
        return line;
    }
    let extensions: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|extension| !extension.is_empty())
        .map(str::to_string)
        .collect();
    let path = std::env::var("PATH").unwrap_or_default();
    let mut found: Option<std::path::PathBuf> = None;
    'outer: for directory in path.split(';').filter(|directory| !directory.is_empty()) {
        for extension in &extensions {
            let candidate = std::path::Path::new(directory)
                .join(format!("{command}{}", extension.to_lowercase()));
            if candidate.is_file() {
                found = Some(candidate);
                break 'outer;
            }
        }
    }
    let Some(found) = found else {
        let mut line = vec![command.to_string()];
        line.extend(args.iter().cloned());
        return line;
    };
    let extension = found
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if extension != "cmd" && extension != "bat" {
        let mut line = vec![found.to_string_lossy().to_string()];
        line.extend(args.iter().cloned());
        return line;
    }
    if args.iter().all(|argument| is_plain_argument(argument)) {
        let mut line = vec![
            "cmd.exe".to_string(),
            "/d".to_string(),
            "/c".to_string(),
            command.to_string(),
        ];
        line.extend(args.iter().cloned());
        return line;
    }
    let mut line = vec![command.to_string()];
    line.extend(args.iter().cloned());
    line
}

#[allow(dead_code)]
fn _unused(_: &Map<String, Value>, _: &SecretRedactor) {}
